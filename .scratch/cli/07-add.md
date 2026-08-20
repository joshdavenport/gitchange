### gitchange add

`gitchange [-C <dir>] add <changelist> [<path>[:<hunk-id>]...] [--containing <line>]` (alias: `stage`) — mutating

- verb + `stage` alias **[reserved]** (ADR 0015) — stages, write-through to the real index (ADR 0003). Mirrors git's add-then-commit shape so agents keep one vocabulary; raw `git add` stays valid
- `<changelist>` positional, required **[reserved]** — the bare form stages the changelist's hunks (the TUI's `space` at changelist scope). No default: naming the target is ADR 0015's explicit-mode contract. `unassigned` is a legal scope (`space` on the unassigned group); `all` refuses — a view, not a scope
- `[<path>...]` narrowing **[reserved]** — each path narrows the scope to the **file row** (changelist, path): only hunks the changelist owns in the file move, so `add` can never touch another changelist's hunks and needs no `--take-owned` analog. Variadic; validated all-or-nothing against one snapshot, like `assign`
- `◑` included **[reserved]** — the scope's `○` and `◑` hunks alike go index := worktree: `space`'s stage direction, and `git add`'s meaning on a re-modified file
- refusals **[reserved]** — a nonexistent changelist, a changelist owning no hunks, or a named path with no hunks owned by that changelist (the refusal names who does own them) refuses (exit `1`), naming every offender; a fully `●` scope is satisfied, not refused — repeating an add is idempotent
- staleness at apply **[reserved]** — fails soft per hunk, `assign`'s rule: exit `0` with a `StaleHunk` advisory per skipped hunk when at least one hunk was staged, exit `1` when none was
- `<path>:<hunk-id>` and `--containing <line>` **[reserved]** — the shared addressing grammar (see Decisions), rules verbatim from `assign`: last-colon parse, ID-shaped suffix, path as consistency guard; `--containing` single-path, exclusive with a `:hunk-id` argument (exit `2`), exactly-one-match with candidate-ID lists. Explicit IDs refuse at validation when stale; sweeps fail soft at apply
- scope consistency guard **[reserved]** — an addressed hunk not owned by the named changelist refuses (exit `1`), naming the actual owner; an unassigned hunk stages via `add unassigned <path>:<hunk-id>`. `--containing` resolves over every hunk in the path's universe first, then checks the scope on the unique match

## Decisions

[`add` is staging - ADR 0015](decisions/add-is-staging.md)
[`add` is changelist-first: `add <changelist> [<path>...]`](decisions/add-is-changelist-first.md)
