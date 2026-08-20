### gitchange assign

`gitchange [-C <dir>] assign <path>[:<hunk-id>]... [--containing <line>] [--take-owned] (--to <changelist> | --unassign)` — mutating

- verb + `--to <changelist>` **[reserved]** (#41) — **assign** the path's hunks to a changelist
- `--unassign` **[reserved]** (#41) — convenience for `--to unassigned`; the explicit form stays valid. A recordless release, not a parking spot: with a real changelist active, the next persisting refresh claims the released hunks into it (ADR 0016); they stay unassigned only when capture is off
- multiple `<path>` arguments **[reserved]** — variadic, like `git add`; validated all-or-nothing against one snapshot: any refusing argument refuses the whole command (exit `1`), naming every offender, and nothing is assigned
- path resolution **[reserved]** — a `<path>` is cwd-relative, normalized to repo-relative; `./`, `../`, and absolute paths inside the repo are legal. After normalization the path is literal — no globs, no pathspec magic (see the paths decision). An argument escaping the repo refuses (exit `1`), all-or-nothing with the other offenders; a directory argument refuses, naming it as a directory and listing the changed files under it. Shared addressing grammar: `add`, `unstage`, and `diff`'s path scoping inherit verbatim
- `<path>:<hunk-id>` **[reserved]** — explicit **hunk ID** address: the last colon splits path from ID, the suffix must be ID-shaped, and the path is a consistency guard (an ID resolving to another file refuses). Mixes freely with whole-path arguments; one path may repeat with different IDs. A not-found, stale, or ambiguous ID is a refusal
- owned-path refusal **[reserved]** — a path holding hunks owned by another changelist refuses (exit `1`), naming the owner; **unassigned** hunks are not owned
- nothing-to-assign refusal **[reserved]** — a path with no hunks in the change universe (clean, or nonexistent) refuses; hunks already owned by the target are satisfied, not refused — repeating an assign is idempotent
- `--take-owned` **[reserved]** — the named override for the owned-path refusal: takes the owned hunks too. For deliberate reorganization; the agent skill never teaches it
- `--containing <line>` **[reserved]** — content-addressed hunk selection: a fixed-string substring match, verbatim bytes, against the content of changed lines only (`+` and `-` origins; origin chars, context lines, and no-newline markers excluded). Matches count hunks, not occurrences, over every hunk in the path's universe; ownership is checked after uniqueness. Exactly one match assigns; zero or several refuse, listing candidate hunk IDs — a degenerate hunk (whole-file or mode) is a natural zero-match, its ID in the list. Single value: repeating the flag, an empty value, or a value containing a newline is a usage error (exit `2`)
- unit-subset refusal **[reserved]** — an addressed set naming a proper subset of a file's **index-entry unit** refuses (exit `1`), listing every member's ID and both resolutions: name them all, or sweep the path. The retry is an ordinary multi-ID assign under the rules above — ownership, `--take-owned`, staleness, idempotency — with no new flag. A single-member unit (a plain binary) never trips it

Interactions:

- `--to` and `--unassign` are mutually exclusive (usage error)
- `--containing` with more than one path argument is a usage error (exit `2`)
- `--containing` with a `<path>:<hunk-id>` argument is a usage error (exit `2`) — one addressing mode per command
- with `--containing` or a `<path>:<hunk-id>` argument, the owned-path refusal narrows to the addressed hunk
- `--containing`'s unique match is the addressed set: landing on a unit member trips the unit-subset refusal, member IDs in the list
- staleness discovered at apply — reachable only by whole-path sweeps, since explicit IDs refuse at validation — fails soft per hunk: exit `0` with a `StaleHunk` advisory per skipped hunk when at least one hunk was assigned, exit `1` when none was

## Decisions

[`assign`, not `add` or `move`](decisions/assign-not-add.md)
[No `unassign` verb](decisions/no-unassign-verb.md)
[`assign` refuses owned paths; the guard is not a flag](decisions/assign-refuses-owned-paths.md)
[Multi-path `assign` validates all-or-nothing](decisions/multi-path-assign.md)
[Hunk-level selection on the CLI is content-first](decisions/hunk-level-selection.md)
[`--containing` matches by substring over changed lines; multi-match resolves by ID, not a flag](decisions/containing-by-substring.md)
[Single-hunk addressing defers to the index-entry unit by refusal, never by widening](decisions/single-hunk-addressing.md)
[Accepted: collisions surface at the second assign; merged hunks are joint](decisions/accepted-collisions-surface.md)
[Degenerate hunks travel as facts, not labels - amended #112](decisions/degenerate-hunks-travel-as-facts.md)
