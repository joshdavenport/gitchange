use std::process::ExitCode;

use clap::{Parser, Subcommand};

use gitchange_core::{ChangeKind, FileStage, Repo};

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

fn status() -> anyhow::Result<()> {
    let repo = open_repo()?;
    let snapshot = repo.refresh()?;
    for changelist in &snapshot.changelists {
        let marker = if snapshot.active.as_deref() == Some(changelist.name.as_str()) {
            '*'
        } else {
            ' '
        };
        println!("{marker} {}", changelist.name);
    }
    if !snapshot.changelists.is_empty() && !snapshot.files.is_empty() {
        println!();
    }
    for file in &snapshot.files {
        println!(
            "{} {} {} {}/{}",
            stage_mark(file.stage()),
            sigil(file.kind),
            file.path,
            file.staged_hunks(),
            file.total_hunks(),
        );
    }
    Ok(())
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

fn sigil(kind: ChangeKind) -> char {
    match kind {
        ChangeKind::Added => 'A',
        ChangeKind::Modified => 'M',
        ChangeKind::Deleted => 'D',
        ChangeKind::TypeChanged => 'T',
        ChangeKind::Untracked => '?',
        ChangeKind::Conflicted => 'U',
    }
}
