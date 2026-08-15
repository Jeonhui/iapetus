//! Resource identifiers, enforcing PRD §8.2.
//!
//! Protobuf carries these as plain `string`, so the invariants live here:
//! a known prefix, a 26-character Crockford Base32 ULID, case-sensitive,
//! and opaque — nothing outside this module may infer ordering or creation
//! time from an id.
//!
//! `actor.id` is deliberately **not** modelled here. Per §8.2 it is a value the
//! customer asserts, free-form UTF-8 up to 128 bytes, and Iapetus never parses
//! it. Giving it a typed id would imply a validation we do not perform.

use std::fmt;
use std::str::FromStr;

/// The prefix registry from PRD §8.2.
///
/// Adding a resource means adding it here; the table and the parser cannot
/// drift apart because both are generated from this one list.
macro_rules! id_kinds {
    ($( $variant:ident => $prefix:literal ),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum IdKind {
            $( $variant, )+
        }

        impl IdKind {
            pub const ALL: &'static [IdKind] = &[ $( IdKind::$variant, )+ ];

            pub const fn prefix(self) -> &'static str {
                match self {
                    $( IdKind::$variant => $prefix, )+
                }
            }

            pub fn from_prefix(p: &str) -> Option<Self> {
                match p {
                    $( $prefix => Some(IdKind::$variant), )+
                    _ => None,
                }
            }
        }
    };
}

id_kinds! {
    Desktop  => "dsk",
    Session  => "ses",
    Owner    => "own",
    Project  => "prj",
    Job      => "job",
    Secret   => "sec",
    Webhook  => "whk",
    Image    => "img",
    Event    => "evt",
    Delivery => "dlv",
    Screenshot => "sht",
    Request  => "req",
    Token    => "jti",
}

/// Every prefix is exactly three characters, which makes every identifier
/// exactly 30 characters (§8.2). That uniformity buys fixed-width database
/// columns, aligned log output, and a parser that can slice at a constant
/// offset. It is asserted against the registry in the tests, so the constant
/// and the table cannot drift apart.
pub const PREFIX_LEN: usize = 3;

/// Crockford Base32 excludes I, L, O, and U so they cannot be confused with
/// 1, 0, and V when a human reads an id out of a log.
const CROCKFORD: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
pub const ULID_LEN: usize = 26;

/// Every identifier is `PREFIX_LEN + 1 + ULID_LEN` characters (§8.2).
pub const ID_LEN: usize = PREFIX_LEN + 1 + ULID_LEN;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IdError {
    #[error("identifier is missing its `prefix_` separator")]
    NoSeparator,
    #[error("unknown prefix `{0}`")]
    UnknownPrefix(String),
    #[error("expected prefix `{expected}`, found `{found}`")]
    WrongKind { expected: &'static str, found: String },
    #[error("ULID body must be {ULID_LEN} characters, found {0}")]
    BadLength(usize),
    #[error("character `{0}` is not valid Crockford Base32")]
    BadCharacter(char),
}

/// An opaque, prefixed resource identifier.
///
/// Ordering is intentionally not derived. ULIDs happen to sort lexicographically
/// by creation time, and deriving `Ord` would quietly invite callers to depend
/// on that — which §8.2 forbids. Pagination uses server-issued cursors instead.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Id {
    kind: IdKind,
    body: String,
}

impl Id {
    pub fn kind(&self) -> IdKind {
        self.kind
    }

    /// Parses and requires a specific resource kind.
    ///
    /// Prefer this over [`FromStr`] wherever the expected kind is known, so a
    /// `ses_` arriving where a `dsk_` belongs fails at the boundary rather than
    /// deeper in.
    pub fn parse_as(s: &str, expected: IdKind) -> Result<Self, IdError> {
        let id: Id = s.parse()?;
        if id.kind != expected {
            return Err(IdError::WrongKind {
                expected: expected.prefix(),
                found: id.kind.prefix().to_string(),
            });
        }
        Ok(id)
    }
}

