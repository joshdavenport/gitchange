//! The shared addressing grammar (#122), resolved against a snapshot: how
//! a typed `<path>[:<hunk-id>]` argument becomes a repo-relative path and,
//! where one is named, the single hunk it addresses.
//!
//! `diff` is the first consumer (#158) and uses an ID as a selector with
//! validation; the mutating verbs inherit the same parse and the same
//! refusals, and decide only what to do with the hunk that comes back —
//! addressing is the language, refusals are the verb's.
//!
//! Every refusal here is exit `1`: these are repo-state answers, which
//! clap cannot see. Path offenders are collected and reported together, so
//! a caller fixing a command line learns all of its mistakes at once.

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use gitchange_core::{
    ChangeKind, ChangedFile, Hunk, HunkId, Snapshot, UNASSIGNED, conflicted_hint, holder_label,
};

/// One resolved `<path>[:<hunk-id>]` argument: the path in the spelling
/// every gitchange surface prints (repo-relative, `/`-separated), plus the
/// hunk selector where the argument carried one.
pub struct PathArg {
    /// Repo-relative, as core reports paths (#122).
    pub path: String,
    selector: Option<Selector>,
}

/// The `<hunk-id>` half of an address: a hex prefix of the ID and, where
/// identical hunks made it part of the address, the ordinal.
struct Selector {
    /// Lowercase hex, sigil stripped — [`HunkId::has_prefix`]'s input.
    hex: String,
    ordinal: Option<usize>,
    /// The suffix as typed, for refusals.
    typed: String,
}

impl PathArg {
    /// The hunk this argument addresses and the composed address it
    /// resolved to, `None` when it named a path alone. A not-found, stale,
    /// or ambiguous ID refuses — an aged address fails loud rather than
    /// resolving to whatever now sits at that position (#122), and the ID's
    /// own path prefix is a consistency guard, so an ID that lives in
    /// another file refuses naming it.
    ///
    /// The address comes back beside the hunk because the caller typed an
    /// abbreviation, or a prefix of one: what a receipt echoes and a
    /// refusal names is the composed form, minted here, so no caller has
    /// to re-derive it.
    pub fn resolve_hunk<'a>(
        &self,
        snapshot: &'a Snapshot,
    ) -> anyhow::Result<Option<(&'a Hunk, String)>> {
        let Some(selector) = &self.selector else {
            return Ok(None);
        };
        let matches = file_in(snapshot, &self.path)
            .map(|file| selector.matches_in(file))
            .unwrap_or_default();
        match matches.as_slice() {
            [(hunk, address)] => Ok(Some((hunk, address.clone()))),
            [] => anyhow::bail!(selector.nothing_found(snapshot, &self.path)),
            candidates => anyhow::bail!(
                "hunk '{}' in '{}' is ambiguous — name one of: {}",
                selector.typed,
                self.path,
                candidates
                    .iter()
                    .map(|(_, address)| address.clone())
                    .collect::<Vec<String>>()
                    .join(", ")
            ),
        }
    }
}

impl Selector {
    /// This selector's matches in `file`, each with its composed address —
    /// the string a refusal lists and a caller pastes back.
    fn matches_in<'a>(&self, file: &'a ChangedFile) -> Vec<(&'a Hunk, String)> {
        file.hunks
            .iter()
            .zip(file.hunk_addresses())
            .filter(|(_, address)| {
                address.id.has_prefix(&self.hex)
                    // An ordinal narrows; its absence accepts every hunk
                    // sharing the ID, which is what makes a shared address
                    // ambiguous rather than arbitrary.
                    && self.ordinal.is_none_or(|n| address.offset == Some(n))
            })
            .map(|(hunk, address)| (hunk, address.abbreviated_at(&file.path)))
            .collect()
    }

    /// The refusal for an ID that resolved to nothing at `path`: the
    /// consistency guard's, where the ID belongs to another file
    /// (`CONTEXT.md` §Hunk ID makes an ID repo-unique, so the wrong path
    /// is a caller mistake worth naming), and otherwise the staleness
    /// tripwire's — an address is snapshot-scoped, so "not here" and "not
    /// any more" are one answer, worded so a caller reads both.
    fn nothing_found(&self, snapshot: &Snapshot, path: &str) -> String {
        let elsewhere = snapshot
            .files
            .iter()
            .find(|file| file.path != path && !self.matches_in(file).is_empty());
        match elsewhere {
            Some(file) => format!(
                "hunk '{}' is in '{}', not '{path}' — an address is path-rooted",
                self.typed, file.path
            ),
            None => format!(
                "no hunk '{}' in '{path}' — an ID addresses one snapshot, so re-read \
                 it with 'gitchange diff {path}'",
                self.typed
            ),
        }
    }
}

