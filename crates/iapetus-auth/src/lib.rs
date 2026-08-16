//! Token signing and verification (PRD §8.1).
//!
//! The tokens that authenticate every principal — Agent, Viewer, Guest — are
//! Ed25519 JWTs. This crate is the one place the claim set, the signature
//! scheme, and the scope rules live, so the Control Plane that issues a token
//! and the gateway or guest that verifies it cannot disagree about what a token
//! means.
//!
//! Ed25519 rather than RS256 (§8.1): signing and verification are fast, the
//! keys are short, and it sidesteps the RSA padding-selection traps that turn
//! JWT libraries into vulnerabilities. The JWT is assembled by hand — three
//! base64url segments, a detached signature — rather than pulled from a
//! general-purpose library, because the general-purpose libraries are exactly
//! where those traps live, and the format is small enough to get right.
//!
//! Verification is stateless, as a JWT is meant to be. Revocation is not — it
//! needs the Control Plane's `jti` list — so this crate verifies signature,
//! expiry, audience, and scope, and exposes the `jti` for a caller that also
//! checks revocation. What a valid signature cannot tell you is whether the
//! token was pulled back, and that asymmetry is stated rather than hidden.

use std::collections::HashMap;

use base64::Engine;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// The audience an Agent or Viewer token is issued for (§8.1).
pub const AUDIENCE: &str = "iapetus";
/// The audience a Guest token carries, kept distinct so a Guest token cannot be
/// replayed as an Agent token — it authorizes only the §19.5 stream (§9.1).
pub const AUDIENCE_GUEST: &str = "iapetus-guest";

/// §8.1: a human token always carries these two, and the customer cannot
/// withhold them. Attached at verification so a forged token that omits them
/// gains nothing and a genuine one cannot be stripped of them.
pub const HUMAN_MANDATORY_SCOPES: [&str; 2] = ["desktop:owners:manage", "desktop:audit:read"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActorType {
    Agent,
    Human,
}

/// The full claim set (§8.1). Serialized as the JWT payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    /// The handle revocation operates on.
    pub jti: String,
    pub iss: String,
    pub aud: String,
    pub sub: String,
    pub actor_type: ActorType,
    #[serde(default)]
    pub project_id: String,
    #[serde(default)]
    pub desktop_ids: Vec<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
    pub iat: i64,
    pub exp: i64,
    /// Time of first issuance, never reset on refresh. Enforces the total
    /// lifetime cap that `iat` alone cannot (§8.1).
    pub orig_iat: i64,
}

impl Claims {
    /// The scopes this token grants, with a human's two mandatory scopes folded
    /// in. Callers must check authority through this, never the raw `scopes`,
    /// or a human token could be issued without the rights §8.1 guarantees.
    #[must_use]
    pub fn effective_scopes(&self) -> Vec<String> {
        let mut out = self.scopes.clone();
        if self.actor_type == ActorType::Human {
            for s in HUMAN_MANDATORY_SCOPES {
                if !out.iter().any(|x| x == s) {
                    out.push(s.to_string());
                }
            }
        }
        out
    }

    /// Whether the token grants `scope`.
    #[must_use]
    pub fn has_scope(&self, scope: &str) -> bool {
        self.effective_scopes().iter().any(|s| s == scope)
    }

    /// Whether the token is scoped to `desktop_id`.
    ///
    /// A token names the Desktops it may touch; one that names none reaches no
    /// Desktop, which is the safe direction for a malformed token (§8.1).
    #[must_use]
    pub fn covers_desktop(&self, desktop_id: &str) -> bool {
        self.desktop_ids.iter().any(|d| d == desktop_id)
    }

