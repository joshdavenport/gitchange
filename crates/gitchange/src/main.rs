use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use clap::builder::NonEmptyStringValueParser;
use clap::{ArgAction, ArgGroup, Args, Parser, Subcommand};

use gitchange_core::{
    ACTIVE_MARKER, Advisory, ChangedFile, GroupKind, LockHolder, OpOutcome, Repo, target_named,
};

/// The prefix every diagnostic this binary writes to stderr carries, so
/// output piped alongside git's own is attributable at a glance.
const DIAG: &str = "gitchange:";

/// How long a mutating command absorbs lock contention before surfacing
/// it. A shape rather than a tuned constant — long enough that a live
/// TUI's hold across one write never reaches the caller, short enough
/// that persistent contention is reported instead of waited out. No flag
/// tunes it: a `--lock-timeout` would tune around contention rather than
/// surface it, and retry is neither a guard nor an override (#122).
const LOCK_RETRY_BUDGET: Duration = Duration::from_secs(2);

/// The gap between attempts, so the common case — a hold measured in
/// milliseconds — costs one short sleep rather than a fixed penalty.
const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(50);

/// An operational refusal: the text names what to do before retrying,
/// and stdout stays empty. The failure half of the published exit-code
/// scheme (#122) with [`TRANSIENT`]; success is `0` and a usage error is
/// clap's own `2`, neither of which reaches the error mapping.
const REFUSAL: u8 = 1;

/// Transient lock contention: retry the same command unchanged.
const TRANSIENT: u8 = 3;

/// The capture-pending hint (#143): what `status` says under the
/// unassigned group when a real changelist is active and unassigned holds
/// hunks — the one state where the read's ownership and the next
/// persisting refresh's will differ. Derived from record facts alone (the
/// marker, the absence of records), so a read may state it. It names the
/// mechanism and the resolution, never a destination (#122 §Forecasts):
/// where a hunk lands is context-derived — an intervening `switch` moves
/// it, the entry-unit rule overrides it hunk by hunk — so a named landing
/// spot is either wrong or the preview of context-derived ownership the
/// refresh split forbids (ADR 0005). The claiming refresh's receipt
/// reports where hunks actually went, once.
const CAPTURE_PENDING_HINT: &str =
    "capture on: run 'gitchange refresh' to claim these, or they're claimed at your next action";

/// What exit `3` tells the caller to do, in words: the code is the
/// branching surface, and this line is the same contract for a human
/// reading the terminal. Composed here rather than in core because the
/// exit-code scheme is the binary's, not the library's — and because
/// core's own text, which stops at "retry in a moment", has no command
/// to name. It promises nothing about what was written: the resolution
/// is the same invocation, and only that.
const RETRY_UNCHANGED: &str = "run the same command again, unchanged";

