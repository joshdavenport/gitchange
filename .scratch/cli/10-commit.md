### gitchange commit

`gitchange [-C <dir>] commit <changelist> (-m <message>... | -F <file> | --no-edit) [--amend] [-n | --no-verify] [--allow-unassigned] [--allow-staged-stale] [--allow-foreign-head]` — mutating

- verb **[reserved]** (ADR 0015) — commits the changelist's staged hunks via the ADR 0004 temp index
- operation-in-progress refusal **[reserved]** (ADR 0007) — commit refuses (exit `1`) while a git operation (merge, rebase, cherry-pick, revert, am) is in progress: the commit would conclude that operation with one changelist's payload. Already core-enforced (`repo.rs`), so the CLI inherits it unbuilt; listed so the reference names every commit guard
- foreign-content refusal **[reserved]** (ADR 0004, #106) — commit refuses (exit `1`) when the payload would draw content out of an index entry a second **holder** — unassigned included — also has content in: the entry commits whole (ADR 0009), so either side of a split refuses. The refusal names the holders and the one resolution — assign the file's hunks to one changelist (one op: the **index-entry unit** assigns together) and retry. No override flag: a hard refusal in every frontend, never a warn-and-confirm, so ADR 0015 maps no flag, and ADR 0004 rejected proceeding loudly. Checks before the empty-payload refusal; already core-enforced (`commit.rs`), so the CLI inherits it unbuilt
- `-m, --message <message>` **[reserved]** — `git commit`'s spelling with its `-m` grammar wholesale: the value takes any bytes the shell delivers (multiline works), and repeated `-m` concatenates as paragraphs
- `-F, --file <file>` **[reserved]** — git's spelling, `-` reads stdin (the `git commit -F - <<'EOF'` agent pattern); core already delivers every message to the shelled-out `git commit` via `-F`
- one message source **[reserved]** — every commit names exactly one: `-m` (repeatable, one source), `-F`, or `--no-edit` (amend only). None, or two together, is a usage error (exit `2`) — there is no editor, so no default exists to fall back on
- `-n, --no-verify` **[reserved]** — forwarded verbatim to the shelled-out `git commit` (ADR 0004); hooks otherwise run natively, and a hook rejection is exit `1` carrying git's stderr
- `--amend` **[reserved]** — ADR 0004's amend: the same temp-index path plus `--amend`, so the amended tip is HEAD's content plus the changelist's payload. Guarded by the foreign-head guard; requires a message source — `-m`/`-F` replace, `--no-edit` keeps
- `--no-edit` **[reserved]** — keeps HEAD's message (git's spelling; core omits `-F` and forwards the flag — core work, not built). Amend-only: without `--amend` it is a usage error (exit `2`)
- foreign-head guard **[reserved]** — `--amend` refuses (exit `1`) unless HEAD is the named changelist's own recorded gitchange commit; state records `{ oid, changelist }` at each commit's aftermath (core work, not built). Checks before the empty-payload refusal; an amend re-records, so amend-after-amend passes; rename rewrites the record; no record yet refuses. CLI-only — the TUI stays gate-free
- `--allow-foreign-head` **[reserved]** — the named override for the foreign-head guard, inert when the guard would not fire
- amend needs a payload **[reserved]** — reword stays git's job: `--amend` with an empty payload refuses (exit `1`), naming both resolutions — `add <changelist>` and retry, or raw `git commit --amend`
- strict non-interactive defaults **[reserved]** (ADR 0015) — staged-stale hunks refuse with an error naming them; empty payload refuses; each TUI warn-and-confirm becomes a named override flag
- `--allow-staged-stale` **[reserved]** — the named override for the staged-stale refusal: `◑` hunks ship as the index holds them. Without it the refusal (exit `1`) names each `◑` hunk as `<path>:<hunk-id>` and both resolutions — `add <changelist> [<path>…]` to align (index := worktree), or this flag. Inert when the payload holds no `◑`
- empty-payload refusal **[reserved]** — no stage-all flag: the refusal (exit `1`) names `add <changelist>` as the resolution. An empty payload implies no `◑` in the scope (ADR 0004, #90), so this refusal and the staged-stale one never co-occur
- no drift guard **[reserved]** — the CLI passes core's `commit` no expected payload: drift is a confirm-flow concept, and the CLI has no confirm step — the synchronous refresh inside commit is the snapshot that ships
- `unassigned` as a commit target **[reserved]** — legal, gated: bare `commit unassigned` refuses (exit `1`), naming both resolutions — assign the hunks to a changelist and commit that, or pass `--allow-unassigned`. With the flag it commits the unassigned scope under the same rules as any changelist. The gate checks before the empty-payload refusal
- `--allow-unassigned` **[reserved]** — the named override for the unassigned-target refusal, `git commit --allow-empty`'s shape: it names the normally-refused condition and is inert when the condition is absent (legal, no-op with a named changelist)
- `all` as a commit target **[reserved]** — refuses (exit `1`), `add`/`unstage`/`diff`'s family and their code: core has no all-scope commit and the TUI's `c` on the **All** view is a no-op (ADR 0004: one commit mechanism; `c` never grows a second meaning), so there is no multi-commit loop to expose and op parity holds with nothing owed. The refusal speaks plainly — `all` is not a valid changelist — never the view/scope terminology, and names the resolution: commit one changelist by name. Target validation, so it checks before every payload guard. No override flag: nothing exists to allow

## Decisions

[Forecasts name the mechanism, never the destination (#106)](decisions/forecasts-name-the-mechanism.md)
[`commit unassigned` is legal behind `--allow-unassigned`](decisions/commit-unassigned.md)
[Commit's two confirms: one becomes a flag, the other composes](decisions/commits-confirms.md)
[Amend carries an attribution guard; reword stays git's job](decisions/amend-carries-attribution-guard.md)
[One message source; git's `-m`/`-F` grammar arrives wholesale](decisions/gits-m-uppercase-f.md)
