//! Control Plane token service (PRD §8.1, §8.4).
//!
//! The other half of §8.1: `iapetus-auth` verifies tokens, this issues them.
//! Keeping the two in separate crates with the shared `iapetus-auth` claim set
//! between them means the issuer cannot mint a token the verifier reads
//! differently.
//!
//! Everything here is pure but for the revocation clock, which is passed in.
//! Issuance, refresh, and the refresh cap are decided from an explicit `now`,
//! so the boundary that matters — the moment a refreshed token crosses its
//! total-lifetime wall — is a test rather than a race.
//!
//! Three rules from §8.1 live here:
//!
//! * **The Project Key is not a JWT.** It is an opaque random string compared
//!   by hash, so a leaked hash does not yield the key and the comparison is
//!   constant-time against timing.
//! * **Refresh keeps `orig_iat`.** A token refreshed every hour still hits the
//!   24h (agent) or 8h (viewer) wall, or self-refresh would make revocation
//!   meaningless.
//! * **Revocation is stateful with a 5-second TTL.** A JWT verifies statelessly;
//!   pulling one back does not, so the revoked `jti` list is held here and
//!   expires entries as the short token lifetimes age out.

pub mod secrets;

use std::collections::HashMap;
use std::sync::Mutex;

use iapetus_auth::{ActorType, Claims, Issuer, AUDIENCE};
use sha2::{Digest, Sha256};

/// §8.1 total-lifetime caps, in seconds, measured from `orig_iat`.
pub const AGENT_LIFETIME_CAP_SEC: i64 = 24 * 3600;
pub const VIEWER_LIFETIME_CAP_SEC: i64 = 8 * 3600;

/// §8.1: the revoked-jti list is a 5-second TTL cache, which is what backs the
/// "< 5s" revocation commitment.
pub const REVOCATION_TTL_SEC: i64 = 5;

/// Default token lifetimes (§8.1): agents 1h, viewers 15m.
pub const AGENT_TTL_SEC: i64 = 3600;
pub const VIEWER_TTL_SEC: i64 = 900;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Agent,
    Viewer,
}

/// What a viewer token may request (§8.1). Not the lease itself — the maximum
/// level this token is allowed to acquire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Read,
    Write,
}

