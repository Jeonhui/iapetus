//! The Iapetus guest daemon (PRD §19.5).
//!
//! Everything above `platform` is OS-agnostic and unit-testable; the unsafe FFI
//! for X11, DXGI, and SendInput is confined below it.

pub mod channel;
pub mod dispatch;
pub mod frame;
pub mod platform;
