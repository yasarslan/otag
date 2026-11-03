//! The features otag builds on top of its panes and its adapters.
//!
//! Each one is a small model with its own rules, kept away from both the
//! terminal plumbing in `otag-core` and the file formats in `otag-agents`:
//! what is here is arithmetic and judgement, which is the part worth testing
//! on its own.
#![forbid(unsafe_code)]

pub mod handoff;
pub mod usage;
