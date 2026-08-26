//! `diff`'s scope resolution and its annotated text face (#158): the
//! changelist-scoped patch git cannot print.
//!
//! Selection and rendering are separate on purpose — the JSON face (#159)
//! renders the same [`select`] result through core's serialiser, and
//! ADR 0018's promise is that the two faces never disagree about which
//! files appear or in what order.

use std::collections::HashSet;
use std::path::Path;

use gitchange_core::{
    ALL, ChangeKind, ChangedFile, Hunk, HunkAddress, HunkIdentity, HunkLine, ModeDelta, Snapshot,
    UNASSIGNED, conflicted_hint, holder_label, target_named,
};

use crate::scope;

/// The files one `diff` invocation shows, in the snapshot's path order.
///
/// Every scope selects **whole file objects**, foreign hunks included: a
/// scope chooses which files appear, never which of a file's hunks do.
/// That is the TUI's dim-not-hide semantics made structural, and it keeps
/// the whole-file⇒unit-membership derivation trustworthy under any scope
/// (#143).
pub fn select<'a, 'tokens>(
    snapshot: &'a Snapshot,
    scope: ScopeToken<'tokens>,
    paths: impl IntoIterator<Item = &'tokens str>,
    workdir: &Path,
) -> anyhow::Result<Vec<&'a ChangedFile>> {
    let mut tokens: Vec<&str> = paths.into_iter().collect();
    let changelist = match resolve_token(scope, snapshot, workdir)? {
        Token::Absent => None,
        Token::Changelist(changelist) => Some(changelist),
        // Git's rule set again: a token that turned out to be a path is
        // simply the first of them, in argument order.
        Token::Path(token) => {
            tokens.insert(0, token);
            None
        }
    };
    let args = scope::resolve_paths(tokens, snapshot, workdir)?;
    // A `<path>:<hunk-id>` scope is a selector with validation, never hunk
    // narrowing (#143): the resolved hunk is discarded, and what the
    // address buys is the refusal when it has aged — the verification
    // read's staleness tripwire.
    for arg in &args {
        arg.resolve_hunk(snapshot)?;
    }
    let mut files = match changelist {
        Some(changelist) => snapshot.files_in(changelist),
        None => snapshot.files.iter().collect(),
    };
    if !args.is_empty() {
        // Paths union with each other and intersect the changelist at the
        // file level, which both fall out of filtering one ordered list.
        let named: HashSet<&str> = args.iter().map(|arg| arg.path.as_str()).collect();
        files.retain(|file| named.contains(file.path.as_str()));
    }
    Ok(files)
}

/// `diff`'s first positional as the command line left it: absent, or
/// present with the `--` boundary either spent on it or not. Composed by
/// the caller, because whether `--` was typed is an argument fact — and
/// the one the parse cannot carry, `diff <name> --` and `diff <name>`
/// parsing alike.
pub enum ScopeToken<'a> {
    Absent,
    /// No `--`: the token may be a changelist or a path, and git's rule
    /// set decides — or refuses.
    Ambiguous(&'a str),
    /// `--` was typed, so the slot before it is a changelist and nothing
    /// else — git's own disambiguation, in the changelist direction.
    Settled(&'a str),
}

/// What `diff`'s first pre-`--` token turned out to be.
enum Token<'a> {
    Absent,
    /// A changelist scope; `None` is unassigned, which is legal here.
    Changelist(Option<&'a str>),
    /// A path — the caller puts it back at the head of the path list.
    Path(&'a str),
}

/// Git's rule set for the ambiguous slot (#143): with no `--` to settle
/// it, a token matching both readings refuses naming `--`, and a token
/// matching neither refuses naming both readings with the changelist
/// candidates listed. These are exit `1`, not `2`: whether a name is a
/// changelist or a path is repo state, which clap cannot see.
fn resolve_token<'a>(
    scope: ScopeToken<'a>,
    snapshot: &Snapshot,
    workdir: &Path,
) -> anyhow::Result<Token<'a>> {
    let (token, settled) = match scope {
        ScopeToken::Absent => return Ok(Token::Absent),
        ScopeToken::Ambiguous(token) => (token, false),
        ScopeToken::Settled(token) => (token, true),
    };
    if token == ALL {
        // One scope, one spelling: bare `diff` is already the whole view,
        // so `all` would be a second name for it.
        anyhow::bail!("'{ALL}' is not a diff scope — bare 'gitchange diff' is already every hunk");
    }
    let changelist = token == UNASSIGNED
        || snapshot
            .changelists
            .iter()
            .any(|changelist| changelist.name == token);
    match (
        changelist,
        !settled && scope::names_a_path(token, snapshot, workdir),
    ) {
        (true, true) => anyhow::bail!(
            "'{token}' is both a changelist and a path — write \
             'gitchange diff {token} --' for the changelist, or \
             'gitchange diff -- {token}' for the path"
        ),
        (true, false) => Ok(Token::Changelist(target_named(token))),
        (false, true) => Ok(Token::Path(token)),
        (false, false) => anyhow::bail!(
            "'{token}' is neither a changelist nor a path — {}",
            scope::changelist_scopes(&snapshot.changelists)
        ),
    }
}