#[derive(Parser)]
// `name` is left to clap, which takes it from the package name.
#[command(version, about = "Organise uncommitted changes into changelists")]
struct Cli {
    /// Run as if gitchange were launched in that directory
    // The help text says "that directory" rather than repeating `<dir>`,
    // which clap already prints from `value_name` — and which rustdoc reads
    // as an unclosed HTML tag, this being a doc comment as well as help.
    //
    // Git's short, the only spelling git has; single occurrence (clap's
    // default for a `Set` option), so git's repeatable-composing form is
    // not borrowed. Honoured by `enter`, before any command runs.
    #[arg(short = 'C', value_name = "dir", global = true)]
    dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// List changelists and changed files in the current repository
    Status {
        /// Emit the context envelope as JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// Set the active changelist
    Switch {
        /// Name of the changelist to make active, or `unassigned` to
        /// capture nothing
        name: String,
    },
    /// Run a persisting refresh now: capture new hunks into the active
    /// changelist and report each decision
    Refresh,
    /// List changelists, or create, delete, or rename one
    // `git branch`'s grammar wholesale, one mode per invocation: every
    // pairwise conflict is declared here so clap raises it as exit 2.
    #[command(group(ArgGroup::new("delete_mode").args(["delete", "force_delete"])))]
    Changelist {
        /// Create a changelist with this name
        #[arg(
            value_name = "name",
            conflicts_with_all = ["delete", "force_delete", "force", "rename"]
        )]
        name: Option<String>,
        /// Delete these changelists; refuses while any holds records
        // Unlike git's, `-d` and `-D` each carry their own value list, so
        // git's tolerance of the redundant pair does not map: combining
        // them is a usage error.
        #[arg(
            short = 'd',
            long = "delete",
            value_name = "name",
            num_args = 1..,
            conflicts_with = "force_delete"
        )]
        delete: Vec<String>,
        /// Delete these changelists, releasing their records (`--delete --force`)
        #[arg(short = 'D', value_name = "name", num_args = 1..)]
        force_delete: Vec<String>,
        /// Release records instead of refusing; legal-redundant beside `-D`
        #[arg(short = 'f', long, requires = "delete_mode")]
        force: bool,
        /// Rename a changelist, rewriting its records
        // Exactly two values: git's one-arg rename-the-active form dies
        // declaratively as wrong arity. `Set`, so a second `-m` is an error.
        #[arg(
            short = 'm',
            long = "move",
            value_names = ["old", "new"],
            num_args = 2,
            action = ArgAction::Set,
            conflicts_with_all = ["delete", "force_delete", "force"]
        )]
        rename: Option<Vec<String>>,
    },
    /// Assign hunks to a changelist, or release them to unassigned
    #[command(group(ArgGroup::new("target").required(true)))]
    Assign {
        /// Paths, or single hunks as `<path>:<hunk-id>`
        #[arg(value_name = "path[:hunk-id]", required = true)]
        paths: Vec<String>,
        #[command(flatten)]
        containing: Containing,
        /// Take hunks other changelists own, not just unassigned ones
        #[arg(long)]
        take_owned: bool,
        /// The changelist to own the hunks
        #[arg(long, value_name = "changelist", group = "target")]
        to: Option<String>,
        /// Release the hunks to unassigned
        #[arg(long, group = "target")]
        unassign: bool,
    },
    /// Stage a changelist's hunks: all of them, or the addressed ones
    // The alias is vocabulary — git's staging verb — not an abbreviation,
    // so it shows in help.
    #[command(visible_alias = "stage")]
    Add {
        #[command(flatten)]
        scope: StagingScope,
    },
    /// Unstage a changelist's hunks: all of them, or the addressed ones
    Unstage {
        #[command(flatten)]
        scope: StagingScope,
    },
    /// Commit a changelist's staged hunks, each guard's override named
    // There is no editor, so no default message source exists: exactly
    // one of `-m`, `-F`, `--no-edit` is required.
    #[command(group(ArgGroup::new("message_source").required(true)))]
    Commit {
        /// The changelist whose staged hunks form the commit payload
        #[arg(value_name = "changelist")]
        changelist: String,
        /// The commit message; repeatable, as git's
        #[arg(
            short,
            long,
            value_name = "message",
            allow_hyphen_values = true,
            group = "message_source"
        )]
        message: Vec<String>,
        /// Read the commit message from this file (`-` for stdin)
        #[arg(short = 'F', long, value_name = "file", group = "message_source")]
        file: Option<String>,
        /// Keep the message being amended (amend only)
        #[arg(long, group = "message_source", requires = "amend")]
        no_edit: bool,
        /// Amend the changelist's own last commit
        #[arg(long)]
        amend: bool,
        /// Bypass the pre-commit and commit-msg hooks
        #[arg(short = 'n', long)]
        no_verify: bool,
        /// Commit even while unassigned hunks are staged
        #[arg(long)]
        allow_unassigned: bool,
        /// Commit even while staged-stale hunks are in the payload
        #[arg(long)]
        allow_staged_stale: bool,
        /// Amend even though HEAD is not the changelist's own last commit
        #[arg(long)]
        allow_foreign_head: bool,
    },
    /// Show a changelist's hunks, annotated with owner and hunk ID — the
    /// patch git cannot print
    Diff {
        #[command(flatten)]
        scope: DiffScope,
        /// Emit the hunk envelope as JSON instead of text
        #[arg(long)]
        json: bool,
        /// Omit hunk content from the JSON envelope
        #[arg(long, requires = "json")]
        no_content: bool,
    },
    /// git's `restore`, which gitchange does not have — see `unstage`
    // Hidden from help and completions; never executes (#140). Both git
    // spellings parse so each can be corrected: `--staged` routes to
    // `unstage`; bare, to raw `git restore`. `rest` swallows any other
    // git flag (`--source=…`, `--worktree`) in any position — including a
    // `--staged` that follows a path, since once the positional is
    // collecting, clap takes hyphen tokens as values;
    // `Command::restore_staged` reads both places.
    #[command(hide = true)]
    Restore {
        /// git's index := HEAD spelling; gitchange's verb is `unstage`
        #[arg(long)]
        staged: bool,
        /// Whatever else git's `restore` would take; ignored
        #[arg(value_name = "git-restore-args", allow_hyphen_values = true)]
        rest: Vec<String>,
    },
}

