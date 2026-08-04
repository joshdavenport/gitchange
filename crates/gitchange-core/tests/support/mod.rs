//! Shim onto the shared fixture crate. The builder itself lives in
//! `gitchange-test-support` (ADR 0006/0008, issue #59) so the TUI crate's
//! run-loop tests build repos through the same one; core's test files
//! keep saying `mod support; use support::RepoFixture;` and are otherwise
//! untouched.
//!
//! Each test binary compiles this module separately, so a re-export it
//! happens not to use is expected rather than dead.
#![allow(unused_imports)]

pub use gitchange_test_support::{NON_UTF8_PATH, RepoFixture};

#[cfg(unix)]
pub use gitchange_test_support::UnwritableOdb;
