### gitchange unstage

`gitchange [-C <dir>] unstage <changelist> [<path>[:<hunk-id>]...] [--containing <line>]` — mutating

- verb **[reserved]** — explicit-direction unstage: index := HEAD for the scope's hunks. The TUI's `space` toggle cannot translate — decide-by-current-state is the ambient inference ADR 0015 forbids — so the direction is the verb. No alias: `unstage` is itself the vocabulary, `add`/`stage`'s symmetric twin
- grammar **[reserved]** — `add`'s mirror: `<changelist>` positional, required; `[<path>...]` narrows to the **file row**; `unassigned` is a legal scope; `all` refuses — a view, not a scope. `<path>:<hunk-id>` and `--containing <line>` speak the shared addressing grammar, rules verbatim from `assign`
- sweeps take `●` only **[reserved]** — a changelist or path sweep unstages the scope's `●` hunks; a `◑` hunk is kept, and the receipt names it with both resolutions — unstage it by address, or `add` then `unstage`. Core's bulk filter stands unchanged (see Decisions)
- addressed `◑` unstages **[reserved]** — an explicit `<path>:<hunk-id>` or `--containing` match on a `◑` hunk sets index := HEAD, discarding the staged version — index-only content included, ungated (see Decisions)
- refusals **[reserved]** — `add`'s rules verbatim: a nonexistent changelist, a changelist owning no hunks, or a named path with no hunks owned by that changelist (the refusal names who does own them) refuses (exit `1`), naming every offender; a fully `○` scope is satisfied, not refused — repeating an unstage is idempotent
- scope consistency guard **[reserved]** — an addressed hunk not owned by the named changelist refuses (exit `1`), naming the actual owner; an unassigned hunk unstages via `unstage unassigned <path>:<hunk-id>`
- staleness at apply **[reserved]** — fails soft per hunk, `assign`'s rule: exit `0` with a `StaleHunk` advisory per skipped hunk when at least one hunk was unstaged, exit `1` when none was; explicit IDs refuse at validation
- hidden `restore` correction **[reserved]** — a hidden subcommand, absent from help, that never executes (exit `2`): `restore --staged` corrects to `unstage`; bare `restore` says gitchange has no worktree restore and raw `git restore` stays valid

## Decisions

[`unstage` is the verb; git's analog becomes a hidden correction](decisions/unstage-is-the-verb.md)
[Unstage sweeps take `●` only; `◑` clears by address, ungated](decisions/unstage-sweeps.md)
