use std::process::ExitCode;

use clap::{Parser, Subcommand};

use gitchange_core::{ACTIVE_MARKER, ChangedFile, GroupKind, Repo, target_named};

/// The prefix every diagnostic this binary writes to stderr carries, so
/// output piped alongside git's own is attributable at a glance.
const DIAG: &str = "gitchange:";

#[derive(Parser)]
// `name` is left to clap, which takes it from the package name.
#[command(version, about = "Organise uncommitted changes into changelists")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// List changelists and changed files in the current repository
    Status,
    /// Set the active changelist
    Switch {
        /// Name of the changelist to make active, or `unassigned` to
        /// capture nothing
        name: String,
    },
}

fn main() -> ExitCode {
    // Usage errors exit with code 2 via clap; operational errors map to 1.
    let cli = Cli::parse();
    let result = match cli.command {
        Some(Command::Status) => status(),
        Some(Command::Switch { name }) => switch(&name),
        None => gitchange_tui::run().map_err(anyhow::Error::from),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{DIAG} {err:#}");
            ExitCode::from(1)
        }
    }
}

/// The All view as text — core's grouping (`Snapshot::groups`, ADR 0006)
/// rendered line by line.
fn status() -> anyhow::Result<()> {
    let repo = open_repo()?;
    let snapshot = repo.refresh()?;
    for advisory in &snapshot.advisories {
        eprintln!("{DIAG} notice: {}", advisory.message());
    }
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
/// ambiguous-edit routing then flow to unassigned (ADR 0015). One
/// sentence covers both, since "changelist 'unassigned'" would name a
/// changelist that cannot exist.
fn switch(name: &str) -> anyhow::Result<()> {
    let repo = open_repo()?;
    repo.switch(target_named(name))?;
    println!("Switched to '{name}'");
    Ok(())
}

fn open_repo() -> anyhow::Result<Repo> {
    let cwd = std::env::current_dir()?;
    Ok(Repo::discover(&cwd)?)
}
