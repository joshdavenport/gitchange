use std::process::ExitCode;

use clap::{Parser, Subcommand};

use gitchange_core::{ChangeKind, ChangedFile, FileStage, Notice, Repo};

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

/// The All view as text: every changelist in user order with its files,
/// the active one marked `*`, the unassigned group last.
fn status() -> anyhow::Result<()> {
    let repo = open_repo()?;
    let snapshot = repo.refresh()?;
    for notice in &snapshot.notices {
        eprintln!("gitchange: {}", notice_line(notice));
    }
    for changelist in &snapshot.changelists {
        let marker = if snapshot.active.as_deref() == Some(changelist.name.as_str()) {
            '*'
        } else {
            ' '
        };
        println!("{marker} {}", changelist.name);
        print_files(&snapshot.files_in(Some(&changelist.name)));
    }
    let unassigned = snapshot.files_in(None);
    if !unassigned.is_empty() {
        println!("  unassigned");
        print_files(&unassigned);
    }
    Ok(())
}

fn print_files(files: &[&ChangedFile]) {
    for file in files {
        println!(
            "    {} {} {} {}/{}",
            stage_mark(file.stage()),
            sigil(file.kind),
            file.path,
            file.staged_hunks(),
            file.total_hunks(),
        );
    }
}

fn notice_line(notice: &Notice) -> String {
    match notice {
        Notice::AmbiguousOverlap {
            path,
            new_start,
            candidates,
            assigned_to,
        } => {
            let destination = match assigned_to {
                Some(name) => format!("assigned to active changelist '{name}'"),
                None => "left unassigned".into(),
            };
            format!(
                "notice: hunk at {path}:{new_start} overlaps changelists {}; {destination}",
                candidates
                    .iter()
                    .map(|name| format!("'{name}'"))
                    .collect::<Vec<_>>()
                    .join(", "),
            )
        }
        Notice::StaleHunk { path, new_start } => {
            format!(
                "notice: hunk at {path}:{new_start} changed since the last refresh; nothing applied"
            )
        }
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
