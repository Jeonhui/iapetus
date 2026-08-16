//! Secret storage (PRD §8.3 Secret, §9.3).
//!
//! A secret's value is **write-only**: it goes in at creation and comes out only
//! to `secret.type`, never through any read API (§8.3). Two things enforce that
//! here. The value is wrapped in [`SecretValue`], whose `Debug` prints
//! `[redacted]`, so a stray `{:?}` in a log or an error cannot leak it. And the
//! metadata accessor returns everything *except* the value, so there is no path
//! that hands it back to a caller by accident.
//!
//! A secret is bound to Desktops (§9.3). Scoped only to a project, any Desktop
//! in that project could type any secret, which would break the "Desktop is the
//! unit of credential trust" principle at the API level. `allowed_desktop_ids`
//! is checked on every resolution; leaving it empty permits the whole project,
//! and that is reported as a warning rather than silently allowed.

use std::collections::HashMap;

pub use iapetus_proto::secret::{SecretError, SecretValue};

#[derive(Debug, Clone)]
struct Secret {
    name: String,
    value: SecretValue,
    /// Empty means the whole project — a warning condition, not a denial (§9.3).
    allowed_desktop_ids: Vec<String>,
}

/// Metadata a read API may return — everything but the value (§8.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretMeta {
    pub secret_ref: String,
    pub name: String,
    /// Empty here is the §9.3 project-wide warning case.
    pub allowed_desktop_ids: Vec<String>,
}

#[derive(Default)]
pub struct SecretStore {
    secrets: HashMap<String, Secret>,
}

impl SecretStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores a secret. Returns whether it is project-wide (§9.3 warning).
    pub fn create(
        &mut self,
        secret_ref: impl Into<String>,
        name: impl Into<String>,
        value: SecretValue,
        allowed_desktop_ids: Vec<String>,
    ) -> bool {
        let project_wide = allowed_desktop_ids.is_empty();
        self.secrets.insert(
            secret_ref.into(),
            Secret { name: name.into(), value, allowed_desktop_ids },
        );
        project_wide
    }

    /// Resolves a secret's value for a Desktop, checking the binding (§9.3).
    ///
    /// This is the only path the value comes out of, and it exists solely for
    /// `secret.type`. The error names the ref and the desktop, never the value.
    pub fn value_for(&self, secret_ref: &str, desktop_id: &str) -> Result<&SecretValue, SecretError> {
        let secret = self
            .secrets
            .get(secret_ref)
            .ok_or_else(|| SecretError::Unknown(secret_ref.to_string()))?;
        // Empty allow-list permits the whole project (§9.3); otherwise the
        // Desktop must be named.
        if !secret.allowed_desktop_ids.is_empty()
            && !secret.allowed_desktop_ids.iter().any(|d| d == desktop_id)
        {
            return Err(SecretError::NotAllowedHere {
                secret_ref: secret_ref.to_string(),
                desktop_id: desktop_id.to_string(),
            });
        }
        Ok(&secret.value)
    }

    /// Metadata for a read API — no value (§8.3 "value is not readable").
    #[must_use]
    pub fn metadata(&self, secret_ref: &str) -> Option<SecretMeta> {
        self.secrets.get(secret_ref).map(|s| SecretMeta {
            secret_ref: secret_ref.to_string(),
            name: s.name.clone(),
            allowed_desktop_ids: s.allowed_desktop_ids.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_resolves_only_on_a_desktop_it_is_bound_to() {
        // §9.3: a secret is the unit of credential trust bound to Desktops, so
        // one project's ERP password must not be typeable on another Desktop.
        let mut store = SecretStore::new();
        let wide = store.create("sec_erp", "ERP", SecretValue::new("hunter2"), vec!["dsk_a".into()]);
        assert!(!wide, "a bound secret is not project-wide");

        assert_eq!(store.value_for("sec_erp", "dsk_a").unwrap().expose(), "hunter2");
        assert!(matches!(
            store.value_for("sec_erp", "dsk_b"),
            Err(SecretError::NotAllowedHere { .. })
        ));
    }

    #[test]
    fn an_unbound_secret_is_project_wide_and_flags_a_warning() {
        // §9.3: leaving allowed_desktop_ids unset permits the whole project and
        // must raise a warning rather than silently allowing it.
        let mut store = SecretStore::new();
        let wide = store.create("sec_any", "Any", SecretValue::new("v"), vec![]);
        assert!(wide, "an unbound secret must report the project-wide warning");
        assert!(store.value_for("sec_any", "dsk_anything").is_ok());
    }

    #[test]
    fn an_unknown_secret_is_an_error_naming_only_the_ref() {
        let store = SecretStore::new();
        let e = store.value_for("sec_missing", "dsk_a").unwrap_err();
        assert_eq!(e, SecretError::Unknown("sec_missing".into()));
    }

    #[test]
    fn metadata_never_carries_the_value() {
        // §8.3: the value is write-only. A read API returns name and binding,
        // and there is no accessor that returns the value except value_for.
        let mut store = SecretStore::new();
        store.create("sec_1", "Login", SecretValue::new("top-secret-pw"), vec!["dsk_a".into()]);
        let meta = store.metadata("sec_1").unwrap();
        assert_eq!(meta.name, "Login");
        assert_eq!(meta.allowed_desktop_ids, vec!["dsk_a".to_string()]);
        // The value is not on the metadata type at all — this is a compile-time
        // guarantee, asserted here as documentation.
        let debug = format!("{meta:?}");
        assert!(!debug.contains("top-secret-pw"), "the value leaked into metadata");
    }

    #[test]
    fn the_value_does_not_leak_through_debug() {
        // The newtype's whole job: a stray {:?} in a log must not print the
        // credential.
        let v = SecretValue::new("super-secret");
        assert_eq!(format!("{v:?}"), "SecretValue([redacted])");
        assert!(!format!("{v:?}").contains("super-secret"));

        // And nested inside a derived-Debug struct.
        #[derive(Debug)]
        struct Holder {
            _v: SecretValue,
        }
        let h = Holder { _v: SecretValue::new("nested-secret") };
        assert!(!format!("{h:?}").contains("nested-secret"));
    }
}
