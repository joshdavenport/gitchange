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
//! | `grammar`    | the clap tree: what parses, and what is a usage error    |
//! | `invocation` | reaching the command at all: `-C`, discovery, the TUI    |
//! | `status`     | `status`'s two faces — the text rows and the envelope    |
//! | `switch`     | the marker: the receipt, the refusals, the claim it skips |
//! | `refresh`    | the claim-now: the counted receipt, and its silences      |
//! | `locking`    | lock contention at the binary seam: live, dead, unread   |
//! | `diff`       | `diff`'s scope resolution and its two faces              |
//! | `add`        | `add`'s sweep, its refusals, and its receipt             |
//! | `unstage`    | the mirror: the `●`-only sweep and its kept-`◑` notices  |
//! | `assign`     | membership: the path sweep, the ownership guard, release |
//! | `changelist` | the noun command: the bare listing, and create           |
//! | `ladder`     | the escalation ladder: that the rungs chain              |

mod support;

mod add;
mod assign;
mod changelist;
mod commit;
mod diff;
mod grammar;
mod invocation;
mod ladder;
mod locking;
mod refresh;
mod status;
mod switch;
mod unstage;
