# Path encoding: UTF-8 strings, loud failure on non-UTF-8

Every path gitchange handles — `ChangedFile.path` in core, paths inside
membership records, anything the state file persists — is a UTF-8 Rust
`String` carrying git's repo-relative path bytes verbatim. A repository
containing a path that is not valid UTF-8 makes refresh fail loudly with
`Error::NonUtf8Path` naming the offending path; nothing is persisted from
that refresh. Non-UTF-8 paths are **unsupported in v0.1, loudly — never
lossily**.

This resolves the deferral from ticket 22, where `String::from_utf8_lossy`
silently mangled non-UTF-8 path bytes. Lossy conversion is the one option
that is actively dangerous here: membership records are keyed by path, and
whole-file hunks keep membership by path continuity, so a lossy path can
collide two distinct non-UTF-8 paths and fails to round-trip against what
git reports on the next refresh — silent misfiling, exactly the failure
class ADR 0001 forbids. The decision is frozen now because this ticket
freezes state-file schema v1: an encoding change after paths persist is a
migration; before, it is a line of code.

## Considered options

- **UTF-8 `String`, fail loudly on non-UTF-8 (chosen)** — preserves the
  human-readable JSON property ADR 0002 is built on (`cat` is the
  debugger; paths read as paths). Non-UTF-8 paths in real repositories are
  vanishingly rare, and macOS/Windows filesystems effectively enforce
  UTF-8 already. The failure is visible, attributable, and recoverable by
  renaming the offending file.
- **Raw path bytes with a JSON escape convention** — byte-faithful, but
  invents a private encoding every reader of the state file must know,
  sacrificing plain-text debuggability for a case we can instead refuse.
  Remains the natural v2 shape if support is ever demanded; the version
  field makes that an explicit migration, not a guess.
- **`PathBuf`** — platform-dependent (`OsString` has no portable JSON
  form), so it does not solve serialization, and it destroys the
  byte-for-byte equality with git's reporting that matching depends on.

## Consequences

- Path comparison everywhere (matcher, membership, state) is plain
  `String` equality against exactly the bytes git reports.
- A repository with a non-UTF-8 path cannot use gitchange at all in v0.1
  — one bad path fails the whole refresh. Acceptable: partial visibility
  would violate ADR 0003's invariant that every committable hunk is
  visible.
- Supporting such paths later means a schema version bump and an escape
  convention, with no ambiguity about what v1 files contain (valid UTF-8,
  always).
