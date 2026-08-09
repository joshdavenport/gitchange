//! Commit mechanics (issue 28, ADR 0004): commit a changelist's staged
//! hunks from a temporary index, live index and worktree untouched, hooks
//! run natively — and the ADR 0012 record aftermath: consumed records
//! removed, surviving same-file records commuted, retained `◑` records
//! rewritten against the new HEAD, baseline stamped in the same locked
//! update so the external-move guard never fires on an own commit.
//!
//! Split by the step of the commit each test exercises. ADR 0009's
//! whole-file (binary) cases are the flavours of those same steps, so
//! each sits with the text case it mirrors rather than in a module of its
//! own.
//!
//! | Module       | Concern                                             |
//! |--------------|-----------------------------------------------------|
//! | `helpers`    | the fixtures and readers these modules share        |
//! | `payload`    | the confirmed payload, its counts, drift, the ops   |
//! | `temp_index` | what the commit writes, the hooks, the two refusals |
//! | `outcome`    | what a commit reports: short id, amend, edge shapes |
//! | `aftermath`  | ADR 0012's records after an own commit              |

mod helpers;

mod aftermath;
mod outcome;
mod payload;
mod temp_index;
