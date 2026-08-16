//! The control-lease state machine (PRD §5.6).
//!
//! Authority is equal between a human and an agent, but there is physically one
//! keyboard: if both type at once the screen is corrupted. So what this
//! arbitrates is not authority but **input order** — exactly one Session holds
//! `WRITE` at a time, and the rules for who may take it from whom are the
//! product's, not an implementation detail.
//!
//! It is a pure state machine on purpose. Every rule below — the one-way
//! human-preempts-agent asymmetry, the 300-second idle handover, no queue —
//! is decided here from an explicit `now`, with no clock, no lock, and no
//! platform, so all of it is testable without a running Desktop. The Control
//! Plane will own the authoritative instance (§7.5), and `iapetusd` performs
//! the one thing that must happen inside the guest: releasing held keys before
//! a handover.
//!
//! Time is passed in rather than read. A lease decision that depended on a
//! hidden clock could not be tested for the boundaries that matter — the second
//! before an idle handover versus the second after — and those boundaries are
//! the whole design.

use std::time::Duration;

pub use crate::v1::{ActorType, ControlLevel};

/// Who is asking. The id distinguishes two agents, or two people, from each
/// other; the type decides which arbitration rule applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actor {
    pub kind: ActorType,
    pub id: String,
}

impl Actor {
    pub fn agent(id: impl Into<String>) -> Self {
        Self { kind: ActorType::Agent, id: id.into() }
    }
    pub fn human(id: impl Into<String>) -> Self {
        Self { kind: ActorType::Human, id: id.into() }
    }
    fn is_human(&self) -> bool {
        self.kind == ActorType::Human
    }
}

/// A monotonic instant, in milliseconds from an arbitrary origin.
///
/// The caller supplies it. `iapetusd` and the Control Plane both have a
/// monotonic clock; the state machine only needs the differences between the
/// values, so the origin is irrelevant and passing it in keeps this pure.
pub type Millis = u64;

/// Who currently holds `WRITE`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Holder {
    session: String,
    actor: Actor,
    since: Millis,
    /// The last time this holder actually sent input. Only meaningful for a
    /// human holder, whose lease passes to a waiting agent after an idle gap.
    last_input: Millis,
    /// The most recent heartbeat. A holder that stops heartbeating for three
    /// intervals has its lease reclaimed (§5.6).
    last_heartbeat: Millis,
    heartbeat_interval: Duration,
}

/// The outcome of an acquire attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Acquired {
    /// The lease is now the requester's, and nobody had to give it up.
    Granted,
    /// The requester already held it — acquiring again is a no-op, not an error,
    /// so a client that retries after a dropped response is not punished.
    AlreadyHeld,
    /// A human took the lease from an agent. The previous holder must be sent
    /// `control.revoked` and dropped to `READ`, and the guest must release the
    /// agent's held keys before the human's first keystroke (§5.6).
    Preempted { previous: Revoked },
}

/// Why an acquire failed. Maps to `CONTROL_HELD` (§8.9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub holder: Actor,
    pub since: Millis,
    pub retry_after: Duration,
}

/// The holder that lost the lease, for the `control.revoked` event and the
/// key-release that must precede handover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revoked {
    pub session: String,
    pub actor: Actor,
    pub reason: RevokeReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevokeReason {
    /// A human preempted an agent.
    Preempted,
    /// The lease expired — three heartbeats missed.
    LeaseExpired,
    /// The holder released it, or a human's lease idled out to a waiting agent.
    Released,
}

/// The single input lease for one Desktop.
#[derive(Debug, Default)]
pub struct ControlLease {
    holder: Option<Holder>,
}

impl ControlLease {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether `session` currently holds `WRITE`.
    #[must_use]
    pub fn holds_write(&self, session: &str) -> bool {
        self.holder.as_ref().is_some_and(|h| h.session == session)
    }

    /// The actor currently holding `WRITE`, if any.
    #[must_use]
    pub fn holder(&self) -> Option<&Actor> {
        self.holder.as_ref().map(|h| &h.actor)
    }

    /// The session id currently holding `WRITE`, if any. Lets a viewer tell
    /// whether the operator is itself or another person (§7.5 V-10).
    #[must_use]
    pub fn holder_session(&self) -> Option<String> {
        self.holder.as_ref().map(|h| h.session.clone())
    }

