# CLI machine surface: one versioned JSON envelope; receipts stay text

The reads own the machine surface: `status --json` and `diff --json`
each emit exactly one JSON document on stdout — a versioned envelope
object — and **no mutating command takes `--json`**; a mutation's
receipt is text (CONTEXT.md, **Receipt**). Agents are a primary consumer
(ADR 0015), so the dialect is public API from the day it ships; its
shape, its version policy, and the machine/text boundary are fixed here
rather than grown surface by surface.

## The dialect

- **One envelope object per `--json` surface** — never a bare array,
  never NDJSON. The snapshot is atomic and whole (ADR 0005); there is
  nothing to stream.
- **Field names are snake_case** — the state file's convention.
- **Unions are discriminated by an explicit string field (`kind`)**,
  never by field presence.
- **Ordering is promised** — the opposite of git porcelain's stance:
  groups in All-view order, files in path order, hunks in file order.
  The JSON must never disagree with the text output.
- **Bytes are plain JSON strings** — no `{text|bytes}` union. Non-UTF-8
  paths fail the whole refresh (ADR 0010), and diff content is lossily
  UTF-8 before the serialiser sees it, so a union would advertise
  fidelity gitchange does not have.
- **Absence is `null`, spelled rather than omitted** — an optional field
  is always present, so a consumer never has to tell "no value" from "a
  field this version does not have". *Amended (issue #159):* one field
  is omitted rather than nulled — `diff --json`'s `lines` under
  `--no-content`, where the caller asked not to be sent content, so its
  absence is the answer rather than a fact about the hunk. A `null`
  there would read as a hunk with no lines. No other field omits, and a
  new omission is a change to this ADR.
- **Errors stay plain text on stderr** — one schema to version.
- **No JSON surface carries advisories.** Read envelopes have no
  advisories field — a read-only refresh writes nothing, decides
  nothing, and advises nothing (ADR 0005) — and mutation receipts are
  text (below). A `{ kind, message, …variant fields }` advisory shape
  returns only if a machine receipt ever lands, as an additive change
  under the contract below.

## `schema_version` and the additive contract

The envelope carries an integer `schema_version` — a field, not a flag.
The version is global to the JSON dialect: `status --json` and
`diff --json` share types (change kinds, stage tokens, the advisory
shape), so a breaking change to either bumps the one integer for both.
One wire module in core owns the constant and the serialisers, so the two
surfaces cannot drift.

The contract published beside the field: adding a field, adding a member
to a string-typed enum, and populating a previously-null field are not
breaking and do not bump the version; renaming, removing, retyping, or
changing the meaning of a field does. Consumers must ignore unknown
fields and unknown enum values.

Distinct from the state-file version, whose mismatch is a refusal — two
versions, two policies, easy to confuse.

## Receipts stay text

A receipt is a tripwire, not a data carrier: the records keep the facts,
and `status --json` / `diff --json` re-read them. This is the VCS
lineage's stated contract — gh: "keep `--json` support scoped to only
list- and view-type commands"; git porcelain and jj expect
mutate-then-re-read — and no surveyed tool puts warnings inside a stdout
mutation envelope.

Reversibility settled the polarity: under the additive contract a future
`--json` receipt is a non-breaking addition, while an envelope shipped
now becomes API on day one. How a receipt is composed — the echo, the
`notice:` lines, what a failure leaves on stdout — is the CLI spec's to
define; CONTEXT.md's **Receipt** entry grounds the term.

## Considered options

- **A versioned flag (`--porcelain=v2` style)** — rejected: it freezes
  v1 on day one, and gitchange has no installed base to protect.
- **NDJSON / streaming** — rejected: the snapshot is atomic and whole;
  a streaming dialect shapes the wire around a capability the model
  rules out.
- **Field selection (gh's mandatory `--json <fields>` list)** — an
  explicit no. gh's field list is a real compatibility mechanism and it
  does not transfer: gitchange's snapshot is computed before printing,
  so selection saves nothing; the document is a nested tagged union a
  flat name list cannot address; and every selectable name widens the
  public API it is meant to protect. The version field plus the additive
  contract do the compatibility job; payload size is handled by scoping
  and `--no-content`, never projection. What gitchange borrows from gh
  instead is its error shape — an unrecognised name lists the valid
  candidates — carried by the CLI spec.
- **A `{text|bytes}` content union** — rejected: it advertises fidelity
  gitchange does not have (ADR 0010; lossy UTF-8 diff content).