impl Command {
    /// Whether a `restore` invocation carried `--staged` anywhere — as
    /// the parsed flag, or swallowed into `rest` after a path (`git
    /// restore src/a.rs --staged` is valid git, so both orders must
    /// correct to `unstage`).
    fn restore_staged(staged: bool, rest: &[String]) -> bool {
        staged || rest.iter().any(|arg| arg == "--staged")
    }
}

/// `add`'s grammar, which `unstage` carries verbatim: one symmetric
/// vocabulary with the direction in the verb.
#[derive(Args)]
struct StagingScope {
    /// The changelist whose hunks to act on
    #[arg(value_name = "changelist")]
    changelist: String,
    /// Narrow to these paths, or to single hunks as `<path>:<hunk-id>`
    #[arg(value_name = "path[:hunk-id]")]
    paths: Vec<String>,
    #[command(flatten)]
    containing: Containing,
}

/// `--containing <line>`, declared once so the three verbs that address
/// a hunk by its text cannot drift: single occurrence (clap's default for
/// a `Set` option), non-empty, and a leading hyphen allowed because a
/// changed line may begin with `-`.
#[derive(Args)]
struct Containing {
    /// Narrow one path to the hunk whose changed lines contain this text
    #[arg(
        long,
        value_name = "line",
        allow_hyphen_values = true,
        value_parser = NonEmptyStringValueParser::new()
    )]
    containing: Option<String>,
}

/// `diff`'s positionals, the `--` boundary preserved: the first pre-`--`
/// token (changelist or path — the read spec resolves which) and the path
/// list, where `--` forces every following token into the paths and
/// leaves the scope slot empty. Clap carries the boundary itself: the
/// `last` positional is reachable only after `--`, and the parser jumps
/// straight to it when `--` appears, so `diff -- x` never fills `scope`.
#[derive(Args)]
struct DiffScope {
    /// A changelist, or the first path
    #[arg(value_name = "changelist")]
    scope: Option<String>,
    /// Paths, or single hunks as `<path>:<hunk-id>`
    #[arg(value_name = "path[:hunk-id]")]
    paths: Vec<String>,
    /// Paths after `--`; concatenated onto `paths` by `DiffScope::paths`
    #[arg(value_name = "path[:hunk-id]", last = true, hide = true)]
    trailing_paths: Vec<String>,
}

impl DiffScope {
    /// The full path list in argument order, `--` boundary spent.
    #[allow(dead_code)] // read by the diff handler (#158)
    fn paths(&self) -> impl Iterator<Item = &str> {
        self.paths
            .iter()
            .chain(&self.trailing_paths)
            .map(String::as_str)
    }
}

