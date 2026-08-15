//! Shared types for the Iapetus Control Plane and `iapetusd`.
//!
//! The `.proto` files under `proto/` are the single source of truth for the
//! wire format (PRD §19.1); this crate compiles them and adds the invariants
//! protobuf cannot express — identifier format (§8.2) and the caps that bound
//! every request (§8.2 "Caps").
//!
//! Both sides of the daemon channel depend on this crate so the schema cannot
//! drift between them.

pub mod id;
pub mod limits;

/// Types generated from `proto/iapetus/v1/*.proto`.
pub mod v1 {
    #![allow(clippy::doc_markdown)]
    tonic::include_proto!("iapetus.v1");
}

pub use id::{Id, IdError, IdKind};