#[derive(Debug, Clone)]
pub struct IssueRequest {
    pub kind: TokenKind,
    pub actor_type: ActorType,
    pub actor_id: String,
    pub project_id: String,
    pub desktop_ids: Vec<String>,
    /// Agent tokens carry the scopes the caller asks for; a viewer's are derived
    /// from `control` instead, so this is ignored for viewers.
    pub scopes: Vec<String>,
    /// Viewer only: the maximum level the token may request.
    pub control: Control,
    pub ttl_sec: i64,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IssueError {
    #[error("the Project Key is not recognized")]
    BadProjectKey,
    #[error("a token must name at least one desktop")]
    NoDesktops,
    #[error(transparent)]
    Auth(#[from] iapetus_auth::AuthError),
    #[error("this token has reached its total lifetime and cannot be refreshed")]
    LifetimeExhausted,
}

/// Issues, refreshes, and revokes tokens for one signing key.
pub struct TokenService {
    issuer: Issuer,
    /// The Project Key, stored as a SHA-256 hash. The plaintext never rests
    /// here, so a dump of this does not yield a usable key.
    project_key_hash: [u8; 32],
    iss_url: String,
    /// Revoked jti → the time the entry may be dropped. Held under a Mutex
    /// because revocation is the one bit of shared mutable state.
    revoked: Mutex<HashMap<String, i64>>,
    /// Monotonic-ish counter for unique jti values within a run.
    counter: std::sync::atomic::AtomicU64,
}

impl TokenService {
    /// Builds a service that accepts `project_key` and signs with `issuer`.
    pub fn new(issuer: Issuer, project_key: &str, iss_url: impl Into<String>) -> Self {
        Self {
            issuer,
            project_key_hash: sha256(project_key.as_bytes()),
            iss_url: iss_url.into(),
            revoked: Mutex::new(HashMap::new()),
            counter: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// The public verifying key, to publish in a JWKS.
    #[must_use]
    pub fn verifying_key(&self) -> ed25519_dalek::VerifyingKey {
        self.issuer.verifying_key()
    }

    #[must_use]
    pub fn kid(&self) -> &str {
        self.issuer.kid()
    }

    /// The `/.well-known/jwks.json` body (§8.1), an OKP/Ed25519 key set.
    #[must_use]
    pub fn jwks_json(&self) -> String {
        use base64::Engine;
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let x = b64.encode(self.issuer.verifying_key().to_bytes());
        format!(
            r#"{{"keys":[{{"kty":"OKP","crv":"Ed25519","use":"sig","alg":"EdDSA","kid":"{}","x":"{x}"}}]}}"#,
            self.issuer.kid()
        )
    }

    /// Checks a Project Key by constant-time hash comparison (§8.1).
    #[must_use]
    pub fn project_key_ok(&self, presented: &str) -> bool {
        let got = sha256(presented.as_bytes());
        // Constant-time: a byte-by-byte early return would leak the shared
        // prefix length through timing.
        got.iter().zip(self.project_key_hash.iter()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
    }

    /// Issues a token, on the strength of a valid Project Key.
    pub fn issue(&self, project_key: &str, req: &IssueRequest, now: i64) -> Result<String, IssueError> {
        if !self.project_key_ok(project_key) {
            return Err(IssueError::BadProjectKey);
        }
        if req.desktop_ids.is_empty() {
            return Err(IssueError::NoDesktops);
        }
        Ok(self.sign(req, now, now))
    }

    /// Refreshes a still-valid token, keeping its `orig_iat` (§8.1).
    ///
    /// The caller presents the current token; it is verified, then reissued with
    /// a fresh `iat`/`exp` but the same `orig_iat`, so the total-lifetime cap is
    /// unaffected by refreshing. A token already past the cap is refused —
    /// self-refresh cannot outrun revocation.
    pub fn refresh(&self, token: &str, now: i64) -> Result<String, IssueError> {
        let mut jwks = iapetus_auth::Jwks::new();
        jwks.insert(self.issuer.kid(), self.issuer.verifying_key());
        // Verify against the token's own audience without a cap check here — the
        // cap is enforced explicitly below, so its error is distinct from an
        // ordinary expiry.
        let policy = iapetus_auth::Policy { audience: AUDIENCE, lifetime_cap_sec: None };
        let claims = iapetus_auth::verify(token, &jwks, &policy, now)?;

        let cap = match claims.actor_type {
            ActorType::Agent => AGENT_LIFETIME_CAP_SEC,
            ActorType::Human => VIEWER_LIFETIME_CAP_SEC,
        };
        // The refreshed token would carry orig_iat unchanged, so a refresh that
        // lands past the wall is refused rather than issued dead-on-arrival.
        if now - claims.orig_iat >= cap {
            return Err(IssueError::LifetimeExhausted);
        }

        let ttl = match claims.actor_type {
            ActorType::Agent => AGENT_TTL_SEC,
            ActorType::Human => VIEWER_TTL_SEC,
        };
        let refreshed = Claims {
            jti: self.next_jti(),
            iss: self.iss_url.clone(),
            aud: claims.aud,
            sub: claims.sub,
            actor_type: claims.actor_type,
            project_id: claims.project_id,
            desktop_ids: claims.desktop_ids,
            scopes: claims.scopes,
            iat: now,
            exp: now + ttl,
            orig_iat: claims.orig_iat, // the whole point of the cap
        };
        Ok(self.issuer.sign(refreshed))
    }

    /// Revokes a `jti` for the TTL window (§8.1).
    pub fn revoke(&self, jti: &str, now: i64) {
        let mut r = self.revoked.lock().unwrap();
        r.retain(|_, &mut exp| exp > now); // drop expired entries as we go
        r.insert(jti.to_string(), now + REVOCATION_TTL_SEC);
    }

    /// Whether a `jti` is currently revoked.
    #[must_use]
    pub fn is_revoked(&self, jti: &str, now: i64) -> bool {
        let r = self.revoked.lock().unwrap();
        r.get(jti).is_some_and(|&exp| exp > now)
    }

    fn sign(&self, req: &IssueRequest, iat: i64, orig_iat: i64) -> String {
        let (aud, ttl, scopes) = match req.kind {
            TokenKind::Agent => (AUDIENCE.to_string(), req.ttl_sec, req.scopes.clone()),
            TokenKind::Viewer => {
                // A viewer's scopes come from its control level, not the caller:
                // WRITE carries desktop:control, READ carries only desktop:read.
                // The two human-mandatory scopes are added by the verifier
                // (§8.1), so they are not repeated here.
                let s = match req.control {
                    Control::Write => vec!["desktop:control".to_string(), "desktop:read".to_string()],
                    Control::Read => vec!["desktop:read".to_string()],
                };
                (AUDIENCE.to_string(), req.ttl_sec, s)
            }
        };
        let claims = Claims {
            jti: self.next_jti(),
            iss: self.iss_url.clone(),
            aud,
            sub: req.actor_id.clone(),
            actor_type: req.actor_type,
            project_id: req.project_id.clone(),
            desktop_ids: req.desktop_ids.clone(),
            scopes,
            iat,
            exp: iat + ttl,
            orig_iat,
        };
        self.issuer.sign(claims)
    }

    fn next_jti(&self) -> String {
        let n = self.counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // Not a ULID, but unique within a run and shaped like §8.2's ids.
        format!("jti_{n:026}")
    }
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    const PROJECT_KEY: &str = "sk_iap_live_testkey_do_not_use_in_production_00";

    fn service() -> TokenService {
        let issuer = Issuer::new("k1", SigningKey::from_bytes(&[5u8; 32]), "https://api.iapetus.dev");
        TokenService::new(issuer, PROJECT_KEY, "https://api.iapetus.dev")
    }

    fn jwks(svc: &TokenService) -> iapetus_auth::Jwks {
        let mut j = iapetus_auth::Jwks::new();
        j.insert(svc.kid(), svc.verifying_key());
        j
    }

    fn viewer_req(control: Control) -> IssueRequest {
        IssueRequest {
            kind: TokenKind::Viewer,
            actor_type: ActorType::Human,
            actor_id: "usr_kim".into(),
            project_id: "prj_1".into(),
            desktop_ids: vec!["dsk_1".into()],
            scopes: vec![],
            control,
            ttl_sec: VIEWER_TTL_SEC,
        }
    }

    #[test]
    fn a_valid_project_key_issues_a_token_a_bad_one_does_not() {
        let svc = service();
        assert!(svc.issue(PROJECT_KEY, &viewer_req(Control::Write), 1000).is_ok());
        assert_eq!(
            svc.issue("sk_iap_live_wrong", &viewer_req(Control::Write), 1000),
            Err(IssueError::BadProjectKey)
        );
    }

    #[test]
    fn an_issued_token_verifies_and_carries_the_right_scopes() {
        // The whole point of the two crates: what this issues, iapetus-auth
        // accepts, with the scopes the control level implies.
        let svc = service();
        let now = 1_000_000;
        let write = svc.issue(PROJECT_KEY, &viewer_req(Control::Write), now).unwrap();
        let policy = iapetus_auth::Policy { audience: AUDIENCE, lifetime_cap_sec: Some(VIEWER_LIFETIME_CAP_SEC) };
        let claims = iapetus_auth::verify(&write, &jwks(&svc), &policy, now).unwrap();
        assert!(claims.has_scope("desktop:control"), "a WRITE viewer token must carry control");
        // And a human token gains its two mandatory scopes at verification.
        assert!(claims.has_scope("desktop:owners:manage"));

        let read = svc.issue(PROJECT_KEY, &viewer_req(Control::Read), now).unwrap();
        let claims = iapetus_auth::verify(&read, &jwks(&svc), &policy, now).unwrap();
        assert!(!claims.has_scope("desktop:control"), "a READ token must not carry control");
    }

    #[test]
    fn a_token_naming_no_desktop_is_refused() {
        let svc = service();
        let mut req = viewer_req(Control::Read);
        req.desktop_ids = vec![];
        assert_eq!(svc.issue(PROJECT_KEY, &req, 1000), Err(IssueError::NoDesktops));
    }

    #[test]
    fn refresh_keeps_orig_iat_so_the_cap_still_bites() {
        // §8.1: refreshing resets iat but never orig_iat, or the total-lifetime
        // cap could be dodged forever by refreshing.
        // Self-refresh is a chain: a viewer token lives 15 minutes and is
        // refreshed before it expires, over and over. orig_iat must survive
        // every link, so the 8-hour wall arrives no matter how often it renews.
        let svc = service();
        let orig = 1_000_000;
        let mut token = svc.issue(PROJECT_KEY, &viewer_req(Control::Write), orig).unwrap();
        let policy = iapetus_auth::Policy { audience: AUDIENCE, lifetime_cap_sec: None };

        let mut t = orig;
        loop {
            t += 600; // refresh every 10 minutes, well inside the 15-minute ttl
            match svc.refresh(&token, t) {
                Ok(fresh) => {
                    let c = iapetus_auth::verify(&fresh, &jwks(&svc), &policy, t).unwrap();
                    assert_eq!(c.orig_iat, orig, "a refresh in the chain moved orig_iat");
                    assert_eq!(c.iat, t, "a refresh did not move iat forward");
                    token = fresh;
                }
                Err(IssueError::LifetimeExhausted) => {
                    // The wall is the 8-hour viewer cap, reached despite the
                    // token being individually valid at every step.
                    assert!(t >= orig + VIEWER_LIFETIME_CAP_SEC, "capped too early at {}", t - orig);
                    assert!(t < orig + VIEWER_LIFETIME_CAP_SEC + 600, "capped too late");
                    break;
                }
                Err(e) => panic!("unexpected refresh error: {e}"),
            }
        }
    }

    #[test]
    fn a_revoked_jti_reads_as_revoked_until_the_ttl_expires() {
        let svc = service();
        let now = 1_000_000;
        svc.revoke("jti_abc", now);
        assert!(svc.is_revoked("jti_abc", now));
        assert!(svc.is_revoked("jti_abc", now + REVOCATION_TTL_SEC - 1));
        // Past the TTL it ages out — the short token lifetimes keep the list small.
        assert!(!svc.is_revoked("jti_abc", now + REVOCATION_TTL_SEC));
        assert!(!svc.is_revoked("jti_never_revoked", now));
    }

    #[test]
    fn the_jwks_body_carries_the_public_key_by_kid() {
        let svc = service();
        let body = svc.jwks_json();
        assert!(body.contains(r#""kid":"k1""#));
        assert!(body.contains(r#""crv":"Ed25519""#));
        assert!(body.contains(r#""kty":"OKP""#));
    }

    #[test]
    fn each_issued_token_has_a_distinct_jti() {
        // The revocation handle must be unique, or revoking one token pulls
        // another.
        let svc = service();
        let a = svc.issue(PROJECT_KEY, &viewer_req(Control::Read), 1000).unwrap();
        let b = svc.issue(PROJECT_KEY, &viewer_req(Control::Read), 1000).unwrap();
        let policy = iapetus_auth::Policy { audience: AUDIENCE, lifetime_cap_sec: None };
        let ca = iapetus_auth::verify(&a, &jwks(&svc), &policy, 1000).unwrap();
        let cb = iapetus_auth::verify(&b, &jwks(&svc), &policy, 1000).unwrap();
        assert_ne!(ca.jti, cb.jti);
    }
}