fn main() -> ExitCode {
    // Usage errors exit with code 2 via clap; everything else is mapped
    // from the error by `report_failure`.
    let cli = Cli::parse();
    // The hidden `restore` correction is a usage error, not an operation,
    // so it exits before `run` — and so before `-C` is honoured, as a
    // usage error would with any other command.
    if let Some(Command::Restore { staged, rest }) = &cli.command {
        return restore(Command::restore_staged(*staged, rest));
    }
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => ExitCode::from(report_failure(&err)),
    }
}

/// Everything after a successful parse: take up `-C`'s directory, then
/// dispatch. `-C` goes first because it is the environment the command
/// runs in, not a step of the command, so a stub refuses on the directory
/// before it can refuse as not-implemented. Only the parse itself comes
/// earlier: usage errors, and clap's `--help`/`--version`, answer without
/// entering anything (where git would fail on the directory first).
fn run(cli: Cli) -> anyhow::Result<()> {
    if let Some(dir) = &cli.dir {
        enter(dir)?;
    }
    match cli.command {
        // The JSON face refuses before the built text path runs: `--json`
        // must never fall back to text, since exit 0 on a `--json` call
        // promises the envelope was delivered.
        Some(Command::Status { json: true }) => not_implemented("status --json"),
        // A read: no lock, so no contention to absorb (#122).
        Some(Command::Status { json: false }) => status(),
        Some(Command::Switch { name }) => with_lock_retry(|| switch(&name)),
        Some(Command::Refresh) => not_implemented("refresh"),
        Some(Command::Changelist { .. }) => not_implemented("changelist"),
        Some(Command::Assign { .. }) => not_implemented("assign"),
        Some(Command::Add { .. }) => not_implemented("add"),
        Some(Command::Unstage { .. }) => not_implemented("unstage"),
        Some(Command::Commit { .. }) => not_implemented("commit"),
        Some(Command::Diff { .. }) => not_implemented("diff"),
        Some(Command::Restore { .. }) => unreachable!("handled in main"),
        None => gitchange_tui::run().map_err(anyhow::Error::from),
    }
}

/// `-C <dir>`, implemented as git implements it: change the process's
/// working directory. "Run as if launched in `<dir>`" (#122) is then
/// literally true rather than emulated — repo discovery, the TUI's own
/// `current_dir` read, and every cwd-relative argument a command will
/// ever resolve (paths, `commit -F <file>`) follow without being told, so
/// no handler can forget to honour the flag. A directory that cannot be
/// entered is an operational refusal (exit 1): there is no such place to
/// run from.
fn enter(dir: &Path) -> anyhow::Result<()> {
    std::env::set_current_dir(dir).with_context(|| format!("cannot change to '{}'", dir.display()))
}

/// Run a mutating command, absorbing brief lock contention: a contended
/// attempt sleeps and runs again until the budget is spent, so a live
/// TUI's hold never surfaces as an error (#122). Reads never come here —
/// they take no lock and cannot contend.
///
/// The retry unit is the whole command, not the lock take: contention can
/// arrive from any write a command makes, and re-running the command is
/// exactly what exit `3` asks the caller to do. Re-running is safe while a
/// command's writes all sit under one lock take, as every op's do today —
/// contention then refuses before anything is written (ADR 0002:
/// fail-fast, never queued, never stolen). A command that took the lock
/// twice would replay its first write here, and would need its own
/// narrower retry.
///
/// Every attempt classifies the holder afresh and only the last one
/// decides, so no class short-circuits: a live holder that dies mid-budget
/// leaves the stale lock the dead refusal names, and a stale lock someone
/// clears mid-budget lets the retry through.
fn with_lock_retry(op: impl Fn() -> anyhow::Result<()>) -> anyhow::Result<()> {
    let started = Instant::now();
    loop {
        let result = op();
        let contended = matches!(&result, Err(err) if lock_holder(err).is_some());
        if !contended || started.elapsed() >= LOCK_RETRY_BUDGET {
            return result;
        }
        std::thread::sleep(LOCK_RETRY_INTERVAL);
    }
}

