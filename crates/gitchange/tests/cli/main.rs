//! The binary's integration suite: every assertion runs the real
//! executable as a child process and reads what a caller would — stdout,
//! stderr, and the 0/1/2/3 exit-code contract (ticket 13, grown to the
//! full published scheme by #136). Nothing here links gitchange-core, and
//! fixtures are built with the git CLI, so git2 stays a core-only
//! dependency (ADR 0006).
//!
//! One test binary, split by concern so a change to one area reads one
//! file. Start from the module whose concern you are changing:
//!
//! | Module       | Concern                                                 |
//! |--------------|---------------------------------------------------------|
//! | `support`    | the fixture shim — shared by all                         |
//! | `grammar`    | the clap tree: what parses, what stubs, what is usage    |
//! | `invocation` | reaching the command at all: `-C`, discovery, the TUI    |
//! | `status`     | `status`'s two faces — the text rows and the envelope    |
//! | `switch`     | `switch`'s receipt, its refusals, and the active marker  |
//! | `locking`    | lock contention at the binary seam: live, dead, unread   |
//! | `diff`       | `diff`'s scope resolution and annotated text face        |

mod support;

mod diff;
mod grammar;
mod invocation;
mod locking;
mod status;
mod switch;
