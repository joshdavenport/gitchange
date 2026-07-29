use std::process::ExitCode;

use clap::{Parser, Subcommand};

use gitchange_core::{ChangeKind, Repo};

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
    /// List changed files in the current repository
    Status,
}

fn main() -> ExitCode {
    // Usage errors exit with code 2 via clap; operational errors map to 1.
    let cli = Cli::parse();
    let result = match cli.command {
        Some(Command::Status) => status(),
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
    let cwd = std::env::current_dir()?;
    let repo = Repo::discover(&cwd)?;
    let snapshot = repo.refresh()?;
    for file in &snapshot.files {
        println!("{} {}", sigil(file.kind), file.path);
    }
    Ok(())
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