/// The lock holder a failed command contended with, if that is what
/// failed it. The variant survives the trip through `anyhow`, so the
/// classification core made (ADR 0002) is read back here rather than by
/// re-reading the lockfile — whose holder may have changed since.
fn lock_holder(err: &anyhow::Error) -> Option<LockHolder> {
    match err.downcast_ref::<gitchange_core::Error>() {
        Some(gitchange_core::Error::LockContention { holder, .. }) => Some(*holder),
        _ => None,
    }
}

/// Say what failed, on stderr, and answer with the exit code that says
/// what to do about it — one decision, so the text and the code cannot
/// disagree. stdout is left untouched: a failed command produces no
/// result.
///
/// Lock contention is the one failure that splits across the scheme's two
/// failure codes, on the holder alone (ADR 0002): a running holder — or
/// one that cannot be read, which is assumed running — may still release,
/// so the command is worth running again unchanged, and removal goes
/// unmentioned because acting on it would break that session's state. A
/// holder proven gone will never release, so its contention is an
/// ordinary refusal carrying the one accurate resolution, which core's
/// message already spells out.
fn report_failure(err: &anyhow::Error) -> u8 {
    eprintln!("{DIAG} {err:#}");
    match lock_holder(err) {
        Some(LockHolder::Alive { .. } | LockHolder::Unreadable) => {
            eprintln!("{DIAG} {RETRY_UNCHANGED}");
            TRANSIENT
        }
        Some(LockHolder::Dead { .. }) | None => REFUSAL,
    }
}

/// A mutating command's receipt (#122): the op's echo as one line on
/// stdout — nothing when it decided nothing — and each advisory as a
/// `notice:` line on stderr, every severity alike (severity is the
/// presentation layer's, and this surface has one channel for all of
/// them). Every mutating command answers through here, so no verb
/// invents its own dressing.
fn receipt(outcome: OpOutcome) {
    if let Some(echo) = outcome.echo {
        println!("{echo}");
    }
    print_notices(&outcome.advisories);
}

/// Core's advisories in this surface's dressing: the `gitchange:` prefix
/// every diagnostic carries, plus `notice:`, around core's one canonical
/// message (ADR 0006).
fn print_notices(advisories: &[Advisory]) {
    for advisory in advisories {
        eprintln!("{DIAG} notice: {}", advisory.message());
    }
}

/// The All view as text — core's grouping (`Snapshot::groups`, ADR 0006)
/// rendered line by line, from the read-only refresh (ADR 0005): a glance
/// captures nothing and advises nothing. The snapshot is all core hands
/// back — there is no advisories field to print from — and ownership is
/// what the records say, so a recordless hunk sits under unassigned even
/// while a changelist is active; the hint below is how the face says so.
fn status() -> anyhow::Result<()> {
    let repo = open_repo()?;
    let snapshot = repo.read_only_refresh()?;
    for group in snapshot.groups() {
        match &group.kind {
            // Quarantined unmerged paths (ADR 0007) — outside gitchange's
            // remit until resolved, so no stage mark or hunk counts.
            GroupKind::Conflicts => {
                println!("  {}", group.kind.label());
                for file in &group.files {
                    println!(
                        "      {} {} ({})",
                        file.kind.sigil(),
                        file.path,
                        gitchange_core::RESOLVE_OUTSIDE_GITCHANGE
                    );
                }
            }
            // A changelist or unassigned: both wear the `*` on the same
            // terms, since both are switch targets (ADR 0015), so one
            // arm prints them.
            kind => {
                let marker = if kind.active() { ACTIVE_MARKER } else { ' ' };
                println!("{marker} {}", kind.label());
                // Unassigned, not active, holding hunks: exactly one target
                // is always active (CONTEXT.md), so "not unassigned" is "a
                // real changelist", and capture is on for these. The hint
                // sits where git puts its own — under the header, before
                // the rows it speaks for.
                if matches!(kind, GroupKind::Unassigned { active: false })
                    && !group.files.is_empty()
                {
                    println!("    {CAPTURE_PENDING_HINT}");
                }
                print_files(&group.files);
            }
        }
    }
    Ok(())
}

