//! The hunk ID (`CONTEXT.md` §Hunk ID): the snapshot-scoped address
//! frontends name one hunk by — a hash of the file path plus the content
//! anchor, printed with an `h` sigil so it reads as neither a commit nor
//! a blob OID.
//!
//! An address, not identity. Identity stays with membership records and
//! the matcher (ADR 0001); the ID is a pure function of a snapshot's
//! content and is stored nowhere, so an ID copied out of an aged snapshot
//! simply fails to resolve. It is minted on demand, by the surfaces that
//! print or resolve one ([`ChangedFile::hunk_addresses`]), rather than at
//! universe derivation: the TUI never addresses a hunk by ID, so its
//! refreshes never compute one.

use std::collections::HashMap;
use std::fmt;

use sha2::{Digest, Sha256};

use crate::matcher::anchor_lines;
use crate::universe::{ChangedFile, HunkIdentity};

/// The base ID of one hunk: the hash of its path and content anchor.
/// Shared by identical hunks in one file, which [`HunkAddress::offset`]
/// tells apart.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct HunkId([u8; 32]);

impl HunkId {
    /// The sigil every rendering leads with, on the wire included (#122):
    /// an ID an agent copies out of JSON cannot be misread as a commit
    /// SHA.
    pub const SIGIL: char = 'h';

    /// How many hex digits an abbreviated ID carries
    /// ([`HunkId::abbreviated`]) — comfortably above [`MIN_PREFIX_HEX`],
    /// so every abbreviation a text face prints is an address that
    /// resolves when pasted back.
    ///
    /// [`MIN_PREFIX_HEX`]: HunkId::MIN_PREFIX_HEX
    pub const ABBREVIATED_HEX: usize = 8;

    /// The shortest hex prefix an address may name (#122). Short enough
    /// to type, long enough that a prefix collision within one file needs
    /// a deliberate search rather than bad luck.
    pub const MIN_PREFIX_HEX: usize = 7;

    /// What every printed address depends on: an abbreviation a caller
    /// pastes back has to clear the minimum a caller may type.
    const _ABBREVIATION_IS_ADDRESSABLE: () = assert!(Self::ABBREVIATED_HEX >= Self::MIN_PREFIX_HEX);

    /// The ID as a text face prints it: the sigil and the first
    /// [`ABBREVIATED_HEX`] hex digits — a prefix of the full form, so it
    /// resolves as an address (#122).
    ///
    /// [`ABBREVIATED_HEX`]: HunkId::ABBREVIATED_HEX
    pub fn abbreviated(&self) -> String {
        let full = self.to_string();
        // The sigil plus the digits: both are ASCII, so the byte
        // truncation is a character truncation.
        full[..1 + Self::ABBREVIATED_HEX].to_owned()
    }

    /// Whether `hex` — the ID's digits with no sigil, in either case — is
    /// a prefix of this ID. The comparison lives here because the
    /// rendering does: a caller that lowercased the wrong side, or
    /// compared against a sigil'd string, would silently never match.
    pub fn has_prefix(&self, hex: &str) -> bool {
        // The rendering leads with the sigil; the typed prefix is digits
        // alone, in whichever case it was pasted.
        self.to_string()[1..].starts_with(&hex.to_ascii_lowercase())
    }

    /// Mint the ID of a hunk at `path`. A text hunk hashes its content
    /// anchor — the verbatim lines records store (ADR 0001) — so an
    /// unrelated edit elsewhere in the file leaves it unchanged; a
    /// degenerate hunk hashes a domain tag in the anchor's place, so a
    /// chmod'd binary's mode and whole-file hunks stay distinct
    /// (ADR 0017). Every flavour is tagged, so no text anchor can spell
    /// a degenerate hunk's tag by coincidence.
    ///
    /// Each part is length-prefixed rather than separated: a path may
    /// hold any byte but NUL and a diff line may hold NUL, so no one
    /// separator is safe in both, and without framing `("ab", "c")` and
    /// `("a", "bc")` would collide.
    pub(crate) fn mint(path: &str, identity: &HunkIdentity) -> Self {
        let (tag, anchor) = match identity {
            HunkIdentity::Text { lines } => ("text", anchor_lines(lines)),
            HunkIdentity::WholeFile { .. } => ("whole-file", Vec::new()),
            HunkIdentity::ModeChange => ("mode", Vec::new()),
        };
        let mut hasher = Sha256::new();
        for part in [tag, path]
            .into_iter()
            .chain(anchor.iter().map(String::as_str))
        {
            hasher.update((part.len() as u64).to_le_bytes());
            hasher.update(part.as_bytes());
        }
        Self(hasher.finalize().into())
    }
}

/// The `h` sigil plus the full 64 lowercase hex digits — the whole ID,
/// which is what the wire carries. A text face abbreviates it (#158);
/// the abbreviation is a prefix of this.
impl fmt::Display for HunkId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", Self::SIGIL)?;
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for HunkId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HunkId({self})")
    }
}

/// One hunk's address within its file: the base ID and, where the file
/// holds identical hunks, the ordinal that tells this one from the rest.
/// Composed with the path as `<path>:<id>[/<n>]` — the address every
/// hunk-addressing verb speaks (`CONTEXT.md` §Hunk ID).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HunkAddress {
    pub id: HunkId,
    /// This hunk's position, in file order from `0`, among the file's
    /// hunks sharing its [`HunkId`] — `Some` exactly when the ID is
    /// shared, so a unique hunk's address needs no `/<n>`.
    pub offset: Option<usize>,
}

impl HunkAddress {
    /// The composed address `<path>:<id>[/<n>]` as a text face prints it:
    /// the ID abbreviated ([`HunkId::abbreviated`]), the ordinal carried
    /// only where identical hunks make it part of the address. Composed
    /// here rather than at each printing site so the string a refusal
    /// names and the string a patch header carries are one shape — and
    /// one an agent can paste straight back into a verb.
    pub fn abbreviated_at(&self, path: &str) -> String {
        let ordinal = match self.offset {
            Some(offset) => format!("/{offset}"),
            None => String::new(),
        };
        format!("{path}:{}{ordinal}", self.id.abbreviated())
    }
}

/// The addresses of `file`'s hunks, aligned with [`ChangedFile::hunks`].
/// Ordinals are a file-level fact, which is why the file's hunks are
/// minted together.
pub(crate) fn mint_addresses(file: &ChangedFile) -> Vec<HunkAddress> {
    let ids: Vec<HunkId> = file
        .hunks
        .iter()
        .map(|hunk| HunkId::mint(&file.path, &hunk.identity))
        .collect();
    let mut occurrences: HashMap<HunkId, usize> = HashMap::new();
    for id in &ids {
        *occurrences.entry(*id).or_default() += 1;
    }
    let mut seen: HashMap<HunkId, usize> = HashMap::new();
    ids.into_iter()
        .map(|id| {
            let ordinal = seen.entry(id).or_default();
            let offset = (occurrences[&id] > 1).then_some(*ordinal);
            *ordinal += 1;
            HunkAddress { id, offset }
        })
        .collect()
}