/// The annotated unified patch: git's patch format, files flat in path
/// order, with each hunk's owner, stage, and address riding the header's
/// function-context slot.
///
/// A display, not a contract (#143): `git apply`-ability is not promised
/// even where the annotation slot preserves it, and neither the annotation
/// text nor the line shape is a parsing contract — agents read `--json`.
pub fn print_patch(files: &[&ChangedFile]) {
    for file in files {
        print_file(file);
    }
}

fn print_file(file: &ChangedFile) {
    // git's own file header, which is where a reader looks for the path.
    println!("diff --git a/{path} b/{path}", path = file.path);
    if file.kind == ChangeKind::Conflicted {
        // Quarantined (ADR 0007): a conflicted path owns no hunks and is
        // not diffed, so the header carries one line saying so — stated,
        // never silently absent.
        println!("{}", conflicted_hint(&file.path));
        return;
    }
    // git's side lines introduce content, so they sit where git puts
    // them: after the header's mode lines, immediately above the first
    // `@@`. A file whose whole change is degenerate has no content to
    // introduce and so never grows them.
    let mut sides_printed = false;
    for (hunk, address) in file.hunks.iter().zip(file.hunk_addresses()) {
        let suffix = annotation(hunk, &address, &file.path);
        match &hunk.identity {
            HunkIdentity::Text { lines } => {
                if !sides_printed {
                    println!("--- {}", side(file, Side::Head));
                    println!("+++ {}", side(file, Side::Changed));
                    sides_printed = true;
                }
                println!("@@ {} {} @@ {suffix}", hunk.old_coords(), hunk.new_coords());
                for line in lines {
                    print_line(line);
                }
            }
            // Degenerate hunks have no coordinates to frame, so git's own
            // spelling for the change stands as the hunk's header. The
            // annotation rides its first line, as it rides a `@@` one.
            HunkIdentity::WholeFile { .. } => print_borrowed(whole_file_lines(file), &suffix),
            HunkIdentity::ModeChange => print_borrowed(mode_lines(file.mode_delta), &suffix),
        }
    }
}

/// The bracketed suffix every hunk carries: `['<changelist>' <glyph>
/// <path>:<id>]` — the owner in core's `holder_label` spelling, the
/// staging set's token, and the composed address with an abbreviated ID,
/// which is copyable straight into a verb (#122 accepts ≥ 7 characters).
fn annotation(hunk: &Hunk, address: &HunkAddress, path: &str) -> String {
    format!(
        "[{} {} {}]",
        holder_label(hunk.changelist.as_deref()),
        hunk.stage.glyph(),
        address.abbreviated_at(path)
    )
}

/// Which side of the diff a `---`/`+++` line names.
enum Side {
    Head,
    Changed,
}

