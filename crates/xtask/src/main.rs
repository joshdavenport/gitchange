//! Repo tasks for gitchange development. Currently one family:
//! `sandbox`, persistent manual-testing repos under `.sandbox/`
//! (issue #42).

mod sandbox;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "xtask", about = "gitchange development tasks")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Manage manual-testing sandbox repos under .sandbox/
    Sandbox {
        #[command(subcommand)]
        command: SandboxCommand,
    },
}

#[derive(Subcommand)]
enum SandboxCommand {
    /// Build scenario repos from their definitions (nuke-and-rebuild)
    Make {
        /// Scenario name (see `status` for the catalogue)
        name: Option<String>,
        /// Build every scenario
        #[arg(long)]
        all: bool,
    },
    /// Reset scenario repos to initial state (alias of make)
    Reset {
        /// Scenario name (see `status` for the catalogue)
        name: Option<String>,
        /// Reset every scenario
        #[arg(long)]
        all: bool,
    },
    /// Report each scenario: missing / pristine / modified
    Status,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Sandbox { command } => match command {
            SandboxCommand::Make { name, all } | SandboxCommand::Reset { name, all } => {
                sandbox::make(name.as_deref(), all)
            }
            SandboxCommand::Status => sandbox::status(),
        },
    }
}
