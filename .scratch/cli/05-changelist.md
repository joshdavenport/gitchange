### gitchange changelist

`gitchange [-C <dir>] changelist [<name>] [(-d | --delete) <name>... [-f | --force]] [(-m | --move) <old> <new>]` — read-only when listing; mutating otherwise

- bare invocation **[reserved]** — lists changelists: one name per line, user order, `*` on the active one; `unassigned` appears only while it holds the marker (last, as `* unassigned`). Human-shaped only: `--json` and `--format` are usage errors (exit `2`) — machine listing stays `status --json`
- `<name>` creates **[reserved]** — never activates (ADR 0015; the TUI's `n` pairs its own switch, this command does not). Single name. A reserved (`unassigned`, `all`) or existing name refuses (exit `1`); an existing name is a refusal, not satisfied — in a shared tree a quiet "already exists" masks two actors colliding on one name
- `-d, --delete <name>...` **[reserved]** — variadic, all-or-nothing against one snapshot (`assign`'s model): any unknown name or guard-tripping changelist refuses the whole command (exit `1`), naming every offender, and nothing is deleted
- records guard **[reserved]** — a changelist holding any membership records, live or dormant, refuses deletion, naming the counts and the mechanism: deletion prunes the records (ADR 0016) and releases the hunks recordless, for the next persisting refresh to claim — into the active changelist, or into a split entry's established owner (the entry-unit rule, #106); with capture off they stay **unassigned**, where a path-sweep `assign` can take them. An empty changelist deletes ungated
- `-f, --force` / `-D` **[reserved]** — the named override for the records guard; `-f` and `-D` (sugar for `--delete --force`) are `git branch`'s spellings, borrowed with the grammar. Inert when the guard would not fire. The forced receipt carries a notice naming the release count and the mechanism — released recordless, claimed at the next persisting refresh — never a destination (see the mechanism-not-destination decision)
- active-delete notice **[reserved]** — deleting the active changelist proceeds and leaves **unassigned** active (capture off, core's rule); the receipt carries a notice saying so (a new core `Advisory` variant; core work, not built). A notice, not a refusal: the records guard already covers the dangerous case
- `-m, --move <old> <new>` **[reserved]** — renames, carrying the active marker and rewriting every membership record, live and dormant; nothing is lost, so no guard and no notice. Both arguments required: git's one-arg rename-the-active form is the ambient inference ADR 0015 forbids (usage error, exit `2`). `-m` with `-d` or `<name>` is a usage error. `<old> == <new>` is satisfied, not refused — nothing was decided
- rename refusals **[reserved]** — unknown `<old>` refuses (exit `1`), listing valid candidates; `<new>` validates like a create — a reserved or existing name refuses (exit `1`). No `-M`: rename onto an existing name refuses, naming the composition (`changelist -D <target>`, then `-m`)
- no alias **[reserved]** — no `cl`; the typed caller is an agent, which pays no typing cost. `stage` earned alias status as vocabulary, not abbreviation
- `OpOutcome` receipts **[reserved]** — create, delete, and rename return `OpOutcome`: bare state writes, no internal refresh (ADR 0005's persisting set is unchanged); echoes composed in core, notices ride `advisories`, and nothing-decided (`-m x x`) prints nothing, exit `0`. Core work, not built

## Decisions

[`changelist` is the noun command, borrowing `git branch`'s grammar wholesale](decisions/changelist-noun.md)
[Delete refuses while records exist; `--force` is the override](decisions/delete-refuses-while-records-exist.md)
[Rename borrows `-m`/`--move`; the one-arg form and `-M` do not cross](decisions/rename-borrows-m.md)
[Changelist ops return `OpOutcome`; receipts are core-composed, and none of them refreshes](decisions/changelist-ops-opoutcome.md)
[Reads never persist; the deciding refresh's receipt is the only advisory delivery (ADR 0005 amendment)](decisions/reads-never-persist.md)
