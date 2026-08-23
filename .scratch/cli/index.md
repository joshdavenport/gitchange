Governed by CONTEXT.md's Conventions: _where a gitchange concept has a git analog, borrow git's vocabulary for what users type_. Terms in **bold** are CONTEXT.md glossary entries.

## Commands

One section per command. The synopsis line is the grammar; each element below it carries a status. The clap tree in `crates/gitchange/src/main.rs` is the source of truth for what is built — markers here record status as of the last edit.

Legend: **[built]** shipped · **[reserved]** decided and settled, not yet built. `reserved` is an artefact of the #51 process (built/open/reserved) — no open items remain, so every non-built element here is decided. Do not carry the word `reserved` into ground truth, specs, or tickets; state built vs. to-build plainly.

Global flags precede the subcommand and bind to every command, bare invocation included; clap marks them `global`, so each command's `--help` lists them and each synopsis below writes them in position:

- `-C <dir>` **[reserved]** — git's global, borrowed with its short (the only spelling git has): run as if launched in `<dir>` — repo discovery starts there and cwd-relative path arguments resolve against it. Single occurrence; git's repeatable-composing form is not borrowed. A nonexistent `<dir>` refuses (exit `1`). jj's `-R` is redundant and not taken

Each synopsis line carries the command's type: _read-only_ runs a read-only refresh — writes nothing, never advises; _mutating_ writes state — where the op runs an internal persisting refresh, its **receipt** carries that refresh's advisories (see the reads-never-persist and text-receipts decisions), and a bare state write has no refresh, so its receipt is the echo plus any notice the write itself produces.

### Exit codes

`0` success · `1` operational error — the text names what to do before retrying · `2` usage error (clap) · `3` **[reserved]** transient lock contention — retry the same command unchanged

### Command Reference

[`gitchange`](01-gitchange.md)
[`gitchange status`](02-status.md)
[`gitchange switch`](03-switch.md)
[`gitchange refresh`](04-refresh.md)
[`gitchange changelist`](05-changelist.md)
[`gitchange assign`](06-assign.md)
[`gitchange add`](07-add.md)
[`gitchange unstage`](08-unstage.md)
[`gitchange commit`](10-commit.md)
[`gitchange diff`](11-diff.md)

## Decisions

[Op parity: capability is never the gap](decisions/op-parity.md)
[Paths: cwd-relative in, repo-relative out, literal always](decisions/paths-cwd-relative.md)
[Short flags are borrowed, never invented](decisions/short-flags-borrowed.md)
[CLI failures are loud and non-interactive (ADR 0015)](decisions/failures-loud-non-interactive.md)
[The hunk address is `<path>:<hunk-id>`](decisions/hunk-address.md)
[The escalation ladder is the agent workflow](decisions/escalation-ladder.md)
[Mutation receipts are text-only; `--json` stays a read affordance](decisions/mutation-text-receipts.md)
[Hunk addressing is verb-independent](decisions/hunk-addressing.md)
[Lock contention is transient: bounded retry, exit 3, PID-named lockfile](decisions/lock-contention-is-transient.md)

## Agent skill

[The skill and the escalation ladder](12-skill.md)
