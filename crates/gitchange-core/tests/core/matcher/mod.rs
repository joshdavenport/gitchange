//! Matcher behaviour (ticket 25, ADRs 0001/0002/0005): membership
//! records, assignment rules, dormancy — asserted through the public
//! `Repo::refresh()` on real temp repos (ADR 0008), never matcher
//! internals. Determinism makes these table-shaped: same records + same
//! diffs → same membership.
//!
//! Split by the mechanism each test exercises. ADR 0009's whole-file
//! (binary) cases are the flavours of those same mechanisms, so each sits
//! with the text case it mirrors rather than in a module of its own.
//!
//! | Module      | Concern                                              |
//! |-------------|------------------------------------------------------|
//! | `helpers`   | the fixtures and readers these modules share         |
//! | `anchors`   | tier 1 exact match, tier 2 overlap, the anchor shape |
//! | `dormancy`  | retention, exact-only revival, the 14-day prune      |
//! | `lifecycle` | changelist delete/rename/absence, state-file writes  |
//! | `capture`   | auto-capture to the active changelist and its notices|
//! | `renames`   | ADR 0011's delete-plus-add presentation              |

mod helpers;

mod anchors;
mod capture;
mod dormancy;
mod lifecycle;
mod renames;
