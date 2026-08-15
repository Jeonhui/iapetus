//! The caps and timeouts from PRD §8.2, as constants both sides share.
//!
//! These live in code rather than in each service's configuration because the
//! guest's buffers and the gateway's body limits are compiled against them. A
//! Control Plane that accepted a larger batch than `iapetusd` can hold would
//! fail at the far end of the call, where the error is least useful.
//!
//! §8.2 states the reasoning; this file states the numbers.

/// Maximum actions in a single `act` batch.
pub const ACT_MAX_ACTIONS: usize = 64;

/// Maximum characters in one `type` action.
pub const TYPE_MAX_CHARS: usize = 8_192;

/// Inline upload ceiling. Above this a presigned URL is mandatory.
pub const UPLOAD_INLINE_MAX_BYTES: u64 = 32 * 1024 * 1024;

/// Total upload ceiling, presigned included.
pub const UPLOAD_TOTAL_MAX_BYTES: u64 = 5 * 1024 * 1024 * 1024;

/// Per-stream truncation for `shell.exec`, applied to stdout and stderr each.
pub const SHELL_OUTPUT_MAX_BYTES: usize = 1024 * 1024;

/// A screenshot may be returned inline as base64 only below this size.
pub const SCREENSHOT_INLINE_MAX_BYTES: usize = 256 * 1024;

/// Maximum screenshot dimension on either axis.
pub const SCREENSHOT_MAX_DIMENSION: u32 = 4096;

/// Guest-side lists truncate here and set `truncated` (§8.2). The guest has no
/// stable snapshot to paginate over, so it caps rather than pages.
pub const GUEST_LIST_MAX_ENTRIES: usize = 1_000;

/// Concurrent sessions per Desktop.
///
/// The READ cap is deliberately above the six concurrent streams a host
/// supports (§12.4) so the two limits fail distinguishably: attaching succeeds
/// and `NO_STREAM_CAPACITY` names the real constraint.
pub const SESSIONS_MAX_WRITE: usize = 1;
pub const SESSIONS_MAX_READ: usize = 10;

/// Owners per Desktop, and webhooks per project.
pub const OWNERS_MAX_PER_DESKTOP: usize = 50;
pub const WEBHOOKS_MAX_PER_PROJECT: usize = 20;

/// Path length. Windows is the stricter of the two by default.
pub const PATH_MAX_LINUX: usize = 4_096;
pub const PATH_MAX_WINDOWS: usize = 260;

/// In-flight depth on one daemon stream (§19.5). Beyond this, HTTP/2 flow
/// control holds the sender back.
pub const DAEMON_INFLIGHT_DEPTH: usize = 8;

/// Timeouts in milliseconds: (default, maximum).
///
/// `act`'s timeout covers the **whole batch**, not each action (§8.2).
pub const TIMEOUT_ACT_MS: (u32, u32) = (30_000, 120_000);
pub const TIMEOUT_SHELL_MS: (u32, u32) = (30_000, 300_000);
pub const TIMEOUT_WAIT_FOR_MS: (u32, u32) = (10_000, 120_000);

/// Lease lifetime is derived, never stated twice.
///
/// §8.4 fixes it at three heartbeat intervals, matching §5.6's "reclaim after
/// three missed intervals". Computing it here keeps the two from drifting.
pub const HEARTBEAT_INTERVAL_DEFAULT_SEC: u32 = 30;
pub const LEASE_MISSED_INTERVALS: u32 = 3;

#[must_use]
pub const fn lease_ttl_sec(heartbeat_interval_sec: u32) -> u32 {
    heartbeat_interval_sec * LEASE_MISSED_INTERVALS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lease_ttl_is_three_heartbeats() {
        assert_eq!(lease_ttl_sec(HEARTBEAT_INTERVAL_DEFAULT_SEC), 90);
        assert_eq!(lease_ttl_sec(10), 30);
    }

    #[test]
    fn read_session_cap_exceeds_host_stream_capacity() {
        // §12.4 allows six concurrent observers per host. If the session cap
        // equalled it, a caller could not tell which limit it hit.
        const HOST_CONCURRENT_STREAMS: usize = 6;
        assert!(
            SESSIONS_MAX_READ > HOST_CONCURRENT_STREAMS,
            "session and stream caps must stay distinguishable (§8.2)"
        );
    }

    #[test]
    fn inline_upload_stays_below_the_total_ceiling() {
        assert!(UPLOAD_INLINE_MAX_BYTES < UPLOAD_TOTAL_MAX_BYTES);
    }

    #[test]
    fn defaults_never_exceed_their_maximums() {
        for (name, (default, max)) in [
            ("act", TIMEOUT_ACT_MS),
            ("shell", TIMEOUT_SHELL_MS),
            ("wait_for", TIMEOUT_WAIT_FOR_MS),
        ] {
            assert!(default <= max, "{name}: default {default} exceeds max {max}");
        }
    }
}