/// The file `path` names in the snapshot, if it changed at all. Public
/// because the verbs ask it too: the change universe is where a path's
/// hunks live, and one lookup spelling keeps "not in the universe" one
/// answer.
pub fn file_in<'a>(snapshot: &'a Snapshot, path: &str) -> Option<&'a ChangedFile> {
    snapshot.files.iter().find(|file| file.path == path)
}

/// The hunk `value` addresses in `file`, with its composed address — the
/// other half of the shared addressing grammar (#122). The match rule is
/// core's ([`Hunk::contains_changed_text`]); what lives here is what the
/// counts mean.
///
/// Resolved over the file's whole universe, every owner included: the
/// caller checks its own scope on the unique match afterwards. A
/// scope-filtered search would make one value quietly mean different hunks
/// under different verbs, where this way it means one hunk or none (#145).
///
/// Zero or several matches refuse with candidate addresses listed: every
/// hunk of the file where nothing matched — a degenerate hunk has no
/// changed lines, so it can only ever be a candidate — and the matches
/// themselves where too many did.
pub fn resolve_containing<'a>(
    file: &'a ChangedFile,
    value: &str,
) -> Result<(&'a Hunk, String), String> {
    let addressed: Vec<(&Hunk, String)> = file
        .hunks
        .iter()
        .zip(file.hunk_addresses())
        .map(|(hunk, address)| (hunk, address.abbreviated_at(&file.path)))
        .collect();
    let matched: Vec<&(&Hunk, String)> = addressed
        .iter()
        .filter(|(hunk, _)| hunk.contains_changed_text(value))
        .collect();
    match matched.as_slice() {
        [(hunk, address)] => Ok((hunk, address.clone())),
        [] => Err(format!(
            "no changed line of '{}' contains '{value}' — its hunks are: {}",
            file.path,
            candidates(addressed.iter().map(|(_, address)| address.as_str()))
        )),
        several => Err(format!(
            "'{value}' is in {} hunks of '{}' — name one of: {}",
            several.len(),
            file.path,
            candidates(several.iter().map(|(_, address)| address.as_str()))
        )),
    }
}

/// Composed addresses as a refusal lists them, in file order.
fn candidates<'a>(addresses: impl Iterator<Item = &'a str>) -> String {
    addresses.collect::<Vec<&str>>().join(", ")
}

