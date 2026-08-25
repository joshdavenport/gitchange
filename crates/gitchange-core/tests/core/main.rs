//! Core's integration suite: every assertion goes through the public
//! `Repo` surface against real temp repos, never engine internals
//! (ADR 0008).
//!
//! One test binary, split by concern so a change to one area reads one
//! file. Start from the module whose concern you are changing:
//!
//! | Module              | Concern                                              |
//! |---------------------|------------------------------------------------------|
//! | `support`           | the `RepoFixture` shim — shared by all                |
//! | `refresh`           | `discover`/`refresh()`, `Snapshot`'s head and log     |
//! | `read_only_refresh` | the read form: writes nothing, decides nothing        |
//! | `hunk_universe`     | the HEAD↔worktree ∪ HEAD↔index union, ○●◑ derivation  |
//! | `hunk_id`           | the hunk address: the hash, the `h` sigil, ordinals  |
//! | `matcher`           | membership records, assignment rules, dormancy        |
//! | `assign`            | explicit `Repo::assign_hunks` re-anchoring            |
//! | `changelists`       | changelist sync ops and state-file persistence        |
//! | `persistence`       | ADR 0002's file-level properties, not per-operation   |
//! | `lockfile`          | contention: fail-fast, and dead vs. live holders      |
//! | `staging`           | write-through stage/unstage against the live index    |
//! | `commit`            | commit from a temporary index, plus record aftermath  |
//! | `head_moves`        | tier-2 staleness under an external HEAD move          |
//! | `conflicts`         | unmerged-path quarantine and the commit guard         |
//! | `operations`        | the guard's remaining arms — rebase, cherry-pick, am  |
//! | `apply_corpus`      | the data-driven apply-correctness corpus              |
//! | `engine`            | real-fs notify wiring: the worktree half and `.git`   |
//! | `wire`              | the JSON dialect: envelopes, ordering, no advisories  |
//!
//! `matcher` and `commit` are directories, split again by mechanism —
//! start from the table in their `mod.rs`. Cargo only scans the top level
//! of `tests/`, so the nesting is still one binary.

mod support;

mod apply_corpus;
mod assign;
mod changelists;
mod commit;
mod conflicts;
mod engine;
mod head_moves;
mod hunk_id;
mod hunk_universe;
mod lockfile;
mod matcher;
mod operations;
mod persistence;
mod read_only_refresh;
mod refresh;
mod staging;
mod wire;
