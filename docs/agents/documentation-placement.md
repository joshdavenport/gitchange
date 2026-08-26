# Documentation placement

The repository carries **normative** documents only. A document that records
an event rather than binding future work belongs on the issue tracker,
attached to the effort that produced it.

The test to apply before adding a file: **would this document be wrong if
nobody maintained it?** A normative document has a ritual that keeps it true.
A process artifact has none — and a file in the tree implicitly demands to be
kept true, so it gets edited to stay plausible long after it stopped being a
record of anything.

## Normative — in the repo

| Where | What it binds | What keeps it true |
|---|---|---|
| `docs/adr/` | Decisions: what was chosen, the options rejected, the consequences accepted | The **amendment ritual** — a change that contradicts an ADR amends it as part of that change, in a named section citing the issue (`*Amended (issue #57)*`). Amendments are how an ADR survives being wrong. |
| `CONTEXT.md` | The glossary: the project's own vocabulary, including the synonyms it avoids | `/domain-modeling` resolves terms into it as they settle; every output that names a domain concept uses its wording (see `domain.md`) |
| `docs/agents/` | Operating direction for agents: tracker conventions, triage labels, sandboxes, the benchmark harness, this file | Followed on the session it is read, so wrong direction fails visibly rather than quietly |
| `README.md` | The user-facing contract — requirements, the minimum git version | Users hold it to account |
| `skills/` | The product's agent-facing skill: what an agent driving the CLI in a shared tree is taught, and — by absence — what it is not (spec #125). Carried as three verbosities of one content (`-base`, `-spartan`, `-verbose`) while which one agents run best on is being evaluated (#176); a teach-list change edits all three | The ladder-walk test pins the cross-rung property the skill teaches; a refusal reword that breaks the chain fails the suite and flags the skill for re-edit |

Generated tool output is a narrower fourth case: `docs/perf/` holds what
`cargo xbench --out docs/perf` writes (see `benchmarks.md`). It is in the tree
because a documented command puts it there and rewrites it wholesale on the
next run — which is true of `report.md` / `results.csv` and not of the
hand-placed `-ci-ubuntu` copies beside them. The *conclusions* drawn from a
run are not self-maintaining either way: those belong in the ADR they gate, or
the issue they un-gate.

## Process artifacts — on the tracker

Wayfinder briefs and maps, audits, coverage matrices, exit records, session
narratives, agent reports, review findings, disposition tables, benchmark
readings. These are **events**: true when written, historical thereafter.

Post them to the issue they belong to — as a comment, or as a gist linked
from a comment when one won't fit — and say in the same breath that they
reflect the tree at a named commit and are not maintained. That sentence is
the point: it tells the next reader to check `HEAD` instead of trusting the
document, which is the honesty a file in the tree cannot express.

Worked example: the v0.1 exit gate's coverage matrix and exit record were kept
in the repo, and the matrix rotted twice because tickets treated it as living
and edited its rows beside their tests. #67 preserved both on #36, swept their
open verdicts into issues, and deleted them.

## Prototypes live on branches, out of main

A prototype answers one design question and then stops being true. The
**conclusion** goes into the ADR that cites it. The **prototype itself** is
runnable evidence: preserve it on a `prototype/<name>` branch, kept out of
main's future history, with a pointer on the issue that commissioned it.
Main carries no prototype file. Never link a prototype from the tree as though it were a live
reference — that makes a throwaway artifact load-bearing for reading the
code.

## The link direction is repo → tracker

The repo may point at an issue for provenance. **Understanding or changing the
code must never require excavating one.** If a fact turns out to be
load-bearing — a constraint, a rejected option that keeps being re-proposed, a
deviation the tests depend on — promote it into an ADR, or into `CONTEXT.md`
if it is vocabulary, and leave the issue as the audit trail beneath it.

## If a ticket asks you to write a document

Decide which kind it is before choosing a path:

- Does it bind future work, and is there a ritual that will keep it true? →
  the repo. Usually that means it is an ADR, or an amendment to one, not a new
  kind of file.
- Is it a record of what you found or did? → a comment on the issue, with the
  commit it reflects and a not-maintained note.
- Is it a prototype? → conclusion to the ADR, code to a `prototype/<name>`
  branch linked from the issue.
- Is it a file a documented command regenerates? → wherever that command
  writes it, and nowhere else.

When a ticket's own wording implies a new in-repo document, apply the test
anyway and say so if the answer differs.
