use std::process::ExitCode;

use clap::{Parser, Subcommand};

use gitchange_core::{ChangedFile, FileStage, GroupKind, Repo};

#[derive(Parser)]
#[command(
    name = "gitchange",
    version,
    about = "Organise uncommitted changes into changelists"
)]
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
        /// Name of the changelist to make active
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
            eprintln!("gitchange: {err:#}");
            ExitCode::from(1)
        }
    }
}

/// The All view as text — core's grouping (`Snapshot::groups`, ADR 0006)
/// rendered line by line.
fn status() -> anyhow::Result<()> {
    let repo = open_repo()?;
    let snapshot = repo.refresh()?;
    for notice in &snapshot.notices {
        eprintln!("gitchange: notice: {}", notice.message());
    }
    for group in snapshot.groups() {
        match &group.kind {
            // Quarantined unmerged paths (ADR 0007) — outside gitchange's
            // remit until resolved, so no stage mark or hunk counts.
            GroupKind::Conflicts => {
                println!("  conflicts");
                for file in &group.files {
                    println!("      U {} (resolve outside gitchange)", file.path);
                }
            }
            GroupKind::Changelist { name, active } => {
                let marker = if *active { '*' } else { ' ' };
                println!("{marker} {name}");
                print_files(&group.files);
            }
            GroupKind::Unassigned => {
                println!("  unassigned");
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
            stage_mark(file.stage()),
            file.kind.sigil(),
            file.path,
            file.staged_hunks(),
            file.total_hunks(),
        );
    }
}

fn switch(name: &str) -> anyhow::Result<()> {
    let repo = open_repo()?;
    repo.switch(name)?;
    println!("Switched to changelist '{name}'");
    Ok(())
}

fn open_repo() -> anyhow::Result<Repo> {
    let cwd = std::env::current_dir()?;
    Ok(Repo::discover(&cwd)?)
}

fn stage_mark(stage: FileStage) -> char {
    match stage {
        FileStage::Staged => '●',
        FileStage::PartiallyStaged => '◐',
        FileStage::Unstaged => '○',
    }
}
