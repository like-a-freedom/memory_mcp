//! Single-flight activation helper.
//!
//! `activate_once` is the canonical helper used by `Pool`
//! when a slot's broadcast channel is empty. It runs the
//! provided future exactly once even if many subscribers
//! receive the resulting value, by completing the in-flight
//! broadcast and recording the generation.
//!
//! The actual implementation lives in `pool::Pool::acquire_or_wait`
//! because it owns the slot map. This module is reserved
//! for the future scheduler hook.

#[allow(dead_code)]
pub fn placeholder() {}