fn print_files(files: &[&ChangedFile]) {
    for file in files {
        println!(
            "    {} {} {} {}/{}",
            file.stage().glyph(),
            file.kind.sigil(),
            file.path,
            file.staged_hunks(),
            file.total_hunks(),
        );
    }
}

/// `switch <name>`, where `unassigned` is a valid target: capture and
/// ambiguous-edit routing then flow to unassigned (ADR 0015). The line
/// is core's echo, not this frontend's: the marker write and the
/// sentence describing it are composed in one place (ADR 0006/0007), so
/// a switch reads the same in the Log panel and on stdout.
fn switch(name: &str) -> anyhow::Result<()> {
    let repo = open_repo()?;
    receipt(repo.switch(target_named(name))?);
    Ok(())
}

/// The stub contract (#140): a designed-but-unbuilt command parses fully
/// and refuses honestly — exit 1, empty stdout, one prefixed stderr line —
/// without touching the repository, so it refuses identically inside and
/// outside one. Exit 0 would lie to scripts; exit 2 would claim a usage
/// error clap did not raise.
fn not_implemented(command: &str) -> anyhow::Result<()> {
    anyhow::bail!("'{command}' is not implemented yet")
}

/// The hidden `restore` correction — the skeleton's own normative surface
/// (#140). Both git spellings parse and both are usage errors: the
/// grammar was git's, not gitchange's, so exit 2 is the honest class.
fn restore(staged: bool) -> ExitCode {
    if staged {
        eprintln!(
            "{DIAG} 'restore --staged' is git's spelling; gitchange's verb for \
             index := HEAD is 'unstage' — try 'gitchange unstage <changelist> \
             [<path>[:<hunk-id>]...]'"
        );
    } else {
        eprintln!(
            "{DIAG} gitchange has no worktree restore — changelists organise \
             changes, they do not undo them; 'git restore' remains the command \
             for that"
        );
    }
    ExitCode::from(2)
}

fn open_repo() -> anyhow::Result<Repo> {
    let cwd = std::env::current_dir()?;
    Ok(Repo::discover(&cwd)?)
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::{Cli, Command};

    /// Clap's own consistency check — conflicting shorts, a group naming
    /// a missing arg, an illegal positional layout — fails here in CI
    /// rather than at a user's first parse.
    #[test]
    fn the_tree_passes_claps_self_check() {
        Cli::command().debug_assert();
    }

    fn parse_diff(args: &[&str]) -> (Option<String>, Vec<String>) {
        let cli = Cli::try_parse_from(["gitchange", "diff"].iter().chain(args)).unwrap();
        let Some(Command::Diff { scope, .. }) = cli.command else {
            panic!("not a diff")
        };
        (
            scope.scope.clone(),
            scope.paths().map(str::to_owned).collect(),
        )
    }

    /// The skeleton's contract to the diff handler (#158): the pre-`--`
    /// first token and the path list, boundary preserved — after `--`
    /// the scope slot stays empty. Not observable through the stub, so
    /// pinned here rather than at the binary boundary.
    #[test]
    fn diff_preserves_the_double_dash_boundary() {
        assert_eq!(parse_diff(&[]), (None, vec![]));
        assert_eq!(parse_diff(&["feature"]), (Some("feature".into()), vec![]));
        assert_eq!(
            parse_diff(&["a.rs", "b.rs"]),
            (Some("a.rs".into()), vec!["b.rs".into()])
        );
        assert_eq!(parse_diff(&["--", "a.rs"]), (None, vec!["a.rs".into()]));
        assert_eq!(
            parse_diff(&["feature", "--", "a.rs", "b.rs"]),
            (Some("feature".into()), vec!["a.rs".into(), "b.rs".into()])
        );
        assert_eq!(
            parse_diff(&["feature", "a.rs", "--", "b.rs"]),
            (Some("feature".into()), vec!["a.rs".into(), "b.rs".into()])
        );
    }
}
