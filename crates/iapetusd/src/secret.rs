//! Resolving a `secret_ref` to a value for `secret.type` (PRD §9.3).
//!
//! The guest never stores secrets; it asks a resolver for a value at the moment
//! it types it. Where that value actually comes from is the Control Plane's
//! secret store (§8.3), reached over the §19.5 channel — the guest holds no API
//! authority of its own (§9.1). This trait is the boundary, so the dispatcher
//! can be tested against a fake and the real wiring can arrive without touching
//! the action path.
//!
//! The resolved value is a [`SecretValue`], the same redacting newtype the
//! store uses, so a value that leaks into a log through this crate is
//! impossible for the same reason it is in the store.

pub use iapetus_proto::secret::{SecretError, SecretValue};

/// Turns a `secret_ref` into the value to type.
///
/// The value never passes through the agent's context (§9.3) — the agent sent
/// only the ref — and the resolver must not log it either.
pub trait SecretResolver: Send + Sync {
    fn resolve(&self, secret_ref: &str) -> Result<SecretValue, SecretError>;
}
