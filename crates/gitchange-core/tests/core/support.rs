//! Shim onto the shared fixture crate. The builder itself lives in
//! `gitchange-test-support` (ADR 0006/0008, issue #59) so the TUI crate's
//! run-loop tests build repos through the same one; core's test modules
//! say `use crate::support::RepoFixture;` and are otherwise untouched.

pub use gitchange_test_support::{NON_UTF8_PATH, RepoFixture};
