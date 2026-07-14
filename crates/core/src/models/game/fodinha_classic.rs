//! Fodinha Classic game facade.
//!
//! Classic currently uses the shared Fodinha engine without additional rules.
//! Keeping this module as the public Classic boundary lets the shared engine
//! evolve without making Power depend on the Classic game type.

pub use super::fodinha_core::*;
