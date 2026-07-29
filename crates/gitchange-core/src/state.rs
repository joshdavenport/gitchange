use serde::{Deserialize, Serialize};

use crate::error::Error;

/// Names claimed by pseudo-views (`CONTEXT.md`): never valid for user
/// changelists.
pub const RESERVED_NAMES: [&str; 2] = ["all", "unassigned"];

pub const SCHEMA_VERSION: u32 = 1;

/// A named set of uncommitted changes. Grows membership records in
/// ticket 25; for now a changelist is its name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Changelist {
    pub name: String,
}

/// Everything the state file holds (ADR 0002): the changelists, in user
/// order, and the active marker. Exactly one changelist is active
/// whenever any exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct State {
    pub version: u32,
    pub active: Option<String>,
    pub changelists: Vec<Changelist>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            active: None,
            changelists: Vec::new(),
        }
    }
}

impl State {
    fn contains(&self, name: &str) -> bool {
        self.changelists.iter().any(|cl| cl.name == name)
    }

    fn validate_new_name(&self, name: &str) -> Result<(), Error> {
        if name.trim().is_empty() {
            return Err(Error::InvalidName {
                reason: "name is empty".into(),
            });
        }
        if RESERVED_NAMES.contains(&name) {
            return Err(Error::ReservedName { name: name.into() });
        }
        if self.contains(name) {
            return Err(Error::ChangelistExists { name: name.into() });
        }
        Ok(())
    }

    /// Append a changelist. The first one created becomes active; later
    /// ones never steal the marker.
    pub fn create(&mut self, name: &str) -> Result<(), Error> {
        self.validate_new_name(name)?;
        self.changelists.push(Changelist { name: name.into() });
        if self.active.is_none() {
            self.active = Some(name.into());
        }
        Ok(())
    }

    /// Rename `from` to `to`, carrying the active marker with it.
    pub fn rename(&mut self, from: &str, to: &str) -> Result<(), Error> {
        if !self.contains(from) {
            return Err(Error::UnknownChangelist { name: from.into() });
        }
        if from == to {
            return Ok(());
        }
        self.validate_new_name(to)?;
        for cl in &mut self.changelists {
            if cl.name == from {
                cl.name = to.into();
            }
        }
        if self.active.as_deref() == Some(from) {
            self.active = Some(to.into());
        }
        Ok(())
    }

    /// Remove a changelist. Deleting the active one promotes the first
    /// remaining changelist, keeping the exactly-one-active invariant.
    pub fn delete(&mut self, name: &str) -> Result<(), Error> {
        if !self.contains(name) {
            return Err(Error::UnknownChangelist { name: name.into() });
        }
        self.changelists.retain(|cl| cl.name != name);
        if self.active.as_deref() == Some(name) {
            self.active = self.changelists.first().map(|cl| cl.name.clone());
        }
        Ok(())
    }

    /// Set the active marker.
    pub fn switch(&mut self, name: &str) -> Result<(), Error> {
        if !self.contains(name) {
            return Err(Error::UnknownChangelist { name: name.into() });
        }
        self.active = Some(name.into());
        Ok(())
    }
}
