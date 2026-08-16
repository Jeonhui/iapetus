//! Shared secret types (PRD §8.3, §9.3).
//!
//! Both the Control Plane's secret store and the guest's `secret.type` resolver
//! traffic in these, so they live in the shared crate rather than being defined
//! twice. The redacting `Debug` is the point: a secret's plaintext must not
//! reach a log or an error, and a newtype whose `Debug` prints `[redacted]`
//! makes a leak through `{:?}` impossible by construction rather than by
//! remembering not to.

/// A secret's plaintext, kept from leaking through `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretValue(String);

impl SecretValue {
    pub fn new(v: impl Into<String>) -> Self {
        Self(v.into())
    }

    /// The plaintext. Named `expose` so every call site reads as a deliberate
    /// crossing of the boundary, not an ordinary getter.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretValue([redacted])")
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SecretError {
    #[error("no secret `{0}`")]
    Unknown(String),
    #[error("secret `{secret_ref}` is not permitted on desktop `{desktop_id}`")]
    NotAllowedHere { secret_ref: String, desktop_id: String },
    #[error("no resolver could reach the secret store")]
    Unreachable,
}