/// What one path argument narrows a mutating verb's scope to: the path
/// itself, the single hunk an address named, or — under `--containing
/// <line>` — the hunk holding that line. The dispatch between the three is
/// the shared addressing grammar's, so it lives here and every verb reads
/// it the same way (#145/#147):
///
/// - a **conflicted** path is quarantined before any of it (ADR 0007): it
///   holds no hunks to own or address, so every answer below would be a
///   confusing "nobody";
/// - `--containing` resolves over the path's **whole universe** and only
///   then reaches `hunk`, which applies the verb's own scope — a
///   scope-filtered search would make one value quietly mean different
///   hunks under different verbs;
/// - an address's own refusals (not found, stale, ambiguous — #122) come
///   back as offenders like any other, so an aged address joins the
///   caller's all-or-nothing list rather than bailing ahead of the
///   arguments beside it. That is what keeps a stale address from acting.
///
/// The verb supplies only its two answers: `whole` for a bare path — which
/// takes the path's entry in the change universe, `None` where it has none
/// — and `hunk` for one addressed hunk with the composed address a refusal
/// would name. `nothing_to_do` is the refusal a path with no changes at all
/// earns under `--containing`, where there is no universe to search.
pub fn narrow<'a, T>(
    arg: &PathArg,
    containing: Option<&str>,
    snapshot: &'a Snapshot,
    nothing_to_do: impl Fn(&str) -> String,
    whole: impl Fn(Option<&'a ChangedFile>) -> Result<T, String>,
    hunk: impl Fn(&'a Hunk, &str) -> Result<T, String>,
) -> Result<T, String> {
    let file = file_in(snapshot, &arg.path);
    if file.is_some_and(|file| file.kind == ChangeKind::Conflicted) {
        return Err(conflicted_hint(&arg.path));
    }
    if let Some(value) = containing {
        let file = file.ok_or_else(|| nothing_to_do(&arg.path))?;
        let (addressed, address) = resolve_containing(file, value)?;
        return hunk(addressed, &address);
    }
    match arg.resolve_hunk(snapshot) {
        Ok(None) => whole(file),
        Ok(Some((addressed, address))) => hunk(addressed, &address),
        Err(refusal) => Err(format!("{refusal:#}")),
    }
}

/// The example invocations a `--containing` grammar refusal quotes back —
/// the only part of [`check_containing`] that is the calling verb's own.
pub struct Forms {
    /// The verb's addressed form, e.g. `gitchange add <changelist>
    /// <path>:<hunk-id>`.
    pub addressed: String,
    /// The verb's `--containing` form, e.g. `gitchange add <changelist>
    /// <path> --containing <line>`.
    pub narrowed: String,
}

/// The three non-declarative exit-`2` checks every `--containing` verb owns
/// (#145/#147), asked of the raw arguments alone: they are grammar
/// violations, so they answer before the repository is opened and cannot be
/// confused with a refusal about repo state. The skeleton could not declare
/// them — clap sees neither a value's newlines, nor how many positionals an
/// option needs beside it, nor whether one of them is ID-shaped.
///
/// One body for the three verbs because the rules are one rule each: the
/// arity is *exactly one path* everywhere, and only what a caller can
/// mistype differs — `assign`'s grammar guarantees at least one path, so its
/// zero-path arm is unreachable rather than absent.
pub fn check_containing(
    value: Option<&str>,
    paths: &[String],
    forms: &Forms,
) -> anyhow::Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.contains('\n') {
        return Err(crate::usage(format!(
            "'--containing' matches within one changed line, and this value spans \
             several — name a line, or address the hunk: '{}'",
            forms.addressed
        )));
    }
    if paths.len() != 1 {
        return Err(crate::usage(format!(
            "'--containing' narrows exactly one path, and {} were given — '{}'",
            paths.len(),
            forms.narrowed
        )));
    }
    if let Some(token) = paths.iter().find(|token| carries_an_address(token)) {
        return Err(crate::usage(format!(
            "'--containing' and '<path>:<hunk-id>' are two ways to address one \
             hunk — '{token}' already names one, so drop one of them"
        )));
    }
    Ok(())
}

/// Whether `token` carries a `<hunk-id>` suffix — asked of the raw
/// argument, before any snapshot is read, because the question a grammar
/// check asks is which *addressing mode* was typed, not whether the
/// address resolves (#145). A shaped-but-too-short suffix counts: it was
/// meant as an address, which is the fact the check turns on.
pub fn carries_an_address(token: &str) -> bool {
    match split_address(token) {
        Ok((_, selector)) => selector.is_some(),
        Err(_) => true,
    }
}

/// Resolve every path argument against the repository, or refuse naming
/// all of the offenders at once (#122): cwd-relative in, repo-relative
/// out, literal — never a pathspec, so an unexpanded glob is an ordinary
/// not-found and the shell's own expansion feeds the variadic grammar.
///
/// A path that exists and simply has no changes resolves: a clean file is
/// an answer, and the verb decides what an empty selection means.
pub fn resolve_paths<'a>(
    tokens: impl IntoIterator<Item = &'a str>,
    snapshot: &Snapshot,
    workdir: &Path,
) -> anyhow::Result<Vec<PathArg>> {
    let (resolved, offenders) = locate_paths(tokens, snapshot, workdir);
    if !offenders.is_empty() {
        anyhow::bail!(refusal(&offenders));
    }
    Ok(resolved)
}

