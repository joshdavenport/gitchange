---
name: gitchange-cli-verbose
description: Drive gitchange in a shared working tree. Use when asked to use gitchange or when working in a tree where you know other actors are working.
---

# gitchange in a shared tree

gitchange organises the uncommitted changes of one working tree into named
**changelists**, hunk by hunk: two hunks of one file can belong to two
changelists, and each changelist stages and commits independently. Other
actors — humans in the TUI, other agents — edit the same tree at the same
time. gitchange **attributes** their hunks and yours; it does not isolate
anyone, and nothing in the filesystem says who made an edit. That is why the
contract for an agent is: never rely on ambient state; every command names its
target. When a command refuses, the refusal's text is the instruction for the
next move — you navigate by error text, not by this document.

Work from the repo root, or pass `-C <repo-root>` on every command; the
Addresses section says why.

## The flow

Execute these in order at the start of a session, then keep the rhythm of
step 3 while you work.

1. **Open on `unassigned`.** Run `gitchange status --json` and read `active`.
   If it is not `null`, run `gitchange switch unassigned`. The **active
   changelist** is the one that _captures_: every hunk-moving command (`assign`,
   `add`, `unstage`, `commit`) first runs a refresh, and that refresh claims
   every recordless hunk in the tree — anyone's — for the active changelist.
   With `unassigned` active, capture is off and the refresh claims nothing.
   Never switch to a real changelist: while it is active, your next mutation
   would route other actors' unassigned work into it, silently, in a process
   they never ran.
2. **Create an intent-named changelist.** `gitchange changelist fix-login-race`
   — a name that says what the work is, never a session id. The name is the
   broadcast channel: it is what other actors see in the All view while you
   work. If the create refuses with `already exists`, that is a coordination
   smell — someone is already on this, or a previous run left it behind.
   Surface it to the human rather than adopting the changelist: two actors
   sharing one changelist is the attribution race again, at changelist scope.
3. **Assign as you go.** After each edit, `gitchange assign <path> --to
<changelist>`. This is the happy path — one command, zero reads — and it
   succeeds whenever the path's hunks are unassigned or already yours. Assign
   _immediately_, never in a batch at the end: a path assign is a **membership
   sweep**, it takes every hunk in the file, and it stays safe only while a
   file's unassigned pool is near-empty — which immediate assignment is what
   keeps true. The longer hunks sit unassigned, the more other actors' hunks
   sit beside yours in the same pool, and the more often the sweep refuses. A
   refusal here means climb the escalation ladder below.
4. **Stage, then commit, with gitchange verbs.** `gitchange add <changelist>`
   stages every hunk the changelist owns; `gitchange commit
<changelist> -m "…"` commits what it has staged and nothing else. Then
   `gitchange changelist -d <changelist>`. One vocabulary throughout:
   `assign … --to` → `add` → `commit`, the add-then-commit shape you know
   from git, with gitchange verbs.
5. **Read, don't wonder.** `gitchange status --json` and `gitchange diff
--json` answer "what else is going on here". Foreign hunks and foreign
   changelists are expected context in a shared tree, not confusion: the
   changelist names say what is happening even when they cannot say why.
   Assign into, commit, or delete only the changelist you created.

A mutation is done when its receipt has been read on both streams and its
exit code is `0` — see Receipts and Exit codes below.

## The escalation ladder

The recovery path for a **contested path** — one where another changelist owns
some of the hunks. The rungs climb toward ground truth and then to the human;
they never go around in retries. Each refusal's text is the instruction for
the next rung, so read it rather than reasoning about what it might mean.

1. **The path sweep refuses.** `gitchange assign <path> --to <changelist>`
   exits `1` with the owner and the contested path named — for example
   `'src/daemon.rs' holds hunks owned by 'session-rework'`. The whole path is
   named because the whole path is the scope; the evident move is to narrow.
2. **Narrow by a line you wrote.** `gitchange assign <path> --containing
"<distinctive line>" --to <changelist>`. gitchange matches the text
   against each hunk's changed lines and does the diffing for you. Exactly
   one match moves. Zero or several matches refuse, and the refusal lists
   candidate hunk IDs as composed addresses — that list _is_ the resolution:
   retry naming the ones that are yours, pasted verbatim, `gitchange assign
