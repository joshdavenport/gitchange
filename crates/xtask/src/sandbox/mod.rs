//! Sandbox repos for manual TUI testing (issue #42): named, persistent,
//! deterministic repos under `.sandbox/<scenario>/`, built by replaying
//! real core ops so the state file is always what core genuinely
//! produces. Reset is nuke-and-rebuild; `status` compares against a
//! build-time fingerprint in `.sandbox/.meta/`.

pub(crate) mod builder;
mod fingerprint;
mod scenarios;

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use builder::Sandbox;
use scenarios::Scenario;

/// Project root, derived from this crate's manifest at compile time.
pub(crate) fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("xtask lives at <root>/crates/xtask")
        .to_path_buf()
}

fn sandbox_root() -> PathBuf {
    project_root().join(".sandbox")
}

fn meta_dir() -> PathBuf {
    sandbox_root().join(".meta")
}

pub fn make(name: Option<&str>, all: bool) -> Result<()> {
    let catalogue = scenarios::all();
    let selected: Vec<&Scenario> = match (name, all) {
        (None, true) => catalogue.iter().collect(),
        (Some(name), false) => {
            let scenario = catalogue
                .iter()
                .find(|s| s.name == name)
                .with_context(|| format!("unknown scenario `{name}` (see `sandbox status`)"))?;
            vec![scenario]
        }
        (Some(_), true) => bail!("pass a scenario name or --all, not both"),
        (None, false) => bail!("pass a scenario name or --all"),
    };

    let mut skipped = Vec::new();
    for scenario in selected {
        match build_one(scenario) {
            Ok(()) => println!("built    {}", scenario.name),
            Err(err) => {
                // Skip-with-notice: a scenario needing a core op that
                // doesn't exist yet must not gate the whole tool.
                println!("skipped  {} — {err:#}", scenario.name);
                skipped.push(scenario.name);
                let _ = fs::remove_dir_all(sandbox_root().join(scenario.name));
            }
        }
    }
    if !skipped.is_empty() {
        println!(
            "\n{} scenario(s) skipped: {}. File a follow-up issue naming the missing core op(s).",
            skipped.len(),
            skipped.join(", ")
        );
    }
    Ok(())
}

fn build_one(scenario: &Scenario) -> Result<()> {
    let dir = sandbox_root().join(scenario.name);
    if dir.exists() {
        fs::remove_dir_all(&dir)
            .with_context(|| format!("remove existing sandbox {}", dir.display()))?;
    }
    let mut sandbox = Sandbox::init(&dir)?;
    (scenario.build)(&mut sandbox)
        .with_context(|| format!("build scenario `{}`", scenario.name))?;
    let print = fingerprint::capture(&dir, scenario.name)?;
    fingerprint::store(&meta_dir(), &print)?;
    Ok(())
}

pub fn status() -> Result<()> {
    for scenario in scenarios::all() {
        let dir = sandbox_root().join(scenario.name);
        let line = if !dir.exists() {
            "missing".to_string()
        } else {
            match fingerprint::load(&meta_dir(), scenario.name)? {
                None => "modified (no fingerprint recorded — rebuild with `make`)".to_string(),
                Some(stored) => {
                    let current = fingerprint::capture(&dir, scenario.name)?;
                    let diverged = stored.diverged_layers(&current);
                    if diverged.is_empty() {
                        "pristine".to_string()
                    } else {
                        format!("modified ({})", diverged.join(", "))
                    }
                }
            }
        };
        println!("{:<16} {}", scenario.name, line);
    }
    Ok(())
}
