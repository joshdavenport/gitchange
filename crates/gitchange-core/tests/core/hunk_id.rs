//! The hunk ID (`CONTEXT.md` §Hunk ID, foundations #122): the
//! snapshot-scoped address every hunk-addressing verb speaks — a hash of
//! the path plus the content anchor, rendered `h` + 64 lowercase hex,
//! identical hunks in one file told apart by a file-order ordinal.
//! Asserted through `Repo::read_only_refresh()` and
//! `ChangedFile::hunk_addresses()` per ADR 0008.

use crate::support::RepoFixture;
use gitchange_core::{HunkAddress, Repo};

/// The addresses of `path`'s hunks as a fresh read-only refresh derives
/// them, in file order.
fn addresses_of(fixture: &RepoFixture, path: &str) -> Vec<HunkAddress> {
    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.read_only_refresh().unwrap();
    snapshot
        .files
        .iter()
        .find(|file| file.path == path)
        .unwrap_or_else(|| panic!("{path} in the snapshot"))
        .hunk_addresses()
}

/// Whether `rendered` is the `h` sigil followed by exactly 64 lowercase
/// hex digits — the whole ID, as the wire carries it.
fn is_sigil_plus_64_hex(rendered: &str) -> bool {
    let Some(hex) = rendered.strip_prefix('h') else {
        return false;
    };
    hex.len() == 64
        && hex
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

#[test]
fn a_hunk_renders_as_the_h_sigil_plus_64_lowercase_hex_and_a_unique_one_has_no_offset() {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", "one\ntwo\nthree\n")
        .commit_all("init")
        .write("a.txt", "one\ntwo edited\nthree\n");

    let addresses = addresses_of(&fixture, "a.txt");

    assert_eq!(addresses.len(), 1);
    let rendered = addresses[0].id.to_string();
    assert!(is_sigil_plus_64_hex(&rendered), "{rendered}");
    assert_eq!(addresses[0].offset, None, "a unique hunk needs no ordinal");
}

/// Seven distinct lines, repeated: an edit to the same line of two
/// non-adjacent blocks is two hunks with byte-identical anchors.
const BLOCK: &str = "alpha\nbeta\ngamma\ndelta\nepsilon\nzeta\neta\n";

#[test]
fn identical_hunks_in_a_file_share_a_base_id_and_take_file_order_ordinals() {
    // The ID hashes path plus anchor, so two hunks whose anchors agree
    // hash alike; the ordinal is what tells them apart, counted from
    // `/0` in file order.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &BLOCK.repeat(3))
        .commit_all("init")
        .write(
            "a.txt",
            &[
                BLOCK.replace("delta", "DELTA"),
                BLOCK.to_owned(),
                BLOCK.replace("delta", "DELTA"),
            ]
            .concat(),
        );

    let addresses = addresses_of(&fixture, "a.txt");

    assert_eq!(addresses.len(), 2, "two hunks, one block of context apart");
    assert_eq!(addresses[0].id, addresses[1].id);
    assert_eq!(addresses[0].offset, Some(0));
    assert_eq!(addresses[1].offset, Some(1));
}

#[test]
#[cfg(unix)]
fn a_chmodded_binarys_mode_and_whole_file_hunks_take_distinct_ids() {
    // Neither degenerate flavour has an anchor to hash, so each hashes
    // its own domain tag in the anchor's place — the same path yields
    // two IDs, and neither needs an ordinal.
    let fixture = RepoFixture::new();
    fixture
        .write_bytes("blob.bin", &[0u8, 1, 2, 3])
        .commit_all("init")
        .write_bytes("blob.bin", &[0u8, 9, 9, 9, 9])
        .set_exec("blob.bin");

    let addresses = addresses_of(&fixture, "blob.bin");

    assert_eq!(addresses.len(), 2, "the mode hunk and the whole-file hunk");
    assert_ne!(addresses[0].id, addresses[1].id);
    assert_eq!(
        (addresses[0].offset, addresses[1].offset),
        (None, None),
        "distinct IDs, so no ordinals"
    );
}

#[test]
fn a_whole_file_hunks_id_survives_a_rewrite_of_the_files_bytes() {
    // The whole file *is* the hunk (ADR 0009), so its address is path
    // continuity alone: the domain tag and the path are hashed, the blob
    // OIDs deliberately are not — a re-export of the binary keeps the
    // address an agent already holds.
    let fixture = RepoFixture::new();
    fixture
        .write_bytes("blob.bin", &[0u8, 1, 2, 3])
        .commit_all("init")
        .write_bytes("blob.bin", &[0u8, 9, 9, 9, 9]);
    let before = addresses_of(&fixture, "blob.bin");

    fixture.write_bytes("blob.bin", &[0u8, 7, 7]);
    let after = addresses_of(&fixture, "blob.bin");

    assert_eq!(after, before);
}

#[test]
fn the_same_content_in_another_file_yields_a_different_id() {
    // The path is in the hash: an ID is repo-unique, not merely
    // file-unique, so a pasted address can never resolve in the wrong
    // file.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", "one\ntwo\nthree\n")
        .write("b.txt", "one\ntwo\nthree\n")
        .commit_all("init")
        .write("a.txt", "one\ntwo edited\nthree\n")
        .write("b.txt", "one\ntwo edited\nthree\n");

    let a = addresses_of(&fixture, "a.txt");
    let b = addresses_of(&fixture, "b.txt");

    assert_ne!(a[0].id, b[0].id);
}

#[test]
fn an_unrelated_edit_elsewhere_in_the_file_leaves_a_hunks_id_unchanged() {
    // The anchor is the hunk's own lines, not its coordinates or the
    // file's content, so a second hunk appearing below it does not move
    // its address — the property that makes an ID copyable across the
    // assign-as-you-go loop.
    let fixture = RepoFixture::new();
    let numbered = |first: &str, last: &str| {
        let mut lines: Vec<String> = (1..=20).map(|n| format!("line {n}\n")).collect();
        lines[0] = format!("{first}\n");
        lines[19] = format!("{last}\n");
        lines.concat()
    };
    fixture
        .write("a.txt", &numbered("line 1", "line 20"))
        .commit_all("init")
        .write("a.txt", &numbered("first edited", "line 20"));
    let before = addresses_of(&fixture, "a.txt");

    fixture.write("a.txt", &numbered("first edited", "last edited"));
    let after = addresses_of(&fixture, "a.txt");

    assert_eq!(before.len(), 1);
    assert_eq!(after.len(), 2, "the unrelated edit is its own hunk");
    assert_eq!(after[0].id, before[0].id);
    assert_ne!(after[1].id, before[0].id);
}
