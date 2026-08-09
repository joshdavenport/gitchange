# Shared-tree multi-actor model: attribution, not isolation

gitchange supports multiple concurrent actors — humans and coding agents —
working in **one shared working tree**. The guarantee is **attribution**:
every in-flight hunk answers "whose is this?" via changelist ownership, and
finished work commits per-changelist (ADR 0004) regardless of what else is
in flight. The guarantee is **never isolation**: builds, tests and greps
see the whole tree, including other actors' half-finished work. Runtime
interference is the coordinator's problem, exactly as it is without
gitchange. Where interference is likely, gitchange is the wrong tool and
worktree isolation (ADR 0002's per-worktree state) is the right one — a
supported answer, not a failure of this model.

## Why a shared tree at all

Two narratives motivate the model, one per actor kind:

- A human working several things at once, easily accounting for not
  stepping on their own toes.
- Multiple agents working in parallel under human coordination that keeps
  them off each other's toes.

The reflex answer — one worktree per actor — is often right and already
supported. It fails where tooling binds to the tree's identity (local
domain mapping, path-resolved `.env`/config that a browser or service must
see), and it is overkill for coordinated non-colliding work. Existing
shared-tree agent tooling is all *prospective prevention* — file leases,
zone conventions — and none of it attributes the resulting changes or
disentangles the commits; that retrospective half is the ground gitchange
takes (prior-art survey: issue #52).

Diagnosis falls out of attribution without being promised: an actor whose
test run goes red can ask whether the failing area contains hunks it does
not own (`status --json`, `diff --json`) and reach "probably not my
failure" as a query rather than an investigation. The tool informs; it
never prevents.

## The explicit-mode contract

Ambient state cannot be trusted with more than one actor. A filesystem
change carries no author, so capture's inference — "the active
changelist's owner made this" — is sound only with exactly one actor, and
the inference runs in whatever process refreshes (ADR 0005), not the
process that edited. The contract for agents is therefore: **never rely on
ambient state; every operation names its target.**

The agent contract, shipped as a skill once CLI parity exists (#51):

1. Verify the active changelist is `unassigned`; switch it there if not.
2. Create an intent-named changelist (`fix-login-race`, never a session
   id) — the name is the broadcast channel telling other actors what is
   ongoing. A name collision is an error, not adoption: a duplicate name
   is a coordination smell to surface, and adoption would be two actors
   silently sharing a changelist — the race again at changelist scope.
3. Assign your changes to it explicitly as you work, so the All view stays
   live-legible mid-flight — the thing worktrees cannot give.
4. Stage with `gitchange add` (alias `stage`), commit with
   `gitchange commit <name>` when green, delete the changelist when done.
   Raw `git add` remains valid — staging is write-through (ADR 0003) — but
   the skill teaches one vocabulary: the add-then-commit shape agents
   already know from git, with gitchange verbs throughout.
5. Never switch the active changelist to a real changelist; never assign
   into, commit, or delete another actor's changelist.
6. Read, don't wonder: `status --json` and `diff --json` answer "what else
   is going on here", so foreign changes are expected context, not
   confusion — the changelist names say what, even when they can't say why.

CLI behaviour backing this is loud and non-interactive: every TUI
warn-and-confirm becomes a refusal with a specific error and a named
override flag. Nothing broadens a payload silently; agents navigate
unfamiliar terrain by error text.

## Capture off is `switch unassigned`

"Disable auto-capture" reduces to "capture flows to unassigned" — and
unassigned already exists as a pseudo-changelist with a reserved name. So
the mechanism is: **the active changelist may be `unassigned`.** Capture
and ambiguous-edit routing (dormant revival, overlap) then resolve to
unassigned with the usual advisories — with multiple actors the only
correct destination, since no inference toward a real changelist is sound.

This amends the Active-changelist invariant ("exactly one is active
whenever any changelists exist"): exactly one of {the changelists,
unassigned} is active. The CONTEXT.md entry is amended when the behaviour
lands (#52). No new verb, state, or indicator: `switch unassigned` is
already-reserved grammar (#51), and the `*` marker on the unassigned group
is the status-panel visibility #52 asked for.

In a multi-actor session the human loses capture too and works explicit
like the agents. Accepted: a human clobbering the state-of-play
mid-session is human error no mechanism could absorb.

## Commit choreography

One tree has one HEAD: all concurrent work is destined for the current
branch, and finished changelists become separate commits on it,
serialized. Agents may commit their own changelist when their work is
green — ADR 0004's temp index takes only that changelist's staged hunks,
and git's own index lock serializes the final step. A human batching
commits at sync points is a permitted style within the model, not a
different model.

## Scope boundary: same-file concurrency

Out of scope. Assignment is per-path until hunk addressing lands (#51), so
two actors editing one file can cross-contaminate at hunk granularity. The
operating model is coordinated, non-colliding work; where work genuinely
collides in the same files, worktrees are the tool (ADR 0002).

## Considered options

- **Runtime capture toggle (#52 as filed)** — rejected: a repo-global
  boolean has the same concurrency profile as the state it guards. One
  actor disables, another re-enables mid-run (the race, one level up); a
  crash leaves it off silently; every actor needs re-enable discipline.
  `switch unassigned` states the same fact through state that already
  exists and is already visible.
- **Persistent capture config (`capture.auto = false`)** — rejected for
  now: duplicates what `switch unassigned` expresses. Revisit only if
  "someone switched away from unassigned mid-session" proves to be a real
  accident class.
- **Changelist materialization** (check out one changelist's content alone
  for a test run) — rejected: isolation re-entering through the back door;
  a worse worktree that dilutes the thesis.
- **Branch-per-changelist** — rejected as incoherent in a shared tree: one
  working tree has one HEAD, and moving it yanks the tree out from under
  every other actor.
- **Claims/reservations** (Perforce-style prospective open-for-edit; the
  agent-coordination ecosystem's file leases) — rejected permanently, not
  deferred. Prevention is a different tool's story: gitchange is
  descriptive — it attributes what happened — and composes with
  prospective coordination running in front of it. If dogfooding shows
  attribution without claims fails, the answer is a new tool, not a
  feature.

## Consequences

- #51 gains obligated verbs: `add`/`stage`, `commit <changelist>` (strict
  non-interactive defaults), `diff` with `--json` (the hunk-level read
  surface, carrying hunk IDs for `assign`), `status --json`
  (file/changelist level), and create/delete (names undecided — creation
  must not activate, so a `switch -c` shape is out).
- Hunk addressing (#51) gains urgency: assign-as-you-go wants cheap,
  scriptable per-hunk assignment.
- Op parity (#51's decision) is what makes the contract possible: every op
  the skill mandates must exist as a CLI form.
- The skill is written when the verbs exist; the contract above is its
  content. A skill teaching unbuilt verbs would be a lie in this repo's
  own terms.
- `git status`/`git diff` in another terminal keep showing the union of
  everyone's work — honest, unseparated; gitchange's surfaces are where
  separation lives.
