//! The ladder walk (#175, spec #125): one end-to-end test that the
//! escalation ladder's rungs *chain* — each refusal's text is the
//! instruction for the next rung, and the rung that finally acts carries no
//! flag the first one lacked (CONTEXT.md §Escalation ladder).
//!
//! The property this module owns is the one no single verb's test can see:
//! rung *n*'s output is enough to compose rung *n+1*'s command — the owner
//! named, the candidates listed, an aged address loud. Every assertion
//! below reads one rung's output as the *input* to the next, which is why
//! the aged address is here at all: `diff` owns that refusal's own
//! behaviour, and what this test adds is that climbing to the re-read is
//! worth the round trip, because an ID from an aged snapshot fails loud as
//! not-found instead of answering for whatever took its place (CONTEXT.md
//! §Hunk ID). The rungs' own refusals stay tested where they are specified
//! — the ownership guard and the `--containing` cardinality refusals in
//! `assign`, address validation in `diff`.
//!
//! Refusal prose is never pinned whole (#122: error text is not a parsing
//! contract). Each assertion below names the fragment that *is* the
//! instruction — the owner, the contested path, the candidate addresses —
//! so a reword that keeps them keeps this test green.

use std::path::Path;

use crate::support::{git, gitchange, hunks_of, initialised_repo, owners, seed_state_raw, write};

/// The other actor's two edits, at the ends of the file.
const HEADER: &str = "colleague header";
const FOOTNOTE: &str = "colleague footnote";

/// The value rung 2 narrows with: a distinctive line this agent wrote,
/// present in both of its hunks and in neither of the other actor's.
const MINE: &str = "retry budget";

/// A file long enough that four edits are four hunks: seven untouched lines
/// between each pair, so three lines of context each side never meet.
/// `support::long_file` is the same trick at two hunks, which this walk's
/// four-hunk fixture outgrows.
fn wide_file(edits: &[(usize, &str)]) -> String {
    let mut lines: Vec<String> = (1..=32).map(|line| format!("line {line}\n")).collect();
    for (line, text) in edits {
        lines[line - 1] = format!("{text}\n");
    }
    lines.concat()
}

/// The worktree as both actors leave it.
fn both_actors() -> String {
    wide_file(&[
        (1, HEADER),
        (9, "retry budget: 3"),
        (17, "retry budget: applied"),
        (25, FOOTNOTE),
    ])
}

/// The same worktree after the other actor reverts their footnote — the
/// concurrent change that ages an address this test has already minted.
fn footnote_reverted() -> String {
    wide_file(&[
        (1, HEADER),
        (9, "retry budget: 3"),
        (17, "retry budget: applied"),
    ])
}

/// Two actors' work in one file, capture off (`active: null`) — the posture
/// ADR 0015's contract has an agent keep, so ownership here is exactly what
/// the records say and no refresh of this walk's own claims anything:
///
/// - hunk 1 (`colleague header`) — the other actor's, recorded to
///   `'colleague-refactor'`: the foreign hunk rung 1 refuses on.
/// - hunks 2 and 3 (`retry budget: …`) — this agent's own, unassigned, and
///   the only two matching `MINE`: the multi-match rung 2 needs.
/// - hunk 4 (`colleague footnote`) — unassigned, exactly as this agent's
///   two are: nothing in state tells whose it is, and that is the shared
///   tree. What it is here for is being reverted by its author mid-walk,
///   which is what makes the address minted for it genuinely aged rather
///   than merely fabricated.
fn two_actor_repo() -> tempfile::TempDir {
    let dir = initialised_repo();
    let repo = dir.path();
    write(repo, "parser.rs", &wide_file(&[]));
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "--no-verify", "-m", "init"]);
    write(repo, "parser.rs", &both_actors());
    seed_state_raw(
        repo,
        r#"{
  "version": 1, "active": null,
  "changelists": [{ "name": "colleague-refactor" }, { "name": "retry-budget" }],
  "records": [
    {
      "path": "parser.rs", "old_start": 1, "old_lines": 4,
      "new_start": 1, "new_lines": 4, "changelist": "colleague-refactor",
      "anchor": ["-line 1\n", "+colleague header\n"], "dormant_since": null
    }
  ]
}"#,
    );
    dir
}

