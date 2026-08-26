---
name: gitchange-cli-base
description: Drive gitchange in a shared working tree. Use when asked to use gitchange or when working in a tree where you know other actors are working.
---

> Note: We're currently dogfooding gitchange, the CLI tool for managing changelists in a repo, used for isolating your changes amongst any other agents ongoing changes. Tell the user if anything doesn't work or generally go as you expected.

# gitchange in a shared tree

Other actors — humans, other agents — edit the same working tree. gitchange
attributes hunks to changelists; it does not isolate anyone. Every command
names its target, and every refusal is the instruction for the next move.
Work from the repo root, or pass `-C <repo-root>` on every command.

## The flow

1. **Open on `unassigned`.** Run `gitchange status --json`. If `active` is
   not `null`, run `gitchange switch unassigned`. Capture is off while
   `unassigned` is active. Never switch to a real changelist: the refresh
   inside every hunk-moving command would claim other actors' unassigned
   hunks under that name.
2. **Create an intent-named changelist.** `gitchange changelist fix-login-race`
   — the name is what other actors read to learn what you are doing. `already
exists` is an error to surface to the human, never a changelist to adopt.
3. **Assign as you go.** After each edit, `gitchange assign <path> --to
<changelist>`. The happy path is one command and zero reads. Immediate
   assignment keeps a file's unassigned pool near-empty, which is what keeps
   the path-level sweep safe; a batch at the end is what a contested path
   looks like. A refusal means climb the escalation ladder below.
4. **Stage, then commit, with gitchange verbs.** `gitchange add <changelist>`
   stages everything the changelist owns; `gitchange commit <changelist> -m
"…"` commits what it has staged. Then `gitchange changelist -d
<changelist>`. One vocabulary throughout: `assign … --to` → `add` →
   `commit`.
5. **Read, don't wonder.** `status --json` and `diff --json` answer "what else
   is going on here". Foreign changes are expected context: the changelist
   names say what, even when they cannot say why. Assign into, commit, or
   delete only your own changelist.

Each mutation is done when its receipt is read on both streams and the exit
code is `0` — see Receipts and Exit codes.

## The escalation ladder

The recovery path for a contested path. Rungs climb toward ground truth and
then to the human, never around in retries.

1. `gitchange assign <path> --to <changelist>` refuses: the path holds hunks
   another changelist owns. The refusal names the owner and the contested
   path.
2. Narrow by a distinctive line you wrote: `gitchange assign <path>
--containing "<line>" --to <changelist>`. gitchange does the matching.
   Exactly one match moves. Zero or several matches refuse and list the
   candidate hunk IDs as composed addresses — the list _is_ the resolution:
   retry naming the ones that are yours, pasted verbatim, `gitchange assign
<path>:<id> <path>:<id> --to <changelist>`. If the one match is a hunk another
   changelist owns, the refusal names that owner: stop.
3. Re-read ground truth when the listed candidates are not enough to choose:
   `gitchange diff --json <path>`, then address the hunks that are yours.
4. **The human rung.** A refusal that names another actor's live claim — an
   owner on a hunk you meant to take, a changelist you did not create — is
   where you stop and surface it. A refusal may name a flag that would take
   the work anyway; the taught move at that juncture is escalation.

## Addresses

- **Composition.** The wire carries `id` and `offset`, never a composed
  address. The address is `<path>:<id>`, with `/<n>` appended exactly when
  `offset` is non-`null`. Text faces print an abbreviated ID; the JSON prints
  it in full; either form resolves.
- **Root.** Printed paths are repo-relative; typed paths resolve against the
  current directory — which is why the repo root, or `-C`, is what makes a
  copied address resolve verbatim.
- **Validation.** `gitchange diff <path>:<id>` before acting on a copied
  address. A hunk ID addresses one snapshot: an ID from an aged snapshot
  fails loud as not-found, and the refusal names the path to re-read. A
  mis-composed address fails loud too — a missing `/<n>` is refused as
  ambiguous, with the candidates listed.

## The index-entry unit

A file presenting a `whole_file` hunk (a binary, an empty file added or
deleted) assigns as one unit: that hunk plus every `text` hunk in the file;
the `mode` hunk is never in it. Addressing a subset refuses with the full
member list — the rule applying, not breakage; the retry names them all, or
sweeps the path. Derivable from `diff --json`: a `whole_file` hunk among a
file's `hunks` means its `text` hunks are members.

## The staleness family

Three senses, named apart. Bare "stale" means one of these:

- **Staged-stale** — a hunk with `stage: "staged_stale"` (`◑` in text): the
  index holds a different version of it; committing now commits content that
  is not what you see. `gitchange add <changelist> <path>:<id>` sets index :=
  worktree. `index_only: true` means the content now exists only in the
  index, so `add` discards it — read the hunk's `lines` first.
- **Stale action** — the receipt reports a hunk `skipped as stale` or
  `changed since the last refresh`: the snapshot aged between your read and
  your command. That hunk was skipped, its membership untouched; the rest of
  the scope moved. Re-read and retry.
- **Dormant record** — a hunk vanished from the diff (stash, external commit,
  rebase) and its membership record is kept for revival; a HEAD move reports
  it as a `notice:` that records went dormant. A matching outcome, not an
  error; nothing to do.

## Reads

- `gitchange diff --json --no-content [<path>...]` is the hot-loop inventory:
  every hunk's `id`, `offset`, `changelist`, `stage`, `index_only`, no lines.
- `gitchange diff --json <path>` is recovery and verification: the same
  document with `lines`.
- `gitchange diff <path>:<id>` validates an address.
- `gitchange status --json` is orientation: `active`, and every group's file
  rows.
- Reads are side-effect-free: a glance never moves membership.

## Receipts

A mutation's receipt is its one-time output: the echo on stdout (`assigned 2
hunks → 'fix-login-race'`) plus each advisory as a `notice:` line on stderr.
Together they carry every decision the command's internal refresh made,
possibly beyond the verb's target. Nothing replays a receipt; the records keep
the facts. Read both streams before moving on, and re-read state through the
JSON reads rather than parsing receipt prose.

## Exit codes

- `0` — done; read the receipt.
- `1` — refused; stderr is the instruction. The lockfile is removed only when
  this refusal names a process that is no longer running.
- `2` — usage; stderr names the grammar.
- `3` — contention, already retried internally: run the same command again,
  unchanged.