    /// Milliseconds of total lifetime used since first issuance.
    #[must_use]
    pub fn lifetime_used_sec(&self, now: i64) -> i64 {
        now - self.orig_iat
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Header {
    alg: String,
    typ: String,
    kid: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AuthError {
    #[error("token is not three dot-separated segments")]
    Malformed,
    #[error("base64 or JSON decode failed")]
    Decode,
    #[error("algorithm is {0}, expected EdDSA")]
    WrongAlgorithm(String),
    #[error("no key in the JWKS for kid `{0}`")]
    UnknownKey(String),
    #[error("signature does not verify")]
    BadSignature,
    #[error("token expired at {exp}, now {now}")]
    Expired { exp: i64, now: i64 },
    #[error("token not valid until {iat}, now {now}")]
    NotYetValid { iat: i64, now: i64 },
    #[error("audience is `{got}`, expected `{expected}`")]
    WrongAudience { got: String, expected: String },
    #[error("total lifetime cap of {cap_sec}s exceeded ({used_sec}s since first issuance)")]
    LifetimeCapExceeded { used_sec: i64, cap_sec: i64 },
}

/// A set of public keys, chosen by `kid` — the served `/.well-known/jwks.json`
/// in structure (§8.1). Rotation keeps the new and old key here in parallel.
#[derive(Default)]
pub struct Jwks {
    keys: HashMap<String, VerifyingKey>,
}

impl Jwks {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, kid: impl Into<String>, key: VerifyingKey) {
        self.keys.insert(kid.into(), key);
    }

    fn get(&self, kid: &str) -> Option<&VerifyingKey> {
        self.keys.get(kid)
    }
}

/// A signing key with the `kid` verifiers select it by. Held by the Control
/// Plane, never distributed.
pub struct Issuer {
    kid: String,
    key: SigningKey,
    iss: String,
}

impl Issuer {
    pub fn new(kid: impl Into<String>, key: SigningKey, iss: impl Into<String>) -> Self {
        Self { kid: kid.into(), key, iss: iss.into() }
    }

    /// The public half, to publish in a JWKS.
    #[must_use]
    pub fn verifying_key(&self) -> VerifyingKey {
        self.key.verifying_key()
    }

    #[must_use]
    pub fn kid(&self) -> &str {
        &self.kid
    }

    /// Signs `claims` into a compact JWT. `iss` on the claims is overwritten
    /// with the issuer's, so a caller cannot mint a token attributed elsewhere.
    pub fn sign(&self, mut claims: Claims) -> String {
        claims.iss = self.iss.clone();
        let header = Header {
            alg: "EdDSA".to_string(),
            typ: "JWT".to_string(),
            kid: self.kid.clone(),
        };
        let h = B64.encode(serde_json::to_vec(&header).expect("header serializes"));
        let p = B64.encode(serde_json::to_vec(&claims).expect("claims serialize"));
        let signing_input = format!("{h}.{p}");
        let sig = self.key.sign(signing_input.as_bytes());
        format!("{signing_input}.{}", B64.encode(sig.to_bytes()))
    }
}

/// What to check beyond the signature (§8.1).
pub struct Policy<'a> {
    /// The audience the token must carry — `AUDIENCE` or `AUDIENCE_GUEST`.
    pub audience: &'a str,
    /// The total-lifetime cap in seconds since `orig_iat`: 24h for agents, 8h
    /// for viewers (§8.1). `None` skips the check (a Guest token has its own).
    pub lifetime_cap_sec: Option<i64>,
}

/// Verifies a token's signature and claims against `jwks` at time `now`.
///
/// Does **not** check revocation — that needs the Control Plane's `jti` list.
/// The returned claims expose `jti` for a caller that layers that check on top.
pub fn verify(token: &str, jwks: &Jwks, policy: &Policy, now: i64) -> Result<Claims, AuthError> {
    let mut parts = token.split('.');
    let (h, p, s) = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(h), Some(p), Some(s), None) => (h, p, s),
        _ => return Err(AuthError::Malformed),
    };

    let header: Header = B64
        .decode(h)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or(AuthError::Decode)?;

    if header.alg != "EdDSA" {
        // Reject anything else up front — the `alg: none` and algorithm-confusion
        // attacks both begin with a verifier trusting the header's choice.
        return Err(AuthError::WrongAlgorithm(header.alg));
    }

    let key = jwks.get(&header.kid).ok_or_else(|| AuthError::UnknownKey(header.kid.clone()))?;

    // Verify the signature over the exact bytes that were signed, before
    // trusting a single claim.
    let signing_input = format!("{h}.{p}");
    let sig_bytes = B64.decode(s).map_err(|_| AuthError::Decode)?;
    let sig = Signature::from_slice(&sig_bytes).map_err(|_| AuthError::BadSignature)?;
    key.verify(signing_input.as_bytes(), &sig).map_err(|_| AuthError::BadSignature)?;

    let claims: Claims = B64
        .decode(p)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or(AuthError::Decode)?;

    if claims.aud != policy.audience {
        return Err(AuthError::WrongAudience {
            got: claims.aud.clone(),
            expected: policy.audience.to_string(),
        });
    }
    if now >= claims.exp {
        return Err(AuthError::Expired { exp: claims.exp, now });
    }
    // A little leeway backwards is not offered: iat in the future means clock
    // skew or a forged token, and a stream that fails closed on it is correct.
    if now < claims.iat {
        return Err(AuthError::NotYetValid { iat: claims.iat, now });
    }
    if let Some(cap) = policy.lifetime_cap_sec {
        let used = claims.lifetime_used_sec(now);
        if used >= cap {
            return Err(AuthError::LifetimeCapExceeded { used_sec: used, cap_sec: cap });
        }
    }

    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issuer() -> Issuer {
        // A fixed key so tests are deterministic and need no RNG.
        let key = SigningKey::from_bytes(&[7u8; 32]);
        Issuer::new("k1", key, "https://api.iapetus.dev")
    }

    fn jwks_for(iss: &Issuer) -> Jwks {
        let mut j = Jwks::new();
        j.insert(iss.kid(), iss.verifying_key());
        j
    }

    fn claims(now: i64) -> Claims {
        Claims {
            jti: "jti_1".into(),
            iss: "unset".into(),
            aud: AUDIENCE.into(),
            sub: "agent_1".into(),
            actor_type: ActorType::Agent,
            project_id: "prj_1".into(),
            desktop_ids: vec!["dsk_1".into()],
            scopes: vec!["desktop:control".into()],
            iat: now,
            exp: now + 3600,
            orig_iat: now,
        }
    }

    fn policy() -> Policy<'static> {
        Policy { audience: AUDIENCE, lifetime_cap_sec: Some(24 * 3600) }
    }

    #[test]
    fn a_freshly_signed_token_verifies_and_round_trips_its_claims() {
        let iss = issuer();
        let now = 1_000_000;
        let token = iss.sign(claims(now));
        let out = verify(&token, &jwks_for(&iss), &policy(), now).expect("verify");
        assert_eq!(out.sub, "agent_1");
        assert_eq!(out.jti, "jti_1", "the revocation handle must survive the round trip");
        assert_eq!(out.iss, "https://api.iapetus.dev", "iss is stamped by the issuer");
        assert!(out.has_scope("desktop:control"));
    }

    #[test]
    fn a_tampered_payload_fails_the_signature() {
        // The point of signing: flip one claim and the token is rejected, not
        // quietly trusted. This is the escalation a naive verifier allows.
        let iss = issuer();
        let now = 1_000_000;
        let token = iss.sign(claims(now));

        let mut parts: Vec<&str> = token.split('.').collect();
        let mut c = claims(now);
        c.scopes = vec!["desktop:admin".into()]; // grant yourself admin
        let forged = B64.encode(serde_json::to_vec(&c).unwrap());
        parts[1] = &forged;
        let tampered = parts.join(".");

        assert_eq!(
            verify(&tampered, &jwks_for(&iss), &policy(), now),
            Err(AuthError::BadSignature)
        );
    }

    #[test]
    fn a_token_signed_by_another_key_is_rejected() {
        let real = issuer();
        let attacker = Issuer::new("k1", SigningKey::from_bytes(&[9u8; 32]), "https://evil");
        let now = 1_000_000;
        // Same kid, different key: the JWKS holds the real key, so the
        // attacker's signature does not verify against it.
        let token = attacker.sign(claims(now));
        assert_eq!(
            verify(&token, &jwks_for(&real), &policy(), now),
            Err(AuthError::BadSignature)
        );
    }

    #[test]
    fn an_unknown_kid_is_rejected_rather_than_matched_against_any_key() {
        let iss = issuer();
        let mut jwks = Jwks::new();
        jwks.insert("k2", SigningKey::from_bytes(&[1u8; 32]).verifying_key());
        let token = iss.sign(claims(1_000_000));
        assert!(matches!(
            verify(&token, &jwks, &policy(), 1_000_000),
            Err(AuthError::UnknownKey(_))
        ));
    }

    #[test]
    fn the_none_algorithm_is_refused() {
        // alg:none is the classic JWT bypass: a token with no signature that a
        // lax verifier accepts. The header's algorithm choice is never trusted.
        let iss = issuer();
        let now = 1_000_000;
        let header = B64.encode(br#"{"alg":"none","typ":"JWT","kid":"k1"}"#);
        let payload = B64.encode(serde_json::to_vec(&claims(now)).unwrap());
        let token = format!("{header}.{payload}.");
        assert!(matches!(
            verify(&token, &jwks_for(&iss), &policy(), now),
            Err(AuthError::WrongAlgorithm(_))
        ));
    }

    #[test]
    fn an_expired_token_is_rejected() {
        let iss = issuer();
        let now = 1_000_000;
        let token = iss.sign(claims(now));
        // One second past exp.
        assert!(matches!(
            verify(&token, &jwks_for(&iss), &policy(), now + 3601),
            Err(AuthError::Expired { .. })
        ));
    }

    #[test]
    fn the_wrong_audience_is_rejected_so_a_guest_token_cannot_act_as_an_agent() {
        let iss = issuer();
        let now = 1_000_000;
        let mut c = claims(now);
        c.aud = AUDIENCE_GUEST.into(); // a guest token
        let token = iss.sign(c);
        // Verified against the agent audience: refused, so §9.1's "the guest
        // holds no API authority" cannot be bypassed by presenting its token.
        assert!(matches!(
            verify(&token, &jwks_for(&iss), &policy(), now),
            Err(AuthError::WrongAudience { .. })
        ));
    }

    #[test]
    fn the_lifetime_cap_is_measured_from_first_issuance_not_the_latest_refresh() {
        // §8.1: orig_iat, not iat. A token refreshed every hour must still hit
        // the 24h wall, or self-refresh makes revocation meaningless.
        let iss = issuer();
        let orig = 1_000_000;
        let mut c = claims(orig);
        c.orig_iat = orig;
        // Refreshed at 23.5h: fresh iat and a fresh hour of exp (running to
        // 24.5h), same orig_iat. This is the case the cap exists for — a token
        // whose exp is still in the future but whose total life is spent.
        c.iat = orig + 23 * 3600 + 1800;
        c.exp = c.iat + 3600; // valid until orig + 24.5h
        let token = iss.sign(c);

        // 23.5h in: fine, well inside both exp and the cap.
        assert!(verify(&token, &jwks_for(&iss), &policy(), orig + 23 * 3600 + 1801).is_ok());
        // 24h in: exp is still half an hour away, but the 24h cap is reached.
        assert!(matches!(
            verify(&token, &jwks_for(&iss), &policy(), orig + 24 * 3600),
            Err(AuthError::LifetimeCapExceeded { .. })
        ));
    }

    #[test]
    fn a_human_token_always_carries_the_two_mandatory_scopes() {
        // §8.1: the customer cannot withhold them, so they are attached at
        // verification rather than trusted to be present.
        let mut c = claims(1_000_000);
        c.actor_type = ActorType::Human;
        c.scopes = vec![]; // issued with nothing
        assert!(c.has_scope("desktop:owners:manage"), "a human lost owner management");
        assert!(c.has_scope("desktop:audit:read"), "a human lost audit read");

        // An agent gets no such gift.
        let a = claims(1_000_000);
        assert!(!a.has_scope("desktop:owners:manage"));
    }

    #[test]
    fn a_token_reaches_only_the_desktops_it_names() {
        let c = claims(1_000_000);
        assert!(c.covers_desktop("dsk_1"));
        assert!(!c.covers_desktop("dsk_other"), "a token reached a Desktop it was not scoped to");
    }

    #[test]
    fn a_malformed_token_is_rejected_not_panicked_on() {
        let iss = issuer();
        let j = jwks_for(&iss);
        assert_eq!(verify("not.a.jwt.at.all", &j, &policy(), 0), Err(AuthError::Malformed));
        assert_eq!(verify("onlyonepart", &j, &policy(), 0), Err(AuthError::Malformed));
        assert!(verify("bad.bad.bad", &j, &policy(), 0).is_err());
    }
}
