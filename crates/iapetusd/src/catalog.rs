//! The application catalog (PRD §5.5).
//!
//! An App is **a convenience shortcut, not a restriction** — anything absent
//! from the catalog still launches by path. So an unknown key is a "no such
//! shortcut" error, never an authority error, and nothing here is a security
//! control. §9.2's `restricted` mode is where allowlisting lives, and it is
//! enforced at the Desktop boundary rather than by this file.
//!
//! The on-disk shape matches §5.5's App resource so the guest file and the API
//! resource are one document rather than two that drift.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

/// Where the image ships its catalog. Absent is normal, not an error: a Desktop
/// with no catalog can still launch anything by path.
pub const DEFAULT_PATH: &str = "/etc/iapetus/apps.json";

#[derive(Debug, Clone, Deserialize)]
pub struct Launch {
    /// `exec` is the only type in v1. Kept so adding `uri` or `script` later
    /// does not change the shape of every entry.
    #[serde(rename = "type", default = "default_launch_type")]
    pub kind: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

fn default_launch_type() -> String {
    "exec".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct App {
    pub key: String,
    #[serde(default)]
    pub name: Option<String>,
    pub launch: Launch,
}

#[derive(Debug, Deserialize)]
struct CatalogFile {
    #[serde(default)]
    apps: Vec<App>,
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("reading {path}: {source}")]
    Read { path: String, source: std::io::Error },
    #[error("parsing {path}: {source}")]
    Parse { path: String, source: serde_json::Error },
    #[error("duplicate catalog key `{0}`")]
    DuplicateKey(String),
    #[error("app `{key}` declares launch type `{kind}`; only `exec` is supported in v1")]
    UnsupportedLaunch { key: String, kind: String },
}

#[derive(Debug, Default)]
pub struct Catalog {
    by_key: HashMap<String, App>,
}

impl Catalog {
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Loads the catalog, treating a missing file as an empty catalog.
    ///
    /// A malformed one is *not* treated that way. Silently degrading to empty
    /// would turn a typo in the image into "the key does not exist", and the
    /// operator would go looking for the wrong bug.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, CatalogError> {
        let path = path.as_ref();
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::empty()),
            Err(source) => {
                return Err(CatalogError::Read { path: path.display().to_string(), source })
            }
        };
        Self::parse(&text, &path.display().to_string())
    }

    pub fn parse(text: &str, origin: &str) -> Result<Self, CatalogError> {
        let file: CatalogFile = serde_json::from_str(text)
            .map_err(|source| CatalogError::Parse { path: origin.to_string(), source })?;

        let mut by_key = HashMap::with_capacity(file.apps.len());
        for app in file.apps {
            if app.launch.kind != "exec" {
                return Err(CatalogError::UnsupportedLaunch {
                    key: app.key,
                    kind: app.launch.kind,
                });
            }
            // A duplicate key means one of the two entries silently never
            // launches, and which one depends on file order.
            if by_key.contains_key(&app.key) {
                return Err(CatalogError::DuplicateKey(app.key));
            }
            by_key.insert(app.key.clone(), app);
        }
        Ok(Self { by_key })
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&App> {
        self.by_key.get(key)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "apps": [
        { "key": "chrome", "name": "Chromium",
          "launch": { "type": "exec", "command": "/usr/bin/chromium",
                      "args": ["--no-first-run"] } },
        { "key": "terminal",
          "launch": { "command": "/usr/bin/xterm" } }
      ]
    }"#;

    #[test]
    fn a_catalog_entry_resolves_to_its_command_and_arguments() {
        let c = Catalog::parse(SAMPLE, "test").unwrap();
        assert_eq!(c.len(), 2);
        let chrome = c.get("chrome").expect("chrome missing");
        assert_eq!(chrome.launch.command, "/usr/bin/chromium");
        assert_eq!(chrome.launch.args, vec!["--no-first-run"]);
        assert_eq!(chrome.name.as_deref(), Some("Chromium"));
    }

    #[test]
    fn launch_type_defaults_to_exec() {
        // The second entry omits it. Requiring the field would make every
        // hand-written catalog fail on a value that has only one legal setting.
        let c = Catalog::parse(SAMPLE, "test").unwrap();
        assert_eq!(c.get("terminal").unwrap().launch.kind, "exec");
    }

    #[test]
    fn an_unknown_key_is_simply_absent() {
        // §5.5: the catalog is a shortcut, not a restriction. Absence must not
        // read as denial, because the same program launches fine by path.
        let c = Catalog::parse(SAMPLE, "test").unwrap();
        assert!(c.get("nothing-like-this").is_none());
    }

    #[test]
    fn a_duplicate_key_is_rejected_rather_than_resolved_by_file_order() {
        let dup = r#"{"apps":[
          {"key":"a","launch":{"command":"/one"}},
          {"key":"a","launch":{"command":"/two"}}]}"#;
        assert!(matches!(Catalog::parse(dup, "t"), Err(CatalogError::DuplicateKey(_))));
    }

    #[test]
    fn an_unsupported_launch_type_is_named() {
        let bad = r#"{"apps":[{"key":"a","launch":{"type":"uri","command":"x"}}]}"#;
        assert!(matches!(Catalog::parse(bad, "t"), Err(CatalogError::UnsupportedLaunch { .. })));
    }

    #[test]
    fn a_missing_file_is_an_empty_catalog_but_a_malformed_one_is_an_error() {
        // The asymmetry is the point: no catalog is a normal Desktop, while a
        // catalog that does not parse is a broken image, and reporting the
        // second as the first sends the operator after the wrong bug.
        assert!(Catalog::load("/nonexistent/apps.json").unwrap().is_empty());
        assert!(matches!(Catalog::parse("{ not json", "t"), Err(CatalogError::Parse { .. })));
    }
}