/// The all-or-nothing refusal: every offender in argument order, one line.
/// Beside [`locate_paths`], which is where offenders come from, so the
/// verbs that add classes of their own join the list the same way.
pub fn refusal(offenders: &[String]) -> String {
    offenders.join("; ")
}

/// The holders of `file`'s hunks that `keep` accepts, each named once in
/// file order, in the spelling every holder line uses (`None` is
/// unassigned). What a refusal prints when the answer to "whose hunks are
/// these?" is the retry the caller needs.
pub fn holders(file: &ChangedFile, keep: impl Fn(Option<&str>) -> bool) -> Vec<String> {
    let mut holders: Vec<String> = Vec::new();
    for hunk in &file.hunks {
        let owner = hunk.changelist.as_deref();
        if !keep(owner) {
            continue;
        }
        let label = holder_label(owner);
        if !holders.contains(&label) {
            holders.push(label);
        }
    }
    holders
}

/// [`resolve_paths`] with the refusal left to the caller: what resolved,
/// and what refused. For a verb with offender classes of its own to report
/// alongside — the mutating verbs' all-or-nothing validation reports every
/// offender at once (#145), so path resolution cannot be the one that
/// bails first.
pub fn locate_paths<'a>(
    tokens: impl IntoIterator<Item = &'a str>,
    snapshot: &Snapshot,
    workdir: &Path,
) -> (Vec<PathArg>, Vec<String>) {
    let mut resolved = Vec::new();
    let mut offenders = Vec::new();
    for token in tokens {
        match locate(token, snapshot, workdir) {
            Located::Found(arg) => resolved.push(arg),
            Located::Directory { prefix } => {
                offenders.push(directory_refusal(token, &prefix, snapshot));
            }
            Located::Outside => offenders.push(format!("'{token}' is outside the repository")),
            Located::Missing => offenders.push(format!("no such path '{token}'")),
            Located::Unaddressable(refusal) => offenders.push(refusal),
        }
    }
    (resolved, offenders)
}

/// The gh-borrowed error shape (#122): an unrecognised changelist refuses
/// with the valid ones listed, so a typo costs one round trip.
/// `unassigned` is among them — it is a legal scope everywhere a
/// changelist is named, not a changelist anyone created. Shared by every
/// verb that takes a changelist, so one repo answers one list.
pub fn changelist_scopes(snapshot: &Snapshot) -> String {
    // Each real name through `holder_label`, the one home for how a holder
    // is spelled (ADR 0006), so this list and the noun command's — which
    // reaches the same helper through core's own sentence — cannot come to
    // quote a changelist two ways. Unassigned leads, unquoted: it is not a
    // name anyone chose, which is what `holder_label(None)` says too.
    let names: Vec<String> = std::iter::once(UNASSIGNED.to_owned())
        .chain(
            snapshot
                .changelists
                .iter()
                .map(|changelist| holder_label(Some(&changelist.name))),
        )
        .collect();
    format!("the changelist scopes are: {}", names.join(", "))
}

/// What a token points at. One classification, read two ways — as the
/// argument itself ([`resolve_paths`]) and as the "is this a path?"
/// question `diff`'s token resolution asks ([`names_a_path`]) — so the two
/// readings cannot come to disagree about what a token is.
enum Located {
    Found(PathArg),
    /// A directory: it exists, but naming one is refused. `prefix` is its
    /// repo-relative path, for listing what is under it.
    Directory {
        prefix: String,
    },
    /// Outside the worktree.
    Outside,
    /// In neither the working tree nor the snapshot.
    Missing,
    /// A path carrying a suffix that was meant as an address and cannot
    /// serve as one, with the refusal saying why.
    Unaddressable(String),
}

