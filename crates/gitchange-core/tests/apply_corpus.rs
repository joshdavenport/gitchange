//! Apply-correctness corpus (ticket 27, ADR 0008 exit criterion): each
//! case is data — base tree, staged tree, worktree tree, one op, expected
//! index bytes — materialized through `RepoFixture` into a fresh repo and
//! run through the real sync ops. Adding a case requires only data.
//!
//! The corpus doubles as the certification suite any future `GitBackend`
//! adapter (the shell-out fallback) must pass to count as behaviorally
//! equivalent. The commit-mechanics ticket extends `Case` with
//! commit-result expectations.

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use gitchange_core::{Hunk, Notice, Repo, Snapshot};
use support::RepoFixture;

/// Repo-relative paths with file bytes — one tree state.
type Tree = Vec<(&'static str, Vec<u8>)>;

struct Case {
    /// Files committed as the corpus base.
    base: Tree,
    /// Files written and staged after the base commit.
    stage_writes: Tree,
    /// Paths whose deletion is staged (index entry removed; the worktree
    /// file goes through `worktree_removals`).
    stage_removals: Vec<&'static str>,
    /// Paths chmod +x'd and staged (unix-only cases).
    stage_exec: Vec<&'static str>,
    /// Files written to the worktree after any staging.
    worktree_writes: Tree,
    /// Paths removed from the worktree after any staging.
    worktree_removals: Vec<&'static str>,
    /// Paths chmod +x'd in the worktree, unstaged (unix-only cases).
    worktree_exec: Vec<&'static str>,
    /// Files rewritten after the snapshot, before the op — the
    /// validate-at-apply cases (moved hunks, staleness).
    after_snapshot_writes: Tree,
    op: Op,
    /// Expected index blob per path after the op; `None` = no entry.
    index: Vec<(&'static str, Option<Vec<u8>>)>,
    /// Expected index filemode per path (unix-only cases).
    index_modes: Vec<(&'static str, u32)>,
    /// Paths expected in `StaleHunk` notices; empty = clean apply.
    stale: Vec<&'static str>,
}

impl Case {
    fn new(op: Op) -> Self {
        Self {
            base: Vec::new(),
            stage_writes: Vec::new(),
            stage_removals: Vec::new(),
            stage_exec: Vec::new(),
            worktree_writes: Vec::new(),
            worktree_removals: Vec::new(),
            worktree_exec: Vec::new(),
            after_snapshot_writes: Vec::new(),
            op,
            index: Vec::new(),
            index_modes: Vec::new(),
            stale: Vec::new(),
        }
    }
}

enum Op {
    /// Stage the file's `hunk`-th snapshot hunk (universe order).
    StageHunk {
        path: &'static str,
        hunk: usize,
    },
    UnstageHunk {
        path: &'static str,
        hunk: usize,
    },
    StageFile(&'static str),
    UnstageFile(&'static str),
}

/// Materialize the case into a fresh repo, run its op, assert the index.
fn run(case: Case) {
    let fixture = RepoFixture::new();
    for (path, bytes) in &case.base {
        fixture.write_bytes(path, bytes);
    }
    fixture.commit_all("base");
    for (path, bytes) in &case.stage_writes {
        fixture.write_bytes(path, bytes).stage(path);
    }
    for path in &case.stage_removals {
        fixture.stage_removal(path);
    }
    for path in &case.stage_exec {
        set_exec(&fixture.path().join(path));
        fixture.stage(path);
    }
    for (path, bytes) in &case.worktree_writes {
        fixture.write_bytes(path, bytes);
    }
    for path in &case.worktree_removals {
        fs::remove_file(fixture.path().join(path)).unwrap();
    }
    for path in &case.worktree_exec {
        set_exec(&fixture.path().join(path));
    }

    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap();
    for (path, bytes) in &case.after_snapshot_writes {
        fixture.write_bytes(path, bytes);
    }
    let worktree_before = worktree_state(fixture.path());

    let notices = match &case.op {
        Op::StageHunk { path, hunk } => repo
            .stage_hunk(path, &hunk_at(&snapshot, path, *hunk))
            .unwrap(),
        Op::UnstageHunk { path, hunk } => repo
            .unstage_hunk(path, &hunk_at(&snapshot, path, *hunk))
            .unwrap(),
        Op::StageFile(path) => {
            repo.stage_file(path).unwrap();
            Vec::new()
        }
        Op::UnstageFile(path) => {
            repo.unstage_file(path).unwrap();
            Vec::new()
        }
    };

    let stale: Vec<&str> = notices
        .iter()
        .map(|notice| match notice {
            Notice::StaleHunk { path, .. } => path.as_str(),
            other => panic!("unexpected notice from apply op: {other:?}"),
        })
        .collect();
    assert_eq!(stale, case.stale, "stale notices");
    for (path, expected) in &case.index {
        assert_eq!(
            fixture.index_bytes(path),
            expected.clone(),
            "index content of {path}"
        );
    }
    for (path, mode) in &case.index_modes {
        assert_eq!(
            fixture.index_mode(path),
            Some(*mode),
            "index mode of {path}"
        );
    }
    assert_eq!(
        worktree_state(fixture.path()),
        worktree_before,
        "apply ops must never touch the worktree"
    );
}

fn hunk_at(snapshot: &Snapshot, path: &str, position: usize) -> Hunk {
    let file = snapshot
        .files
        .iter()
        .find(|file| file.path == path)
        .unwrap_or_else(|| panic!("no changed file {path} in snapshot"));
    file.hunks
        .get(position)
        .unwrap_or_else(|| {
            panic!(
                "{path} has {} hunks, case wants index {position}",
                file.hunks.len()
            )
        })
        .clone()
}

/// Every worktree file's bytes, `.git` excluded — apply ops write only
/// the index, so this must be identical before and after.
fn worktree_state(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_name() == ".git" {
                continue;
            }
            if path.is_dir() {
                walk(&path, root, out);
            } else {
                out.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(&path).unwrap(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

#[cfg(unix)]
fn set_exec(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(perms.mode() | 0o111);
    fs::set_permissions(path, perms).unwrap();
}

#[cfg(not(unix))]
fn set_exec(_path: &Path) {}

/// `count` numbered lines with `eol` endings; `edits` replace whole lines
/// by 1-based number — the corpus's compact way to spell file content.
fn numbered(count: usize, eol: &str, edits: &[(usize, &str)]) -> Vec<u8> {
    (1..=count)
        .flat_map(|n| {
            let text = edits
                .iter()
                .find(|(line, _)| *line == n)
                .map(|(_, text)| (*text).to_owned())
                .unwrap_or_else(|| format!("line {n}"));
            format!("{text}{eol}").into_bytes()
        })
        .collect()
}

fn lf(count: usize, edits: &[(usize, &str)]) -> Vec<u8> {
    numbered(count, "\n", edits)
}

fn crlf(count: usize, edits: &[(usize, &str)]) -> Vec<u8> {
    numbered(count, "\r\n", edits)
}

/// Latin-1 lines — each carries a raw 0xE9 ('é'), invalid UTF-8, so hunk
/// text goes through the lossy decode while the blob must stay verbatim.
fn latin1(count: usize, eol: &str, edits: &[(usize, &str)]) -> Vec<u8> {
    (1..=count)
        .flat_map(|n| {
            let text = edits
                .iter()
                .find(|(line, _)| *line == n)
                .map(|(_, text)| *text)
                .unwrap_or("base");
            let mut line = b"caf".to_vec();
            line.push(0xE9);
            line.extend_from_slice(format!(" {n} {text}{eol}").as_bytes());
            line
        })
        .collect()
}

/// Latin-1 lines with the final line's newline chopped off.
fn latin1_no_trailing_newline(count: usize, edits: &[(usize, &str)]) -> Vec<u8> {
    let mut bytes = latin1(count, "\n", edits);
    bytes.pop();
    bytes
}

/// Identical 8-line blocks, one per edit (the block's fourth line),
/// separated by unique filler — the repeated-code-block shape.
fn repeated_blocks(edits: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    for (position, edit) in edits.iter().enumerate() {
        if position > 0 {
            for n in 1..=7 {
                out.extend_from_slice(format!("separator {position} {n}\n").as_bytes());
            }
        }
        out.extend_from_slice(
            format!("alpha\nbravo\ncharlie\n{edit}\necho\nfoxtrot\ngolf\nhotel\n").as_bytes(),
        );
    }
    out
}

macro_rules! corpus {
    ($($(#[$meta:meta])* $name:ident: $case:expr;)+) => {
        $(
            $(#[$meta])*
            #[test]
            fn $name() {
                run($case);
            }
        )+
    };
}

corpus! {
    // ——— trailing-newline edges ———

    stage_hunk_keeps_a_missing_trailing_newline:
        Case {
            base: vec![("a.txt", b"one\ntwo\nthree".to_vec())],
            worktree_writes: vec![("a.txt", b"one\nTWO\nthree".to_vec())],
            index: vec![("a.txt", Some(b"one\nTWO\nthree".to_vec()))],
            ..Case::new(Op::StageHunk { path: "a.txt", hunk: 0 })
        };

    stage_hunk_adds_a_trailing_newline:
        Case {
            base: vec![("a.txt", b"one\ntwo\nthree".to_vec())],
            worktree_writes: vec![("a.txt", b"one\ntwo\nthree\n".to_vec())],
            index: vec![("a.txt", Some(b"one\ntwo\nthree\n".to_vec()))],
            ..Case::new(Op::StageHunk { path: "a.txt", hunk: 0 })
        };

    stage_hunk_removes_a_trailing_newline:
        Case {
            base: vec![("a.txt", b"one\ntwo\nthree\n".to_vec())],
            worktree_writes: vec![("a.txt", b"one\ntwo\nthree".to_vec())],
            index: vec![("a.txt", Some(b"one\ntwo\nthree".to_vec()))],
            ..Case::new(Op::StageHunk { path: "a.txt", hunk: 0 })
        };

    unstage_hunk_restores_a_missing_trailing_newline:
        Case {
            base: vec![("a.txt", b"one\ntwo\nthree".to_vec())],
            stage_writes: vec![("a.txt", b"one\nTWO\nthree".to_vec())],
            index: vec![("a.txt", Some(b"one\ntwo\nthree".to_vec()))],
            ..Case::new(Op::UnstageHunk { path: "a.txt", hunk: 0 })
        };

    // ——— CRLF ———

    stage_hunk_keeps_crlf_endings_verbatim:
        Case {
            base: vec![("a.txt", crlf(9, &[]))],
            worktree_writes: vec![("a.txt", crlf(9, &[(5, "edited five")]))],
            index: vec![("a.txt", Some(crlf(9, &[(5, "edited five")])))],
            ..Case::new(Op::StageHunk { path: "a.txt", hunk: 0 })
        };

    unstage_hunk_restores_crlf_endings_verbatim:
        Case {
            base: vec![("a.txt", crlf(9, &[]))],
            stage_writes: vec![("a.txt", crlf(9, &[(5, "edited five")]))],
            index: vec![("a.txt", Some(crlf(9, &[])))],
            ..Case::new(Op::UnstageHunk { path: "a.txt", hunk: 0 })
        };

    stage_hunk_crlf_without_trailing_newline:
        Case {
            base: vec![("a.txt", b"one\r\ntwo\r\nthree".to_vec())],
            worktree_writes: vec![("a.txt", b"one\r\nTWO\r\nthree".to_vec())],
            index: vec![("a.txt", Some(b"one\r\nTWO\r\nthree".to_vec()))],
            ..Case::new(Op::StageHunk { path: "a.txt", hunk: 0 })
        };

    // ——— blank-line-only hunks ———

    stage_hunk_of_only_blank_line_insertions:
        Case {
            base: vec![("a.txt", lf(9, &[]))],
            worktree_writes: vec![(
                "a.txt",
                b"line 1\nline 2\nline 3\nline 4\nline 5\n\n\nline 6\nline 7\nline 8\nline 9\n"
                    .to_vec(),
            )],
            index: vec![(
                "a.txt",
                Some(
                    b"line 1\nline 2\nline 3\nline 4\nline 5\n\n\nline 6\nline 7\nline 8\nline 9\n"
                        .to_vec(),
                ),
            )],
            ..Case::new(Op::StageHunk { path: "a.txt", hunk: 0 })
        };

    stage_hunk_of_only_blank_line_deletions:
        Case {
            base: vec![(
                "a.txt",
                b"line 1\nline 2\nline 3\nline 4\nline 5\n\n\nline 6\nline 7\nline 8\nline 9\n"
                    .to_vec(),
            )],
            worktree_writes: vec![("a.txt", lf(9, &[]))],
            index: vec![("a.txt", Some(lf(9, &[])))],
            ..Case::new(Op::StageHunk { path: "a.txt", hunk: 0 })
        };

    // ——— pure line deletions (the hunk-anchor regime: a deletion-only
    // hunk's header names the line before it) ———

    stage_hunk_deleting_lines_mid_file:
        Case {
            base: vec![("a.txt", lf(9, &[]))],
            worktree_writes: vec![(
                "a.txt",
                b"line 1\nline 2\nline 3\nline 4\nline 7\nline 8\nline 9\n".to_vec(),
            )],
            index: vec![(
                "a.txt",
                Some(b"line 1\nline 2\nline 3\nline 4\nline 7\nline 8\nline 9\n".to_vec()),
            )],
            ..Case::new(Op::StageHunk { path: "a.txt", hunk: 0 })
        };

    unstage_hunk_restores_deleted_lines:
        Case {
            base: vec![("a.txt", lf(9, &[]))],
            stage_writes: vec![(
                "a.txt",
                b"line 1\nline 2\nline 3\nline 4\nline 7\nline 8\nline 9\n".to_vec(),
            )],
            index: vec![("a.txt", Some(lf(9, &[])))],
            ..Case::new(Op::UnstageHunk { path: "a.txt", hunk: 0 })
        };

    stage_hunk_deleting_the_first_lines:
        Case {
            base: vec![("a.txt", lf(9, &[]))],
            worktree_writes: vec![(
                "a.txt",
                b"line 3\nline 4\nline 5\nline 6\nline 7\nline 8\nline 9\n".to_vec(),
            )],
            index: vec![(
                "a.txt",
                Some(b"line 3\nline 4\nline 5\nline 6\nline 7\nline 8\nline 9\n".to_vec()),
            )],
            ..Case::new(Op::StageHunk { path: "a.txt", hunk: 0 })
        };

    // A pure deletion's change core has no width and counts as touching
    // both neighbouring lines — it must still not leak into an op on a
    // separate hunk nearby, in either direction.
    a_nearby_pure_deletion_is_not_staged_with_another_hunk:
        Case {
            base: vec![("a.txt", lf(16, &[]))],
            worktree_writes: vec![(
                "a.txt",
                b"line 1\nline 2\nline 3\nEDIT four\nline 5\nline 6\nline 7\nline 8\n\
line 9\nline 10\nline 11\nline 14\nline 15\nline 16\n"
                    .to_vec(),
            )],
            index: vec![("a.txt", Some(lf(16, &[(4, "EDIT four")])))],
            ..Case::new(Op::StageHunk { path: "a.txt", hunk: 0 })
        };

    a_nearby_pure_deletion_stages_alone:
        Case {
            base: vec![("a.txt", lf(16, &[]))],
            worktree_writes: vec![(
                "a.txt",
                b"line 1\nline 2\nline 3\nEDIT four\nline 5\nline 6\nline 7\nline 8\n\
line 9\nline 10\nline 11\nline 14\nline 15\nline 16\n"
                    .to_vec(),
            )],
            index: vec![(
                "a.txt",
                Some(
                    b"line 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8\n\
line 9\nline 10\nline 11\nline 14\nline 15\nline 16\n"
                        .to_vec(),
                ),
            )],
            ..Case::new(Op::StageHunk { path: "a.txt", hunk: 1 })
        };

    // ——— adjacent hunks (contexts abut: minimal separation that still
    // splits into two hunks) ———

    adjacent_hunks_stage_the_first_alone:
        Case {
            base: vec![("a.txt", lf(16, &[]))],
            worktree_writes: vec![(
                "a.txt",
                lf(16, &[(4, "EDIT four"), (12, "EDIT twelve")]),
            )],
            index: vec![("a.txt", Some(lf(16, &[(4, "EDIT four")])))],
            ..Case::new(Op::StageHunk { path: "a.txt", hunk: 0 })
        };

    adjacent_hunks_stage_the_second_alone:
        Case {
            base: vec![("a.txt", lf(16, &[]))],
            worktree_writes: vec![(
                "a.txt",
                lf(16, &[(4, "EDIT four"), (12, "EDIT twelve")]),
            )],
            index: vec![("a.txt", Some(lf(16, &[(12, "EDIT twelve")])))],
            ..Case::new(Op::StageHunk { path: "a.txt", hunk: 1 })
        };

    adjacent_hunks_unstage_the_first_leaves_the_second:
        Case {
            base: vec![("a.txt", lf(16, &[]))],
            stage_writes: vec![(
                "a.txt",
                lf(16, &[(4, "EDIT four"), (12, "EDIT twelve")]),
            )],
            index: vec![("a.txt", Some(lf(16, &[(12, "EDIT twelve")])))],
            ..Case::new(Op::UnstageHunk { path: "a.txt", hunk: 0 })
        };

    // ——— file creation / deletion ———

    stage_file_creates_an_index_entry_for_an_untracked_file:
        Case {
            base: vec![("keep.txt", b"keep\n".to_vec())],
            worktree_writes: vec![("new.txt", b"alpha\nbeta\n".to_vec())],
            index: vec![("new.txt", Some(b"alpha\nbeta\n".to_vec()))],
            ..Case::new(Op::StageFile("new.txt"))
        };

    stage_hunk_of_an_untracked_file_stages_it_whole:
        Case {
            base: vec![("keep.txt", b"keep\n".to_vec())],
            worktree_writes: vec![("new.txt", b"alpha\nbeta\n".to_vec())],
            index: vec![("new.txt", Some(b"alpha\nbeta\n".to_vec()))],
            ..Case::new(Op::StageHunk { path: "new.txt", hunk: 0 })
        };

    unstage_hunk_of_a_staged_new_file_removes_the_entry:
        Case {
            base: vec![("keep.txt", b"keep\n".to_vec())],
            stage_writes: vec![("new.txt", b"alpha\nbeta\n".to_vec())],
            index: vec![("new.txt", None)],
            ..Case::new(Op::UnstageHunk { path: "new.txt", hunk: 0 })
        };

    stage_file_of_a_deletion_removes_the_entry:
        Case {
            base: vec![("doomed.txt", b"gone\n".to_vec())],
            worktree_removals: vec!["doomed.txt"],
            index: vec![("doomed.txt", None)],
            ..Case::new(Op::StageFile("doomed.txt"))
        };

    stage_hunk_of_a_deletion_stages_it_whole:
        Case {
            base: vec![("doomed.txt", b"gone\n".to_vec())],
            worktree_removals: vec!["doomed.txt"],
            index: vec![("doomed.txt", None)],
            ..Case::new(Op::StageHunk { path: "doomed.txt", hunk: 0 })
        };

    unstage_file_restores_a_staged_deletion:
        Case {
            base: vec![("doomed.txt", b"gone\n".to_vec())],
            stage_removals: vec!["doomed.txt"],
            worktree_removals: vec!["doomed.txt"],
            index: vec![("doomed.txt", Some(b"gone\n".to_vec()))],
            ..Case::new(Op::UnstageFile("doomed.txt"))
        };

    // ——— empty-file edges ———

    stage_file_of_an_empty_untracked_file:
        Case {
            base: vec![("keep.txt", b"keep\n".to_vec())],
            worktree_writes: vec![("empty.txt", Vec::new())],
            index: vec![("empty.txt", Some(Vec::new()))],
            ..Case::new(Op::StageFile("empty.txt"))
        };

    stage_hunk_truncating_a_file_to_empty:
        Case {
            base: vec![("a.txt", b"alpha\nbeta\n".to_vec())],
            worktree_writes: vec![("a.txt", Vec::new())],
            index: vec![("a.txt", Some(Vec::new()))],
            ..Case::new(Op::StageHunk { path: "a.txt", hunk: 0 })
        };

    stage_hunk_filling_an_empty_file:
        Case {
            base: vec![("a.txt", Vec::new())],
            worktree_writes: vec![("a.txt", b"alpha\nbeta\n".to_vec())],
            index: vec![("a.txt", Some(b"alpha\nbeta\n".to_vec()))],
            ..Case::new(Op::StageHunk { path: "a.txt", hunk: 0 })
        };

    unstage_hunk_of_a_staged_truncation_restores_content:
        Case {
            base: vec![("a.txt", b"alpha\nbeta\n".to_vec())],
            stage_writes: vec![("a.txt", Vec::new())],
            index: vec![("a.txt", Some(b"alpha\nbeta\n".to_vec()))],
            ..Case::new(Op::UnstageHunk { path: "a.txt", hunk: 0 })
        };

    // ——— mode changes (unix: filemode is off on Windows) ———

    #[cfg(unix)]
    stage_file_stages_a_mode_change:
        Case {
            base: vec![("tool.sh", b"#!/bin/sh\n".to_vec())],
            worktree_exec: vec!["tool.sh"],
            index: vec![("tool.sh", Some(b"#!/bin/sh\n".to_vec()))],
            index_modes: vec![("tool.sh", 0o100755)],
            ..Case::new(Op::StageFile("tool.sh"))
        };

    #[cfg(unix)]
    unstage_file_restores_the_head_mode:
        Case {
            base: vec![("tool.sh", b"#!/bin/sh\n".to_vec())],
            stage_exec: vec!["tool.sh"],
            index: vec![("tool.sh", Some(b"#!/bin/sh\n".to_vec()))],
            index_modes: vec![("tool.sh", 0o100644)],
            ..Case::new(Op::UnstageFile("tool.sh"))
        };

    #[cfg(unix)]
    stage_file_stages_mode_and_content_together:
        Case {
            base: vec![("tool.sh", b"#!/bin/sh\n".to_vec())],
            worktree_writes: vec![("tool.sh", b"#!/bin/sh\nset -e\n".to_vec())],
            worktree_exec: vec!["tool.sh"],
            index: vec![("tool.sh", Some(b"#!/bin/sh\nset -e\n".to_vec()))],
            index_modes: vec![("tool.sh", 0o100755)],
            ..Case::new(Op::StageFile("tool.sh"))
        };

    // ——— non-UTF-8 text (the #25 heads-up: hunk strings are lossily
    // decoded; blobs must stay byte-verbatim) ———

    stage_hunk_latin1_bytes_verbatim:
        Case {
            base: vec![("latin1.txt", latin1(9, "\n", &[]))],
            worktree_writes: vec![("latin1.txt", latin1(9, "\n", &[(5, "edited")]))],
            index: vec![("latin1.txt", Some(latin1(9, "\n", &[(5, "edited")])))],
            ..Case::new(Op::StageHunk { path: "latin1.txt", hunk: 0 })
        };

    stage_hunk_latin1_bytes_verbatim_with_crlf:
        Case {
            base: vec![("latin1.txt", latin1(9, "\r\n", &[]))],
            worktree_writes: vec![("latin1.txt", latin1(9, "\r\n", &[(5, "edited")]))],
            index: vec![("latin1.txt", Some(latin1(9, "\r\n", &[(5, "edited")])))],
            ..Case::new(Op::StageHunk { path: "latin1.txt", hunk: 0 })
        };

    stage_hunk_latin1_bytes_verbatim_without_trailing_newline:
        Case {
            base: vec![("latin1.txt", latin1_no_trailing_newline(9, &[]))],
            worktree_writes: vec![("latin1.txt", latin1_no_trailing_newline(9, &[(5, "edited")]))],
            index: vec![(
                "latin1.txt",
                Some(latin1_no_trailing_newline(9, &[(5, "edited")])),
            )],
            ..Case::new(Op::StageHunk { path: "latin1.txt", hunk: 0 })
        };

    unstage_hunk_latin1_bytes_restores_the_base_verbatim:
        Case {
            base: vec![("latin1.txt", latin1(9, "\n", &[]))],
            stage_writes: vec![("latin1.txt", latin1(9, "\n", &[(5, "edited")]))],
            index: vec![("latin1.txt", Some(latin1(9, "\n", &[])))],
            ..Case::new(Op::UnstageHunk { path: "latin1.txt", hunk: 0 })
        };

    // ——— identical hunks (repeated code blocks, #26 heads-up) ———

    identical_hunks_three_copies_stage_the_middle_one:
        Case {
            base: vec![("a.txt", repeated_blocks(&["delta", "delta", "delta"]))],
            worktree_writes: vec![("a.txt", repeated_blocks(&["EDIT", "EDIT", "EDIT"]))],
            index: vec![("a.txt", Some(repeated_blocks(&["delta", "EDIT", "delta"])))],
            ..Case::new(Op::StageHunk { path: "a.txt", hunk: 1 })
        };

    identical_hunks_moved_copies_stage_at_the_fresh_position:
        Case {
            base: vec![("a.txt", repeated_blocks(&["delta", "delta"]))],
            worktree_writes: vec![("a.txt", repeated_blocks(&["EDIT", "EDIT"]))],
            // Two lines inserted at the top after the snapshot shift both
            // copies; the proximity tie-break must still pick the second.
            after_snapshot_writes: vec![("a.txt", {
                let mut bytes = b"inserted 1\ninserted 2\n".to_vec();
                bytes.extend_from_slice(&repeated_blocks(&["EDIT", "EDIT"]));
                bytes
            })],
            index: vec![("a.txt", Some(repeated_blocks(&["delta", "EDIT"])))],
            ..Case::new(Op::StageHunk { path: "a.txt", hunk: 1 })
        };

    // ——— cross-diff-base hunk boundaries (#26 heads-up): the op's
    // internal diff draws hunks against a different base than the
    // universe; staged content between two universe hunks must not widen
    // the apply beyond the hunk asked for ———

    staged_reverted_neighbour_does_not_widen_a_stage_to_the_next_hunk:
        Case {
            base: vec![("a.txt", lf(16, &[]))],
            // Line 8 staged, then reverted in the worktree: an index-only
            // hunk sits between the two worktree hunks, merging all three
            // regions in diff(index↔worktree).
            stage_writes: vec![("a.txt", lf(16, &[(8, "STAGED eight")]))],
            worktree_writes: vec![(
                "a.txt",
                lf(16, &[(4, "EDIT four"), (12, "EDIT twelve")]),
            )],
            index: vec![(
                "a.txt",
                Some(lf(16, &[(4, "EDIT four"), (8, "STAGED eight")])),
            )],
            ..Case::new(Op::StageHunk { path: "a.txt", hunk: 0 })
        };

    staged_reverted_neighbour_does_not_widen_a_stage_to_the_previous_hunk:
        Case {
            base: vec![("a.txt", lf(16, &[]))],
            stage_writes: vec![("a.txt", lf(16, &[(8, "STAGED eight")]))],
            worktree_writes: vec![(
                "a.txt",
                lf(16, &[(4, "EDIT four"), (12, "EDIT twelve")]),
            )],
            // The staged line-8 context overlaps both worktree hunks, so
            // the universe derives two staged-stale hunks; the second is
            // the line-12 one.
            index: vec![(
                "a.txt",
                Some(lf(16, &[(8, "STAGED eight"), (12, "EDIT twelve")])),
            )],
            ..Case::new(Op::StageHunk { path: "a.txt", hunk: 1 })
        };

    staged_reverted_neighbour_does_not_widen_an_unstage:
        Case {
            base: vec![("a.txt", lf(16, &[]))],
            // Everything staged, line 8 then reverted in the worktree:
            // diff(HEAD↔index) merges all three regions into one hunk.
            stage_writes: vec![(
                "a.txt",
                lf(16, &[(4, "EDIT four"), (8, "STAGED eight"), (12, "EDIT twelve")]),
            )],
            worktree_writes: vec![(
                "a.txt",
                lf(16, &[(4, "EDIT four"), (12, "EDIT twelve")]),
            )],
            index: vec![(
                "a.txt",
                Some(lf(16, &[(8, "STAGED eight"), (12, "EDIT twelve")])),
            )],
            ..Case::new(Op::UnstageHunk { path: "a.txt", hunk: 0 })
        };

    interleaved_staged_content_stages_as_the_one_merged_hunk_shown:
        Case {
            base: vec![("a.txt", lf(16, &[]))],
            // Edits at 5, 8, 11 merge into one universe hunk (staged-stale
            // around the staged line 8): staging it stages the region.
            stage_writes: vec![("a.txt", lf(16, &[(8, "STAGED eight")]))],
            worktree_writes: vec![(
                "a.txt",
                lf(16, &[(5, "EDIT five"), (8, "STAGED eight"), (11, "EDIT eleven")]),
            )],
            index: vec![(
                "a.txt",
                Some(lf(16, &[(5, "EDIT five"), (8, "STAGED eight"), (11, "EDIT eleven")])),
            )],
            ..Case::new(Op::StageHunk { path: "a.txt", hunk: 0 })
        };

    interleaved_staged_content_unstages_as_the_one_merged_hunk_shown:
        Case {
            base: vec![("a.txt", lf(16, &[]))],
            stage_writes: vec![("a.txt", lf(16, &[(8, "STAGED eight")]))],
            worktree_writes: vec![(
                "a.txt",
                lf(16, &[(5, "EDIT five"), (8, "STAGED eight"), (11, "EDIT eleven")]),
            )],
            index: vec![("a.txt", Some(lf(16, &[])))],
            ..Case::new(Op::UnstageHunk { path: "a.txt", hunk: 0 })
        };

    // ——— validate-at-apply ———

    a_stale_hunk_applies_nothing_and_notices:
        Case {
            base: vec![("a.txt", lf(9, &[]))],
            worktree_writes: vec![("a.txt", lf(9, &[(5, "first edit")]))],
            after_snapshot_writes: vec![("a.txt", lf(9, &[(5, "second edit")]))],
            index: vec![("a.txt", Some(lf(9, &[])))],
            stale: vec!["a.txt"],
            ..Case::new(Op::StageHunk { path: "a.txt", hunk: 0 })
        };
}
