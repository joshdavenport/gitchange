The CLI's pedagogy lives in the agent skill; design sessions keep deciding what it must and must not teach, so this section collects the obligations. The skill's spine is the escalation ladder (see Decisions).

- **The ADR 0015 contract opens the flow** — verify `unassigned` is active and switch there if not; never switch to a real changelist: with capture on, every mutation's internal persisting refresh claims other actors' unassigned work. `refresh` goes untaught, like the overrides
- **Assign-as-you-go, never batched** — immediate assignment keeps a file's unassigned pool near-empty, which is what makes the path-level sweep safe (escalation-ladder decision)
- **Add-then-commit with gitchange verbs** — `assign … --to X` → `add X` → `commit X -m …`; raw `git add` stays valid (ADR 0003)
- **Address composition** — the wire carries `id` and `offset`, never a composed address: the address is `<path>:<id>`, with `/<n>` appended exactly when `offset` is non-null. A bad composition fails loud as not-found
- **Addresses compose at the root** — printed paths are repo-relative everywhere; typed paths resolve against cwd. Work from the repo root (or pass `-C`) so copied addresses resolve verbatim
- **The index-entry unit** — a file presenting a whole-file hunk assigns as one unit: that hunk plus every text hunk in the file, the mode hunk excluded. A subset address refuses with the full member-ID list; the retry names them all. Derivable from `diff --json`: a `whole_file` hunk in `hunks` means the file's text hunks are members (#51 discussion; ADR 0009)
- **The staleness family** — three gitchange internals no agent knows without training, and bare "stale" is ambiguous between them (CONTEXT.md): staged-stale `◑` (the index holds a different version; `index_only: true` means the content exists _only_ in the index, so `add` would overwrite it), the `StaleHunk` advisory (the snapshot aged; re-read and retry), and dormant records (a matching outcome, not an error)
- **Cheap reads** — `--no-content` is the hot-loop inventory; full `diff --json`, scoped by path, is recovery and verification, and a hunk-ID scope (`diff <path>:<id>`) validates the address — a stale ID fails loud before the agent acts on aged content. Reads are side-effect-free: a glance never moves membership (#51 discussion; ADR 0005)
- **Receipts are the sole advisory delivery** — a mutation's receipt (stdout echo plus stderr `notice:` lines) carries every decision its internal refresh made, possibly beyond the verb's target; nothing replays it, so read both streams before moving on
- **Never teaches the overrides** — `--take-owned`, `--allow-unassigned`, and `--allow-foreign-head` exist for deliberate human-directed work, not the agent flow
- **Exit 3 is retry-the-same-command** — transient lock contention, already retried internally once; run the identical command again. Never remove the lockfile unless an exit-1 refusal names a dead process

### Audited, not gaps

- Reorder does not exist anywhere (user order is creation-append order — no parity debt until an op ships);
- discard/restore-worktree,
- undo,
- conflict resolution,
- dormant-record inspection

These exist in neither frontend (dormant visibility is a `status --json` candidate, not parity). Reverse parity: `--unassign` is reserved here while the TUI's assign popup could not target unassigned — resolved by #94.