<path>:<id> <path>:<id> --to <changelist>`. If the one match is a hunk
   another changelist owns, the refusal names that owner: that is the human
   rung.
3. **Re-read ground truth.** When the candidates alone do not tell you which
   hunks are yours, `gitchange diff --json <path>` gives every hunk with its
   `changelist`, `id`, `offset`, and `lines`. Compose the addresses of the
   hunks you wrote and retry the assign with them.
4. **The human rung.** A refusal that names another actor's live claim — an
   owner on a hunk you meant to take, a changelist you did not create — is
   where the ladder ends. Stop and surface it to the human, with the refusal
   text. A refusal may name a flag that would take the work anyway; the
   taught move at that juncture is escalation.

## Addresses

Every hunk-addressing verb speaks the same address, and every address comes
from something gitchange printed.

- **Composition.** The JSON carries `id` and `offset` as two fields, never a
  composed address; composing is yours. The address is `<path>:<id>`, with
  `/<n>` appended exactly when `offset` is non-`null` — identical hunks in one
  file share a base ID, and the ordinal tells them apart. Text faces
  (`diff`, refusals) print an abbreviated ID; the JSON prints it in full;
  either resolves, and pasting what was printed is always correct.
- **Root.** Every path gitchange prints is repo-relative; every path you type
  resolves against the current directory — which is why the repo root, or
  `-C <repo-root>`, is what makes a copied address resolve verbatim.
- **Validation.** A hunk ID is an address into one snapshot, not a durable
  identity: when the hunk's content changes, the ID changes. `gitchange diff
<path>:<id>` before acting on a copied address; an ID from an aged snapshot
  fails loud as not-found, and the refusal names the path to re-read. A
  mis-composed address fails loud too: a missing `/<n>` is refused as
  ambiguous with the candidates listed. Nothing ever acts on an aged address
  by answering for whatever took its place.

## The index-entry unit

A file with no line-addressable content — a changed binary, an empty file
added or deleted — presents a `whole_file` hunk. Because that hunk and any
`text` hunks in the same file share one index entry, and committing the entry
commits it whole, they assign as one **index-entry unit**: the `whole_file`
hunk plus every `text` hunk in the file. The `mode` hunk is never in the unit;
it is independently stageable.

Addressing a subset of the unit refuses with the full member list. That is the
rule applying, not breakage: the retry names every member, or sweeps the path.
You can see a unit coming from `diff --json`: a `whole_file` hunk among a
file's `hunks` means its `text` hunks are members.

## The staleness family

Three distinct conditions, each with its own move. When you meet the word
"stale" on its own, work out which of these it is.

- **Staged-stale.** A hunk with `stage: "staged_stale"` (`◑` in text output):
  the index holds an overlapping-but-different version of it — staged then
  edited, or staged then reverted in the worktree. Committing now would
  commit content that is not what you see. `gitchange add <changelist>
<path>:<id>` sets index := worktree. When `index_only` is `true`, the
  content exists only in the index — the worktree has reverted — so `add`
  discards it; read the hunk's `lines` first to decide whether that is what
  you want.
- **Stale action.** The receipt reports a hunk `skipped as stale`, or
  `changed since the last refresh`: the snapshot aged between your read and
  your command — another actor edited the file in the gap. That hunk was
  skipped and its membership record untouched; the rest of the scope moved.
  Re-read and retry.
- **Dormant record.** A hunk vanished from the diff (stash, external commit,
  rebase) and its membership record is retained for revival if the exact
  content returns; a HEAD move reports it as a `notice:` that records went
  dormant. A matching outcome, not an error; there is nothing to do.

## Reads

- `gitchange diff --json --no-content [<path>...]` is the hot-loop inventory:
  every hunk's `kind`, `id`, `offset`, `changelist`, `stage`, and `index_only`
  with the `lines` omitted — small enough to run before every planning step.
- `gitchange diff --json <path>` is recovery and verification: the same
  document with each text hunk's `lines`.
- `gitchange diff <path>:<id>` validates an address before you act on it.
- `gitchange status --json` is orientation: `active`, and every group's file
  rows with whole-file staging counts.
- Reads are side-effect-free. Each runs a read-only refresh that writes
  nothing, decides nothing, and advises nothing; a glance never moves
  membership, so glance freely.

## Receipts

A mutation's **receipt** is its one-time output: the echo on stdout
(`assigned 2 hunks → 'fix-login-race'`, `staged 1 hunk — 'fix-login-race'`,
`committed b66485b — 'fix-login-race', 1 hunk`) plus each **advisory** as a
`notice:` line on stderr. Together they carry every decision the command's
internal refresh made — an entry-unit capture, an ambiguous overlap, records
going dormant — possibly beyond the verb's own target. Nothing replays a
receipt: the records keep the facts, the receipt carries the narrative, and a
discarded receipt is gone. Read both streams before moving on. Receipt prose
is not a parsing contract: branch on exit codes, and re-read state through
the JSON reads.

## Exit codes

- `0` — done; the receipt says what happened.
- `1` — refused; stderr is the instruction, and stdout is empty. The lockfile
  is touched only when this refusal names a process that is no longer
  running; a live holder is exit `3`.
- `2` — usage error; stderr names the grammar.
- `3` — lock contention. gitchange already retried internally before giving
  up; run the same command again, unchanged. Nothing about the tree needs
  inspecting first.