impl FromStr for Id {
    type Err = IdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (prefix, body) = s.split_once('_').ok_or(IdError::NoSeparator)?;
        let kind =
            IdKind::from_prefix(prefix).ok_or_else(|| IdError::UnknownPrefix(prefix.to_string()))?;

        if body.len() != ULID_LEN {
            return Err(IdError::BadLength(body.len()));
        }
        // Case-sensitive by §8.2: a lowercase body is rejected rather than
        // upcased, so two spellings of the "same" id can never both be valid.
        if let Some(c) = body.chars().find(|c| !CROCKFORD.contains(&(*c as u8))) {
            return Err(IdError::BadCharacter(c));
        }

        Ok(Id { kind, body: body.to_string() })
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}_{}", self.kind.prefix(), self.body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = "dsk_01H8XK4M2N7P9Q3R5S6T7V8W9X";

    #[test]
    fn parses_a_well_formed_identifier() {
        let id: Id = VALID.parse().unwrap();
        assert_eq!(id.kind(), IdKind::Desktop);
        assert_eq!(id.to_string(), VALID, "display must round-trip the input");
    }

    #[test]
    fn every_registered_prefix_round_trips() {
        for kind in IdKind::ALL {
            let s = format!("{}_01H8XK4M2N7P9Q3R5S6T7V8W9X", kind.prefix());
            let id: Id = s.parse().unwrap_or_else(|e| panic!("{s} failed: {e}"));
            assert_eq!(id.kind(), *kind);
        }
    }

    #[test]
    fn rejects_a_lowercase_body() {
        // §8.2 makes ids case-sensitive. Accepting this and upcasing it would
        // mean one resource had two valid spellings.
        let e = "dsk_01h8xk4m2n7p9q3r5s6t7v8w9x".parse::<Id>().unwrap_err();
        assert!(matches!(e, IdError::BadCharacter('h')));
    }

    #[test]
    fn rejects_crockford_excluded_letters() {
        for c in ['I', 'L', 'O', 'U'] {
            // 26 characters exactly, so the parser reaches the charset check
            // rather than failing on length first.
            let s = format!("dsk_{}1H8XK4M2N7P9Q3R5S6T7V8W9X", c);
            assert!(
                matches!(s.parse::<Id>(), Err(IdError::BadCharacter(_))),
                "{c} should be rejected: it is confusable in a log"
            );
        }
    }

    #[test]
    fn rejects_wrong_length() {
        assert_eq!("dsk_01H8XK".parse::<Id>().unwrap_err(), IdError::BadLength(6));
    }

    #[test]
    fn rejects_unknown_and_missing_prefix() {
        assert!(matches!(
            "xyz_01H8XK4M2N7P9Q3R5S6T7V8W9X".parse::<Id>(),
            Err(IdError::UnknownPrefix(_))
        ));
        assert_eq!("01H8XK4M2N7P9Q3R5S6T7V8W9X".parse::<Id>().unwrap_err(), IdError::NoSeparator);
    }

    #[test]
    fn parse_as_rejects_a_different_resource_kind() {
        // The failure this prevents: a session id reaching a Desktop endpoint
        // and only being caught after a lookup miss.
        let e = Id::parse_as("ses_01H8XK4M2N7P9Q3R5S6T7V8W9X", IdKind::Desktop).unwrap_err();
        assert_eq!(e, IdError::WrongKind { expected: "dsk", found: "ses".into() });
    }

    #[test]
    fn every_identifier_is_exactly_thirty_characters() {
        // §8.2 fixes the total at 30. That only holds while every prefix is
        // three characters, so the registry is checked rather than assumed —
        // a four-character prefix would silently make some ids 31.
        for kind in IdKind::ALL {
            assert_eq!(
                kind.prefix().len(),
                PREFIX_LEN,
                "prefix `{}` breaks the fixed 30-character identifier",
                kind.prefix()
            );
        }
        assert_eq!(PREFIX_LEN + 1 + ULID_LEN, 30);
    }
}