fn locate(token: &str, snapshot: &Snapshot, workdir: &Path) -> Located {
    let (typed_path, selector) = match split_address(token) {
        Ok(split) => split,
        Err(refusal) => return Located::Unaddressable(refusal),
    };
    let absolute = normalize(&absolute(Path::new(typed_path)));
    let Some(path) = repo_relative(&absolute, workdir) else {
        return Located::Outside;
    };
    let on_disk = absolute.symlink_metadata();
    // A directory is refused with its changed files named, so the retry is
    // a copy-paste: gitchange offers no directory sweep, and silently
    // treating one as a path would be a sweep by accident (#122). The repo
    // root resolves to the empty path, which is the same refusal.
    if path.is_empty() || on_disk.as_ref().is_ok_and(|meta| meta.is_dir()) {
        return Located::Directory { prefix: path };
    }
    // Deleted files are changed files that no longer exist on disk, so the
    // snapshot is the second place a path may be found. Absent from both,
    // it is a typo — never a silent empty result.
    if on_disk.is_err() && file_in(snapshot, &path).is_none() {
        return Located::Missing;
    }
    Located::Found(PathArg { path, selector })
}

/// The directory refusal, naming what to type instead: the changed files
/// under it, in the snapshot's path order.
fn directory_refusal(token: &str, prefix: &str, snapshot: &Snapshot) -> String {
    let under: Vec<&str> = snapshot
        .files
        .iter()
        .map(|file| file.path.as_str())
        // The repo root, whose prefix is empty, is under-matched by every
        // path; a named directory matches its own children only.
        .filter(|path| prefix.is_empty() || path.starts_with(&format!("{prefix}/")))
        .collect();
    if under.is_empty() {
        return format!("'{token}' is a directory and holds no changed files");
    }
    format!(
        "'{token}' is a directory — name the changed files under it: {}",
        under.join(", ")
    )
}

/// Split `<path>:<hunk-id>` at the last colon, per the shared parse rule
/// (#122): every symbol is legal in a POSIX filename, so what prevents a
/// collision is the shape of the suffix, not the separator. A suffix that
/// is not ID-shaped leaves the whole token a path — which then lives or
/// dies as one, never as a silent misaddress.
fn split_address(token: &str) -> Result<(&str, Option<Selector>), String> {
    let Some((path, suffix)) = token.rsplit_once(':') else {
        return Ok((token, None));
    };
    match Selector::parse(suffix) {
        Some(selector) => Ok((path, Some(selector?))),
        None => Ok((token, None)),
    }
}

impl Selector {
    /// `<id>[/<n>]` with the sigil optional (#122), or `None` where the
    /// suffix is not ID-shaped at all. A shaped-but-short ID is the inner
    /// `Err`: it was meant as an address, so it refuses naming the
    /// minimum rather than degrading into a path nobody has.
    fn parse(suffix: &str) -> Option<Result<Self, String>> {
        let (id, ordinal) = match suffix.split_once('/') {
            Some((id, ordinal)) => (id, Some(ordinal.parse().ok()?)),
            None => (suffix, None),
        };
        let hex = id.strip_prefix(HunkId::SIGIL).unwrap_or(id);
        if hex.is_empty() || !hex.chars().all(|digit| digit.is_ascii_hexdigit()) {
            return None;
        }
        if hex.len() < HunkId::MIN_PREFIX_HEX {
            return Some(Err(format!(
                "hunk ID '{suffix}' is too short — an address needs at least {} characters",
                HunkId::MIN_PREFIX_HEX
            )));
        }
        Some(Ok(Self {
            hex: hex.to_ascii_lowercase(),
            ordinal,
            typed: suffix.to_owned(),
        }))
    }
}