    /// Requests the lease for `session`/`actor` at time `now`.
    ///
    /// Applies the §5.6 arbitration table. An expired or idled-out holder is
    /// cleared first, so a request that arrives just after the gap succeeds
    /// rather than bouncing off a holder that is no longer really there.
    pub fn acquire(
        &mut self,
        session: &str,
        actor: &Actor,
        now: Millis,
        heartbeat_interval: Duration,
    ) -> std::result::Result<Acquired, Held> {
        // A holder that has expired or idled out is gone before anyone else is
        // judged against it. This is also where a human's idle handover
        // happens: the lease simply becomes free for the next requester.
        self.reap(now);

        match &self.holder {
            // Free — first come, first served.
            None => {
                self.set_holder(session, actor, now, heartbeat_interval);
                Ok(Acquired::Granted)
            }

            // The same session asking again. Idempotent, and it refreshes the
            // input timestamp so a re-acquire is not read as idleness.
            Some(h) if h.session == session => {
                self.mark_input(session, now);
                Ok(Acquired::AlreadyHeld)
            }

            Some(h) => {
                let holder_is_human = h.actor.is_human();
                match (holder_is_human, actor.is_human()) {
                    // Human preempts agent — the one direction that is allowed.
                    // Intervention happens precisely when the agent is stuck or
                    // wrong, and a human cannot wait in a queue (§5.6).
                    (false, true) => {
                        let previous = Revoked {
                            session: h.session.clone(),
                            actor: h.actor.clone(),
                            reason: RevokeReason::Preempted,
                        };
                        self.set_holder(session, actor, now, heartbeat_interval);
                        Ok(Acquired::Preempted { previous })
                    }
                    // Every other pairing is rejected, never queued. Agent-pushes-
                    // human is forbidden outright (it would discard the person's
                    // input); the same-type cases are the no-queue policy (§5.6).
                    _ => Err(Held {
                        holder: h.actor.clone(),
                        since: h.since,
                        retry_after: Duration::from_secs(u64::from(
                            crate::limits::CONTROL_RETRY_AFTER_SEC,
                        )),
                    }),
                }
            }
        }
    }

    /// Records that the holder sent input, resetting its idle clock.
    ///
    /// Called for every input action the holder performs. Without it a human
    /// typing steadily would still hand the lease to a waiting agent 300
    /// seconds after acquiring it, mid-task.
    pub fn mark_input(&mut self, session: &str, now: Millis) {
        if let Some(h) = &mut self.holder {
            if h.session == session {
                h.last_input = now;
                h.last_heartbeat = now;
            }
        }
    }

    /// Records a heartbeat from the holder, which keeps the lease alive without
    /// counting as input for idle purposes.
    pub fn heartbeat(&mut self, session: &str, now: Millis) {
        if let Some(h) = &mut self.holder {
            if h.session == session {
                h.last_heartbeat = now;
            }
        }
    }

    /// The holder voluntarily gives up the lease.
    ///
    /// Returns the revoked holder so the guest can release its keys — even a
    /// clean release must leave a clean input state for whoever is next (§5.6).
    /// A release by a session that does not hold it is a harmless no-op.
    pub fn release(&mut self, session: &str) -> Option<Revoked> {
        match &self.holder {
            Some(h) if h.session == session => {
                let r = Revoked {
                    session: h.session.clone(),
                    actor: h.actor.clone(),
                    reason: RevokeReason::Released,
                };
                self.holder = None;
                Some(r)
            }
            _ => None,
        }
    }

    /// Clears the holder if it has expired or (for a human) idled out.
    ///
    /// Returns the revoked holder when one was cleared, so a caller polling on a
    /// timer can emit `control.revoked` for a lease that lapsed with nobody
    /// contending for it. Call it before arbitration and, ideally, on a timer.
    pub fn reap(&mut self, now: Millis) -> Option<Revoked> {
        let Some(h) = &self.holder else { return None };

        let ttl = h.heartbeat_interval.as_millis() as u64
            * u64::from(crate::limits::LEASE_MISSED_INTERVALS);
        if now.saturating_sub(h.last_heartbeat) >= ttl {
            return self.take_holder(RevokeReason::LeaseExpired);
        }

        // A human's lease passes to whoever is waiting after the idle gap; an
        // agent's never idles out — only a human may take an agent's lease, and
        // that path is preemption, not this one.
        if h.actor.is_human() {
            let idle = u64::from(crate::limits::HUMAN_IDLE_HANDOVER_SEC) * 1000;
            if now.saturating_sub(h.last_input) >= idle {
                return self.take_holder(RevokeReason::Released);
            }
        }
        None
    }

