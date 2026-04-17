//! Core types shared by every other Keplor crate.
//!
//! This crate intentionally has no runtime dependencies: it is the anchor of
//! the dependency graph.  Phase 1 fills it with `Event`, `Provider`,
//! `Usage`, `Cost`, and the library error enum.
