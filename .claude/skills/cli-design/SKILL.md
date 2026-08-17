---
name: cli-design
description: Work one unresolved CLI design question from issue #51 to a decision, and fold the result into that issue's Command reference.
disable-model-invocation: true
---

# CLI design session

Issue #51 is the running record of gitchange's CLI surface. This skill takes one
unresolved item from it, grills it to a decision, and writes the result back into
the issue body.

One item per session. Do not batch.

## 1. Pick the item

Read the issue and its comments: `gh issue view 51 --comments`.

Take the first item, in this order of precedence:

1. the first bullet under `## Open`
2. the first bullet under `## Parity gaps`

If the user named an item, take theirs instead. Say which item you took in one
line, then start.

## 2. Grill it

Invoke `mattpocock-skills:grill-with-docs`. If it will not invoke (it is
user-invocation-only), load `mattpocock-skills:grilling` and
`mattpocock-skills:domain-modeling` yourself — that is all `grill-with-docs`
delegates to.

Grill until both of these are settled:

- **How it works** — the op's behaviour, its refusals, and how it interacts with
  the commands already in the reference.
- **The CLI syntax** — the exact verb, flags, arguments and exit code a user types.

Facts are yours to find, never the user's: `crates/gitchange-core/src/repo.rs` for
the op surface, `crates/gitchange/src/main.rs` for the clap tree, `CONTEXT.md` for
vocabulary, `docs/adr/` for what is already decided.

Hold the session to the issue's own governance: where a gitchange concept has a
git analog, borrow git's vocabulary (CONTEXT.md Conventions); CLI failures are
loud and non-interactive, and every override is a named flag (ADR 0015).

## 3. Write it back

Edit the body with `gh issue edit 51 --body-file <file>`. Keep the current layout
exactly — same sections, same order, same entry shape.

- **Command reference** — where the decision lands. Add or amend the command's
  section: synopsis line, then one bullet per element carrying a status marker
  and the provenance (`(#51 discussion)`). Nothing built in this session, so new
  elements are **[reserved]**. A new command gets its own `###` section, placed
  beside the commands it relates to rather than appended at the end.
- **Decisions** — add an entry only when the reasoning would otherwise be
  re-litigated: bold title, provenance, a short paragraph in present tense
  stating what is true.
- **Open** and **Parity gaps** — delete the item you resolved. If the session
  opened a genuinely new question, add it as a new Open bullet; do not leave the
  old item behind as a hedge.

Everything else the session decided stays in the conversation. The body is a
reference, not a session log.

CONTEXT.md and ADRs follow `/domain-modeling`'s gates and
`docs/agents/documentation-placement.md`: vocabulary goes to CONTEXT.md, a
hard-to-reverse trade-off goes to an ADR or an amendment to one. Most sessions
produce neither.

This skill does not build the subcommand. The clap tree stays untouched.

## Done when

- The item is gone from Open / Parity gaps.
- Command reference states the verb, its flags, and their status.
- The body layout is unchanged apart from those edits.
