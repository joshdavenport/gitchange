### gitchange diff

`gitchange [-C <dir>] diff [<changelist>] [--] [<path>[:<hunk-id>]...] [--json] [--no-content]` — read-only

- verb + `--json` **[reserved]** (ADR 0015) — the hunk-level read surface: content, ownership, and hunk IDs for `assign`
- envelope **[reserved]** — one JSON document on stdout: `{ schema_version, files }`; no advisories field — the read path discards the recompute's advisories as undelivered previews (see Decisions). Errors stay plain text on stderr. Repo context (head, active changelist, operation in progress) is `status --json`'s, per the read-surface split
- file object **[reserved]** (#51 discussion; amended #112) — `path` · `change_kind` (`"added"` `"modified"` `"deleted"` `"type_changed"` `"untracked"` `"conflicted"`) · `binary` · `sides` (per side `null` or `{ oid, size }`; present exactly when the file presents a whole-file hunk, else `null`) · `hunks` in file order: mode hunk first, then whole-file, then text. No file-level `mode_delta` — mode facts are hunk-attributed (#112; see the facts-not-labels decision). A conflicted file appears with `hunks: []` — quarantine is stated, never silent
- hunk object, common fields **[reserved]** — `id` (`h` sigil + full 64 hex; the sigil travels on the wire) · `offset` (integer or `null`; non-null means the address is `<id>/<n>`) · `kind` (`"text"` | `"whole_file"` | `"mode"`) · `changelist` (`null` = **unassigned**) · `stage` (`"staged"` | `"unstaged"` | `"staged_stale"`) · `index_only` (true: the hunk exists only in the index — staged then worktree-reverted)
- `"text"` hunk **[reserved]** — adds `old_start`/`old_lines`/`new_start`/`new_lines` and `lines` as `{ origin, content }`: origins `" "` `"+"` `"-"` plus the no-newline markers `"="` `">"` `"<"`, content verbatim with its trailing newline
- `"whole_file"` and `"mode"` hunks **[reserved]** (#51 discussion; amended #112) — no coordinates, no lines; each adds `mode_delta` (`{ "before": "100644", "after": "100755" }` or `null`, octal strings, no kind field — flavour derives from the hunk's `kind`): a `"mode"` hunk's delta is its permission flip, never `null`; a `"whole_file"` hunk's is the type delta, non-null exactly when the change is a type change. The file's `sides` carries the content facts (facts, not labels; see Decisions). No `oids` field on the hunk — `sides` is the single carrier
- `--no-content` **[reserved]** — omits `lines` from text hunks; nothing else changes (same envelope, same IDs). The cheap inventory read for assign-as-you-go. Content is on by default: a verb called `diff` promises content. JSON-only: without `--json` it is a usage error (exit `2`)
- text face **[reserved]** — an annotated unified patch: git's patch format, files flat in path order (the JSON must never disagree with the text), each `@@` header carrying gitchange's facts in git's function-context slot as `['<changelist>' <glyph> <path>:<id>]` — owner via core's `holder_label` rendering (bare `unassigned`), the status face's stage glyph, and the full copyable address with an abbreviated ID (input accepts ≥7-char prefixes; the JSON keeps the full 64). Degenerate hunks borrow git's own lines (`Binary files … differ`, `old mode`/`new mode`) with the same bracketed suffix; a conflicted file prints its file header plus one quarantine line, no content (ADR 0007). A display, not a contract (see the text-face decision)
- plain bytes **[reserved]** — no color, no pager, no tty detection; the TUI is the rich human surface. An empty scope prints nothing, exit `0`, agreeing with `files: []`
- `[<changelist>]` scope **[reserved]** — git's rev slot: at most one; `unassigned` legal; `all` refuses (exit `1`) — bare `diff` is already the whole view. Bare invocation shows the whole hunk universe; git's unstaged-only default is not borrowed (see the positional-scoping decision). A changelist scope selects the files where it owns ≥1 hunk, returned whole (see the whole-file-objects decision); conflicted files own nothing and never match
- token resolution **[reserved]** — git's rules: a token matching both a changelist and an existing path refuses, naming `--`; matching neither refuses naming both readings, candidates listed. Exit `1`, not `2` — the refusal depends on repo state clap cannot see. Everything after the first positional or `--` is a path
- `[<path>...]` scope **[reserved]** — the shared addressing grammar verbatim (cwd-relative, literal, directory refuses listing the changed files under it); paths union, changelist ∩ paths intersect at the file level. A nonexistent path refuses (exit `1`, all-or-nothing with other offenders); a valid-but-empty scope is `files: []`, exit `0`
- `<path>:<hunk-id>` scope **[reserved]** — selector with validation: a live ID returns the whole file object; a not-found, stale, or ambiguous ID refuses (exit `1`) — the verification read's staleness tripwire. Never narrows `hunks` (see the whole-file-objects decision)
- no stage filter, no `--containing` **[reserved]** — the per-hunk `stage` field serves the stage dimension, and the ladder routes multi-match to explicit IDs; both are additive later (see Decisions)

Worked shape (IDs elided; the wire carries the full 64 hex):

```json
{
  "schema_version": 1,
  "files": [
    {
      "path": "src/auth.rs",
      "change_kind": "modified",
      "binary": false,
      "sides": null,
      "hunks": [
        {
          "id": "h3f9c…",
          "offset": null,
          "kind": "mode",
          "changelist": "fix-login-race",
          "stage": "unstaged",
          "index_only": false,
          "mode_delta": { "before": "100644", "after": "100755" }
        },
        {
          "id": "h8c46…",
          "offset": null,
          "kind": "text",
          "changelist": null,
          "stage": "staged_stale",
          "index_only": false,
          "old_start": 38,
          "old_lines": 6,
          "new_start": 39,
          "new_lines": 7,
          "lines": [
            { "origin": " ", "content": "    let token = session.token();\n" },
            { "origin": "+", "content": "    retry(3);\n" }
          ]
        }
      ]
    },
    {
      "path": "assets/logo.png",
      "change_kind": "modified",
      "binary": true,
      "sides": {
        "head": { "oid": "b3f2…", "size": 1024 },
        "changed": { "oid": "c4e3…", "size": 2048 }
      },
      "hunks": [
        {
          "id": "h47c1…",
          "offset": null,
          "kind": "whole_file",
          "changelist": "docs-pass",
          "stage": "staged",
          "index_only": false,
          "mode_delta": null
        }
      ]
    }
  ]
}
```

## Decisions

[Read surfaces split along git lines (ADR 0015)](decisions/read-surfaces.md)
[Reads never persist; the deciding refresh's receipt is the only advisory delivery (ADR 0005 amendment)](decisions/reads-never-persist.md)
[Degenerate hunks travel as facts, not labels - amended #112](decisions/degenerate-hunks-travel-as-facts.md)
[`diff` scopes positionally in git's rev slot; resolution is git's](decisions/diffs-scope-positionally-in-gits-rev-slot.md)
[`diff` scoping selects whole file objects, never hunk subsets](decisions/diff-scoping-selects-whole-file-objects.md)
[`diff`'s text face is an annotated patch — a display, not a contract](decisions/diffs-text-face.md)
[`diff` takes no stage filter and no `--containing`](decisions/diff-no-stage-filter-no-containing.md)
[The JSON dialect: envelope, tagged unions, promised order](decisions/json-dialect.md)
[One `schema_version`, one written additive contract](decisions/schema_version.md)
[No `--json` field selection](decisions/no-json-field-selection.md)