#[test]
fn the_rungs_chain_from_a_contested_path_to_an_addressed_assign() {
    let dir = two_actor_repo();
    let repo = dir.path();

    // Rung 1 — the happy path: one command, zero reads. It refuses, and the
    // refusal is the instruction: it names the owner, and it names the
    // contested scope as the whole path, which is what makes narrowing by a
    // line the evident next move rather than a guess.
    let contested = assign_refusal(repo, &["parser.rs", "--to", "retry-budget"]);
    assert!(
        contested.contains("'colleague-refactor'"),
        "the owner is named: {contested}"
    );
    assert!(
        contested.contains("'parser.rs'"),
        "the contested scope is named: {contested}"
    );

    // Rung 2 — narrowed by a line this agent wrote. Two of its own hunks
    // carry the value, so the answer is a candidate list, and the list is
    // itself the resolution: the retry addresses these, and no flag exists
    // to broaden past them.
    let several = assign_refusal(
        repo,
        &["parser.rs", "--containing", MINE, "--to", "retry-budget"],
    );
    let candidates = candidates_in(&several, "parser.rs");
    assert_eq!(
        candidates.len(),
        2,
        "the refusal lists both matches: {several}"
    );

    // Rung 3 — ground truth. The cheap inventory read carries `id` and
    // `offset`, never a composed address, so the caller composes: each
    // abbreviation rung 2 printed picks out exactly one hunk of the re-read.
    let addresses = composed(&inventory(repo, "parser.rs"), "parser.rs");
    assert_eq!(
        addresses.len(),
        4,
        "the fixture's four hunks: {addresses:?}"
    );
    for candidate in &candidates {
        let matched = addresses
            .iter()
            .filter(|address| address.starts_with(candidate.as_str()))
            .count();
        assert_eq!(
            matched, 1,
            "'{candidate}' resolves against the re-read: {addresses:?}"
        );
    }

    // Still rung 3, and what makes it worth the round trip: an address goes
    // aged without warning — here the other actor reverts the hunk this one
    // names — and the scoped read says so rather than answering for whatever
    // took those lines' place. Nothing acts on aged content.
    let aged = addresses[3].clone();
    assert_eq!(
        gitchange(repo, &["diff", "--json", &aged]).status.code(),
        Some(0),
        "the address resolves while its hunk is there"
    );
    write(repo, "parser.rs", &footnote_reverted());
    let not_found = gitchange(repo, &["diff", "--json", &aged]);
    assert_eq!(
        not_found.status.code(),
        Some(1),
        "an aged address is not found"
    );
    assert_eq!(
        not_found.stdout,
        Vec::<u8>::new(),
        "the not-found lands before any content: no patch, no envelope"
    );
    assert!(
        String::from_utf8_lossy(&not_found.stderr).contains("no hunk"),
        "the refusal says the address resolves to nothing: {}",
        String::from_utf8_lossy(&not_found.stderr)
    );

    // Rung 4 — the retry addresses the listed IDs, pasted verbatim, and
    // carries no flag rung 1 lacked: the ladder climbs to ground truth, not
    // around to an override.
    let retry = [
        candidates[0].as_str(),
        candidates[1].as_str(),
        "--to",
        "retry-budget",
    ];
    let output = gitchange(repo, &[&["assign"], retry.as_slice()].concat());
    assert_eq!(
        output.status.code(),
        Some(0),
        "gitchange assign {retry:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let echo = String::from_utf8(output.stdout).unwrap();
    assert!(echo.contains("assigned 2 hunks"), "{echo}");
    assert_eq!(
        owners(repo, "parser.rs"),
        vec![
            Some("colleague-refactor".to_owned()),
            Some("retry-budget".to_owned()),
            Some("retry-budget".to_owned()),
        ],
        "exactly the addressed hunks moved; the other actor's is untouched"
    );
}

/// An `assign` that must refuse: exit `1`, stdout empty (#122), and the
/// stderr text handed back so the caller can read the next rung out of it.
fn assign_refusal(dir: &Path, args: &[&str]) -> String {
    let output = gitchange(dir, &[&["assign"], args].concat());
    assert_eq!(
        output.status.code(),
        Some(1),
        "gitchange assign {args:?} should refuse"
    );
    assert_eq!(
        output.stdout,
        Vec::<u8>::new(),
        "a failed command leaves stdout empty (#122)"
    );
    String::from_utf8(output.stderr).unwrap()
}

/// The cheap inventory read's document for one path — `diff --json
/// --no-content`, the hot-loop read the re-read rung climbs to, its run
/// asserted to have succeeded.
fn inventory(dir: &Path, path: &str) -> serde_json::Value {
    let args = ["diff", "--json", "--no-content", path];
    let output = gitchange(dir, &args);
    assert_eq!(
        output.status.code(),
        Some(0),
        "gitchange {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("the envelope is one JSON document")
}

/// Every hunk of `path` as a composed address, in file order. The wire
/// carries `id` and `offset` and never the composition, so this is the
/// composition rule itself — `<path>:<id>`, with `/<n>` appended exactly
/// where `offset` is non-null — exercised against the real document.
fn composed(envelope: &serde_json::Value, path: &str) -> Vec<String> {
    hunks_of(envelope, path)
        .iter()
        .map(|hunk| {
            let id = hunk["id"].as_str().expect("id");
            match hunk["offset"].as_u64() {
                Some(offset) => format!("{path}:{id}/{offset}"),
                None => format!("{path}:{id}"),
            }
        })
        .collect()
}

/// The composed addresses a refusal lists, in the order it printed them.
/// Nothing here edits a candidate beyond dropping the `, ` join's comma:
/// what the next rung pastes has to be what the refusal printed.
fn candidates_in(refusal: &str, path: &str) -> Vec<String> {
    let prefix = format!("{path}:h");
    refusal
        .split_whitespace()
        .filter(|token| token.starts_with(&prefix))
        .map(|token| token.trim_end_matches(',').to_owned())
        .collect()
}