/// Whether `token` reads as a path — the "is this a path?" half of
/// `diff`'s token resolution (#143), answered from the same [`locate`]
/// classification the argument itself resolves through, so one token
/// cannot be a path in one position and not in the other.
///
/// A directory reads as one: it exists, and the ambiguity git's `--` rule
/// cures is about existence, not about being addressable. So does a token
/// whose suffix was meant as an address — it was typed as a path, and
/// letting it through is what gets its own refusal said rather than a
/// vaguer "neither a changelist nor a path".
pub fn names_a_path(token: &str, snapshot: &Snapshot, workdir: &Path) -> bool {
    match locate(token, snapshot, workdir) {
        Located::Found(_) | Located::Directory { .. } | Located::Unaddressable(_) => true,
        Located::Outside | Located::Missing => false,
    }
}

/// A typed path against the caller's cwd — git's own grammar, and after
/// `-C` the cwd *is* `<dir>` (#139), so nothing here reads the flag.
fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_owned();
    }
    // A cwd that cannot be read is a repository that could not have been
    // discovered, so this is unreachable in practice; the relative path
    // resolving against nothing at all is the honest fallback.
    std::env::current_dir().unwrap_or_default().join(path)
}

/// `.` dropped and `..` popped, lexically. Lexical rather than
/// `canonicalize`, because a deleted file has no on-disk path to
/// canonicalize and refusing to resolve one would make `diff` unable to
/// name the deletions it prints.
fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component),
        }
    }
    normalized
}

/// `path` canonicalized as deep as the filesystem can take it, with
/// whatever hangs below that left exactly as it arrived.
///
/// What cannot be resolved stays lexical, for [`normalize`]'s reason: a
/// deleted file has no on-disk path to canonicalize, and it is still a path
/// of this worktree.
///
/// The last component alone is exempt when it is a symlink: git tracks a
/// link as a link, so an argument naming one means the link, not its
/// target. Symlinks *above* it do resolve, and must — a macOS worktree
/// under `/var` is reached through one, and resolving it is half of what
/// makes the two spellings meet.
fn canonicalized(path: &Path) -> PathBuf {
    let mut tail: Vec<&OsStr> = Vec::new();
    let mut current = path;
    loop {
        if !current.is_symlink()
            && let Ok(canonical) = current.canonicalize()
        {
            let mut resolved = canonical;
            resolved.extend(tail.iter().rev());
            return resolved;
        }
        let (Some(parent), Some(name)) = (current.parent(), current.file_name()) else {
            return path.to_owned();
        };
        tail.push(name);
        current = parent;
    }
}

/// `absolute` as the repo-relative, `/`-separated path every gitchange
/// surface prints (#122), or `None` where it escapes the worktree.
///
/// Both sides are canonicalized before they are compared (#181), because
/// they reach here from different sources and the same directory can carry
/// two spellings: the argument side is built on the cwd the process was
/// launched with, and `workdir` is libgit2's. The divergence CI confirmed
/// is the Windows 8.3 short name: one component of the cwd arrived aliased
/// (`C:\Users\RUNNER~1\…`) where libgit2 gave it in full
/// (`C:/Users/runneradmin/…`) — the same directory, and nothing a lexical
/// `strip_prefix` can see through, so every path inside the worktree read
/// as an escape. The separators differ too, harmlessly: Windows `Path`
/// reads `/` and `\` alike.
///
/// Canonicalized at the comparison rather than held in both spellings the
/// way core's watcher self-loop filter holds its roots: that filter answers
/// yes/no over roots it never has to name, where this must produce the one
/// repo-relative path a surface prints, and two spellings would mean
/// choosing between them.
fn repo_relative(absolute: &Path, workdir: &Path) -> Option<String> {
    // A worktree is a directory that exists, so the canonical pairing is
    // what runs. Where it somehow cannot be read, both sides drop back to
    // lexical together: a canonical argument against a lexical root is the
    // mismatch this function exists to cure, so the arms are all-or-nothing.
    let (root, resolved) = match workdir.canonicalize() {
        Ok(root) => (root, canonicalized(absolute)),
        Err(_) => (normalize(workdir), absolute.to_owned()),
    };
    let relative = resolved.strip_prefix(root).ok()?;
    let components: Vec<&str> = relative
        .components()
        .map(|component| component.as_os_str().to_str())
        .collect::<Option<Vec<&str>>>()?;
    Some(components.join("/"))
}