    fn take_holder(&mut self, reason: RevokeReason) -> Option<Revoked> {
        let h = self.holder.take()?;
        Some(Revoked { session: h.session, actor: h.actor, reason })
    }

    fn set_holder(&mut self, session: &str, actor: &Actor, now: Millis, hb: Duration) {
        self.holder = Some(Holder {
            session: session.to_string(),
            actor: actor.clone(),
            since: now,
            last_input: now,
            last_heartbeat: now,
            heartbeat_interval: hb,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HB: Duration = Duration::from_secs(30);
    const SEC: Millis = 1000;

    fn lease() -> ControlLease {
        ControlLease::new()
    }

    #[test]
    fn the_first_requester_of_a_free_lease_gets_it() {
        let mut l = lease();
        assert_eq!(l.acquire("s1", &Actor::agent("a"), 0, HB), Ok(Acquired::Granted));
        assert!(l.holds_write("s1"));
        assert_eq!(l.holder(), Some(&Actor::agent("a")));
    }

    #[test]
    fn re_acquiring_your_own_lease_is_a_no_op_not_an_error() {
        // A client that retries after a dropped response must not be punished
        // for holding what it already holds.
        let mut l = lease();
        l.acquire("s1", &Actor::agent("a"), 0, HB).unwrap();
        assert_eq!(l.acquire("s1", &Actor::agent("a"), SEC, HB), Ok(Acquired::AlreadyHeld));
        assert!(l.holds_write("s1"));
    }

    #[test]
    fn a_human_preempts_an_agent_immediately() {
        // The one allowed direction: intervention happens when the agent is
        // stuck or wrong, and a human cannot wait in a queue (§5.6).
        let mut l = lease();
        l.acquire("agent", &Actor::agent("a"), 0, HB).unwrap();

        let out = l.acquire("human", &Actor::human("kim"), SEC, HB).unwrap();
        match out {
            Acquired::Preempted { previous } => {
                assert_eq!(previous.session, "agent");
                assert_eq!(previous.reason, RevokeReason::Preempted);
            }
            other => panic!("expected preemption, got {other:?}"),
        }
        assert!(l.holds_write("human"));
        assert!(!l.holds_write("agent"), "the agent still holds WRITE after being preempted");
    }

    #[test]
    fn an_agent_cannot_preempt_a_human_and_is_told_to_wait() {
        // The forbidden direction. Pushing a human out would discard their
        // input, so it is rejected with a retry hint, never a preemption.
        let mut l = lease();
        l.acquire("human", &Actor::human("kim"), 0, HB).unwrap();

        let err = l.acquire("agent", &Actor::agent("a"), SEC, HB).unwrap_err();
        assert_eq!(err.holder, Actor::human("kim"));
        assert_eq!(err.retry_after, Duration::from_secs(30));
        assert!(l.holds_write("human"), "the human lost the lease to an agent");
    }

    #[test]
    fn an_agent_cannot_preempt_another_agent() {
        // Only humans may preempt; between agents it is first come, first
        // served, because breaking in-flight work leaves the screen unrecoverable.
        let mut l = lease();
        l.acquire("a1", &Actor::agent("a"), 0, HB).unwrap();
        assert!(l.acquire("a2", &Actor::agent("b"), SEC, HB).is_err());
        assert!(l.holds_write("a1"));
    }

    #[test]
    fn one_human_cannot_preempt_another() {
        // The no-queue policy applies between people too.
        let mut l = lease();
        l.acquire("h1", &Actor::human("kim"), 0, HB).unwrap();
        assert!(l.acquire("h2", &Actor::human("lee"), SEC, HB).is_err());
        assert!(l.holds_write("h1"));
    }

    #[test]
    fn a_humans_lease_passes_to_a_waiting_agent_only_after_the_idle_gap() {
        // 300 seconds, not on demand: at a minute the agent would seize the
        // keyboard mid-2FA while the person reads a text (S4).
        let mut l = lease();
        l.acquire("human", &Actor::human("kim"), 0, HB).unwrap();

        // An idle human is still connected — the viewer heartbeats every 30s
        // even while nobody types. Idle means no *input*, not no heartbeat, and
        // conflating them would reclaim the lease at 90s (three missed beats)
        // instead of holding it the full 300s the person is entitled to.
        let idle = u64::from(crate::limits::HUMAN_IDLE_HANDOVER_SEC) * 1000;
        for t in (0..=idle).step_by(20 * 1000usize) {
            l.heartbeat("human", t);
        }

        // One second short of the idle gap: still the human's, agent rejected.
        assert!(l.acquire("agent", &Actor::agent("a"), idle - 1, HB).is_err());
        assert!(l.holds_write("human"));

        // One second past it: the lease has idled free and the agent takes it.
        assert_eq!(l.acquire("agent", &Actor::agent("a"), idle + 1, HB), Ok(Acquired::Granted));
        assert!(l.holds_write("agent"));
    }

    #[test]
    fn a_human_typing_steadily_keeps_the_lease_past_the_idle_gap() {
        // The idle clock is reset by input, or a busy human would still be
        // handed off 300s after acquiring, mid-task.
        let mut l = lease();
        l.acquire("human", &Actor::human("kim"), 0, HB).unwrap();

        let idle = u64::from(crate::limits::HUMAN_IDLE_HANDOVER_SEC) * 1000;
        // Type every 100 seconds for well past the gap. Input counts as a
        // heartbeat too, so this also keeps the lease from expiring.
        let mut last = 0;
        for t in (0..idle * 2).step_by(100 * 1000usize) {
            l.mark_input("human", t);
            last = t;
        }
        // Just after the last keystroke, still short of a fresh idle gap.
        assert!(l.acquire("agent", &Actor::agent("a"), last + 1, HB).is_err());
        assert!(l.holds_write("human"));
    }

    #[test]
    fn an_agent_lease_never_idles_out() {
        // Only a human may take an agent's lease, and that is preemption. An
        // idle agent keeps it (its heartbeat keeps it alive), so another agent
        // still cannot barge in.
        let mut l = lease();
        l.acquire("a1", &Actor::agent("a"), 0, HB).unwrap();

        let long = u64::from(crate::limits::HUMAN_IDLE_HANDOVER_SEC) * 1000 * 3;
        // Keep it alive with heartbeats but send no input.
        for t in (0..long).step_by(20 * 1000usize) {
            l.heartbeat("a1", t);
        }
        assert!(l.acquire("a2", &Actor::agent("b"), long, HB).is_err());
        assert!(l.holds_write("a1"));
    }

    #[test]
    fn a_lease_is_reclaimed_after_three_missed_heartbeats() {
        let mut l = lease();
        l.acquire("a1", &Actor::agent("a"), 0, HB).unwrap();

        // Just short of three intervals — still held.
        let ttl = HB.as_millis() as u64 * u64::from(crate::limits::LEASE_MISSED_INTERVALS);
        assert!(l.reap(ttl - 1).is_none());
        assert!(l.holds_write("a1"));

        // At three intervals it lapses, and a caller polling reap learns so.
        let revoked = l.reap(ttl).expect("the lease was not reclaimed");
        assert_eq!(revoked.reason, RevokeReason::LeaseExpired);
        assert!(l.holder().is_none());
    }

    #[test]
    fn a_release_frees_the_lease_and_names_the_holder_for_key_release() {
        // Even a clean release must leave a clean input state for whoever is
        // next, so it returns the holder the guest has to release keys for.
        let mut l = lease();
        l.acquire("h1", &Actor::human("kim"), 0, HB).unwrap();

        let r = l.release("h1").expect("release returned nothing");
        assert_eq!(r.actor, Actor::human("kim"));
        assert_eq!(r.reason, RevokeReason::Released);
        assert!(l.holder().is_none());

        // A release by a non-holder is a harmless no-op.
        l.acquire("h2", &Actor::human("lee"), SEC, HB).unwrap();
        assert!(l.release("someone-else").is_none());
        assert!(l.holds_write("h2"));
    }

    #[test]
    fn a_freed_lease_is_granted_to_the_next_requester() {
        let mut l = lease();
        l.acquire("a1", &Actor::agent("a"), 0, HB).unwrap();
        l.release("a1");
        assert_eq!(l.acquire("a2", &Actor::agent("b"), SEC, HB), Ok(Acquired::Granted));
    }
}
