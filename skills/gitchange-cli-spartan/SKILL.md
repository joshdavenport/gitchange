---
name: gitchange-cli-spartan
description: Drive gitchange from the command line in a shared working tree — assign hunks to a changelist, stage and commit it, recover from a refusal.
---

# gitchange in a shared tree

Other actors edit the same tree. Every command names its target; every
refusal is the next instruction. Run from the repo root or pass `-C <root>`.

## Flow

1. `gitchange status --json`; if `active` is not `null`, `gitchange switch
   unassigned`. Never switch to a real changelist — every mutation's refresh
   would claim other actors' unassigned hunks under it.
2. `gitchange changelist <intent-name>`. `already exists` → surface to the
   human, never adopt.
3. After each edit: `gitchange assign <path> --to <changelist>`. One command,
   zero reads. Assign as you go — a batch at the end is a contested path.
   Refused → ladder.
4. `gitchange add <changelist>` → `gitchange commit <changelist> -m "…"` →
   `gitchange changelist -d <changelist>`.
5. Foreign changes in `status --json` / `diff --json` are expected context.
   Assign into, commit, delete only your own changelist.

Done per mutation: receipt read on both streams, exit `0`.

## Escalation ladder

Climbs to ground truth, then the human — never around in retries.

1. `assign <path> --to <cl>` refuses → names the owner and the path.
2. `assign <path> --containing "<line you wrote>" --to <cl>`. One match
   moves. Zero/several → refusal lists candidate addresses; retry with them
   verbatim: `assign <path>:<id> <path>:<id> --to <cl>`. Match owned by
   another changelist → stop.
3. `diff --json <path>` — ground truth; address what is yours.
4. Human rung: a refusal naming another actor's live claim. Stop, surface it.
   A flag the refusal names is not the move; escalation is.

## Addresses

- `<path>:<id>`, plus `/<n>` exactly when `offset` is non-`null`. Wire
  carries `id` and `offset`, never the composition. Abbreviated (text) or full
  (JSON) IDs both resolve.
- Printed paths are repo-relative; typed paths resolve against cwd — hence
  root or `-C`.
- `diff <path>:<id>` validates before acting; an aged ID is not-found, and
  the refusal names the path to re-read. A missing `/<n>` is refused as
  ambiguous, candidates listed.

## Index-entry unit

A file with a `whole_file` hunk assigns as one unit: it plus every `text`
hunk in the file; `mode` excluded. Subset refused with the member list — name
them all, or sweep the path. Derivable from `diff --json`: a `whole_file`
hunk among a file's `hunks` means its `text` hunks are members.

## Staleness — three senses

- **Staged-stale** (`stage: "staged_stale"`, `◑`): index holds a different
  version. `add <cl> <path>:<id>` sets index := worktree. `index_only: true`
  → content exists only in the index; `add` discards it. Read `lines` first.
- **Stale action** (receipt: `skipped as stale` / `changed since the last
  refresh`): snapshot aged; that hunk skipped, the rest moved. Re-read, retry.
- **Dormant record** (a HEAD move's `notice:` that records went dormant): the
  hunk vanished from the diff, its record kept for revival. Matching outcome,
  not an error. Nothing to do.

## Reads

- Hot loop: `diff --json --no-content [<path>...]` — ids, owners, stage, no
  lines.
- Recovery/verification: `diff --json <path>`.
- Validation: `diff <path>:<id>`.
- Orientation: `status --json`.
- Reads never move membership.

## Receipts

stdout echo + stderr `notice:` lines = every decision the command's refresh
made, delivered once. Read both; branch on exit codes and re-read state via
JSON.

## Exit codes

`0` done · `1` refused, stderr is the instruction (remove the lockfile only
when the refusal names a dead process) · `2` usage · `3` contention, already
retried: run the identical command again.
