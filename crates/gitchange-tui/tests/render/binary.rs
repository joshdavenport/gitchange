//! Binary whole-file hunks (ADR 0009, issue #43): a sized placeholder
//! stands in for the diff body, and hunk mode is a no-op.

use gitchange_core::{
    BinarySides, BlobInfo, ChangeKind, ChangedFile, Hunk, HunkIdentity, HunkStage,
};
use ratatui::crossterm::event::KeyCode;

use gitchange_tui::app::App;
use gitchange_tui::theme::Theme;

use crate::helpers::{fg_at, find_text, key, render_buffer, snapshot, text_of};

/// A changed binary as core reports it: one whole-file hunk anchored on
/// the blob-OID pair, which `BinarySides` derives so the fixture can't
/// drift from ADR 0009's anchor. Sizes match the `binary` sandbox, so
/// what this renders is what gets eyeballed there.
fn binary_file(changelist: &str) -> ChangedFile {
    let sides = BinarySides {
        head: Some(BlobInfo {
            oid: "9ebcc32e".into(),
            size: 12_698,
        }),
        changed: Some(BlobInfo {
            oid: "0d4f9284".into(),
            size: 15_462,
        }),
    };
    ChangedFile {
        path: "assets/logo.png".into(),
        kind: ChangeKind::Modified,
        binary: true,
        hunks: vec![Hunk {
            old_start: 0,
            old_lines: 0,
            new_start: 0,
            new_lines: 0,
            stage: HunkStage::Unstaged,
            index_only: false,
            identity: HunkIdentity::WholeFile {
                oids: sides.oid_anchor(),
            },
            changelist: Some(changelist.to_owned()),
        }],
        binary_sides: Some(sides),
    }
}

#[test]
fn a_binary_renders_its_sized_placeholder_dim_with_no_hunk_rows() {
    let theme = Theme::default();
    let mut app = App::new("repo");
    let mut snap = snapshot();
    snap.files.push(binary_file("fixes"));
    app.apply_snapshot(snap);
    app.on_key(key(KeyCode::Char('3'))); // focus Files
    app.on_key(key(KeyCode::Char('j'))); // select the PNG

    let buffer = render_buffer(&app);
    let text = text_of(&buffer);

    // The whole-file hunk counts as one hunk in the row and the title.
    assert!(text.contains("assets/logo.png 0/1"), "{text}");
    assert!(text.contains("logo.png (0/1 staged)"), "{text}");
    // One dim placeholder line carrying both sizes; no diff body at all.
    let (x, y) = find_text(&buffer, "Binary file changed (12.4 KB → 15.1 KB)");
    assert_eq!(fg_at(&buffer, x, y), theme.colors.dim);
    assert!(
        !text.contains("@@ -0,0"),
        "no hunk header for a binary\n{text}"
    );

    // Hunk-mode entry is a deliberate no-op, so the frame it would have
    // changed — title, cursor column, tint — is byte-identical after.
    app.on_key(key(KeyCode::Enter));
    assert_eq!(render_buffer(&app), buffer, "enter redrew the binary");
}