/// A patch header side: git's `a/`/`b/` prefix, or `/dev/null` where the
/// file did not exist on that side.
fn side(file: &ChangedFile, side: Side) -> String {
    let (present, prefix) = match side {
        Side::Head => (
            !matches!(file.kind, ChangeKind::Added | ChangeKind::Untracked),
            'a',
        ),
        Side::Changed => (file.kind != ChangeKind::Deleted, 'b'),
    };
    if present {
        format!("{prefix}/{}", file.path)
    } else {
        "/dev/null".to_owned()
    }
}

/// A degenerate hunk's borrowed spelling, the annotation on its first
/// line.
fn print_borrowed(lines: Vec<String>, annotation: &str) {
    for (index, line) in lines.iter().enumerate() {
        match index {
            0 => println!("{line} {annotation}"),
            _ => println!("{line}"),
        }
    }
}

/// git's spelling for a whole-file hunk, by what the change *is*: `Binary
/// files … differ` for a binary (ADR 0009); for a type change the
/// `deleted file mode`/`new file mode` pair, which is what git prints for
/// one (it splits a type change into two file entries, and gitchange
/// presents it as one hunk, so the pair rides that hunk) — deliberately
/// *not* `old mode`/`new mode`, which is git's chmod spelling and would
/// call a symlink swap a permission flip (ADR 0017); and for an empty file
/// coming or going, git's file-header words without the mode, which the
/// snapshot carries only as a delta between two sides an empty add or
/// delete does not have.
///
/// These spellings are the CLI's own and are not shared with the TUI's
/// placeholder (`Binary file changed (12.4 KB → 15.1 KB)`): the two faces
/// answer different questions — a patch in git's grammar here, a sized
/// human summary there — so this is not the one-phrase-two-spellings drift
/// ADR 0006 sinks into core.
fn whole_file_lines(file: &ChangedFile) -> Vec<String> {
    if file.binary {
        return vec![format!(
            "Binary files {} and {} differ",
            side(file, Side::Head),
            side(file, Side::Changed)
        )];
    }
    // The file-level slot holds one delta (#112's gap), so a type change
    // beside a mode hunk reports no type delta here — the same silence the
    // wire's per-hunk `mode_delta` shows, rather than a second derivation
    // that could disagree with it.
    if let Some(ModeDelta::Type { before, after }) = file.mode_delta {
        return vec![
            format!("deleted file mode {before:o}"),
            format!("new file mode {after:o}"),
        ];
    }
    let sides = file.sides.as_ref();
    match (
        sides.and_then(|sides| sides.head.as_ref()),
        sides.and_then(|sides| sides.changed.as_ref()),
    ) {
        (None, Some(_)) => vec!["new file".to_owned()],
        (Some(_), None) => vec!["deleted file".to_owned()],
        // Both sides, no lines, and no delta to name: a change git reports
        // with nothing to say about it beyond that it exists.
        _ => vec!["whole file changed".to_owned()],
    }
}

/// git's chmod pair, octal as git prints it (ADR 0017) — the mode hunk's
/// spelling, and only its own flavour's: a mode hunk is a permission flip
/// by construction. A hunk that exists because the modes differ always has
/// its delta; naming the change without them still beats saying nothing.
fn mode_lines(delta: Option<ModeDelta>) -> Vec<String> {
    match delta {
        Some(ModeDelta::Mode { before, after }) => {
            vec![
                format!("old mode {before:o}"),
                format!("new mode {after:o}"),
            ]
        }
        Some(ModeDelta::Type { .. }) | None => vec!["mode changed".to_owned()],
    }
}

/// One diff line, verbatim behind its origin character. The no-newline
/// markers (`=`, `>`, `<`) carry git's own `\ No newline at end of file`
/// text as their content — led by the newline the line above was missing —
/// so they print as themselves, on their own line, rather than behind an
/// origin git never shows.
fn print_line(line: &HunkLine) {
    match line.origin {
        '=' | '>' | '<' => println!("{}", line.content.trim_matches('\n')),
        origin => println!("{origin}{}", line.content.trim_end_matches('\n')),
    }
}
