//! `commit <changelist>`'s target validation, the guard rungs the CLI
//! owns, and the message sources (#151) — everything between the
//! invocation's one persisting refresh and core's temp-index commit.
//!
//! The refusals are exit `1` throughout: which changelists exist, what a
//! git operation is doing, and what the index holds are repo facts, which
//! clap cannot see. There is no exit-`2` check here at all — `commit`
//! takes a changelist and flags, never a path or an address, so the shared
//! addressing grammar's non-declarative checks have nothing to ask, and
//! the skeleton's declarative surface (#140) is this command's whole
//! exit-`2` story.

use std::io::Read as _;

use anyhow::Context as _;
use gitchange_core::{
    ALL, CommitMessage, Error, PreparedCommit, Snapshot, UNASSIGNED, count_noun, holder_label,
};

use crate::CommitArgs;
use crate::scope;

/// The changelist argument as core takes it (`None` is unassigned), or
/// the refusal it earns.
///
/// Validated ahead of the whole guard stack, so a target nobody
/// recognises is answered as such before any condition of the repo is —
/// `commit all` mid-merge speaks this, not the operation guard.
///
/// `unassigned` is recognised here and gated at rung 4: it is a legal
/// scope everywhere a changelist is named, and with the gate overridden it
/// commits under the same rules as any changelist.
pub fn target<'a>(name: &'a str, snapshot: &Snapshot) -> anyhow::Result<Option<&'a str>> {
    if name == ALL {
        // Deliberately not the staging verbs' "a view, not a scope"
        // wording: there is one commit mechanism and one payload
        // (ADR 0004), so `all` has no commit meaning to be refused *as*,
        // and the plain reading — it is not a changelist — is the whole
        // story. No override, because there is no multi-commit loop to
        // expose.
        anyhow::bail!(
            "'{ALL}' is not a changelist — commit one changelist by name: \
             'gitchange commit <changelist> -m <message>'"
        );
    }
    scope::recognised(name, &snapshot.changelists).map_err(|refusal| anyhow::anyhow!(refusal))
}

/// Rung 3 of the guard stack (#151), amend's own: the temp index builds
/// on HEAD's tree, so amending while HEAD is somebody else's commit folds
/// this payload into it — and commits carry no provenance to check
/// afterwards. The refusal is worded for all three shapes of the hazard at
/// once (another changelist's commit, a commit made outside gitchange, no
/// record yet), because what HEAD *is* instead is not a thing the state
/// file knows.
///
/// Apart from [`refuse`] only because of where it sits in the order —
/// ahead of rung 4's gate — never because it is a different kind of thing;
/// the whole rung, condition and text, is here.
///
/// `head_is_own` is [`gitchange_core::Repo::head_is_own_last_commit`],
/// taken as a thunk because it is the one rung whose fact is not on the
/// snapshot the others read: a second state read, worth not making where
/// the answer cannot matter. That laziness is also what makes the override
/// inert — it short-circuits before the read rather than in an arm of its
/// own.
///
/// CLI-only by decision (ADR 0004 §Amend): a policy guard against agent
/// misfire, the same CLI-stricter-than-TUI split as rung 4's gate, and the
/// TUI's amend stays gate-free — one human's stated intent.
pub fn refuse_foreign_head(
    args: &CommitArgs,
    target: Option<&str>,
    head_is_own: impl FnOnce() -> Result<bool, Error>,
) -> anyhow::Result<Option<String>> {
    if !args.amend || args.allow_foreign_head || head_is_own()? {
        return Ok(None);
    }
    // Phrased around the record rather than as a possessive: a quoted
    // name takes `'s` as `'feature''s`, which reads as a quoting mistake.
    Ok(Some(format!(
        "HEAD is not the commit gitchange last made for {} — amending would fold this payload \
         into whatever commit it is; commit without '--amend', or pass '--allow-foreign-head' \
         to amend HEAD as it stands",
        holder_label(target)
    )))
}

/// Rungs 4 to 6 of the guard stack (#151) in their fixed order: the first
/// condition that holds speaks, complete within its rung, and `None` lets
/// the commit through. One refusal per invocation, so fixing what the
/// text names is always forward progress.
///
/// Rungs 1 to 3 fire earlier by construction, in three homes of their
/// own: the operation guard, read off the refresh's snapshot and enforced
/// again by core at the commit itself; foreign content, which core raises
/// deriving the payload; and [`refuse_foreign_head`], which the handler asks
/// between them and this. Holding the order across those homes is the
/// handler's job; what lives here is the part that is the CLI's own
/// policy, where ADR 0015 maps the TUI's warn-and-confirms onto refusals
/// with named overrides.
///
/// Every override names the condition it excuses and is inert when that
/// condition is absent, so a flag left in a retried command can never
/// change the command's meaning.
pub fn refuse(prepared: &PreparedCommit, args: &CommitArgs) -> Option<String> {
    // The scope is read off the payload rather than taken beside it, so a
    // rung can only ever speak for the commit that would actually be made.
    let target = prepared.changelist();
    // Rung 4, the unassigned gate. Categorical rather than detection:
    // unassigned hunks are recordless (ADR 0016), so there is no
    // attribution to check — the addressed scope simply *is* the
    // unclaimed pool, and an agent committing it is almost always
    // skipping the assign step. The refusal is workflow correction, so it
    // names both resolutions.
    if target.is_none() && !args.allow_unassigned {
        return Some(format!(
            "committing {UNASSIGNED} skips the assign step — assign the hunks and commit \
             that changelist ('gitchange assign <path>... --to <changelist>'), or pass \
             '--allow-unassigned' to commit the unclaimed pool as it stands"
        ));
    }
    // Rung 5, staged-stale: the index holds an overlapping-but-different
    // version of what the worktree shows, so committing now commits
    // content that is not what you see. Each hunk is named as the
    // composed address a caller pastes back, and aligning is a strict
    // subset of `add`, so no separate spelling exists to teach.
    let stale = prepared.stale_addresses();
    if !stale.is_empty() && !args.allow_staged_stale {
        return Some(format!(
            "the index holds a different version of {} in this payload: {}\n\
             align the index to the worktree with 'gitchange add {}', or pass \
             '--allow-staged-stale' to commit what the index holds",
            count_noun(stale.len(), "hunk"),
            stale.join(", "),
            invocation(target),
        ));
    }
    // Rung 6, the empty payload. No stage-all flag and no `-a` borrow:
    // in this state `add <changelist>` *is* the TUI's stage-all offer
    // (ADR 0004), where git's `-a` is file-scoped, ungated and always
    // broadens the payload. Rungs 5 and 6 never co-occur — a `◑` hunk is
    // itself payload.
    if prepared.payload.is_empty() {
        // Under `--amend` the same condition means something the plain
        // reading misses: an empty payload is what a *reword* looks like,
        // and reword stays git's job (ADR 0004 §Amend) — gitchange's
        // commit filters a payload, and a reword has none to filter. So
        // the amend arm names the second resolution rather than leaving a
        // caller to stage something it does not want staged.
        let reword = match args.amend {
            true => ", or reword the commit with raw 'git commit --amend'",
            false => "",
        };
        return Some(format!(
            "{} has no staged hunks to commit — stage some with 'gitchange add {}'{reword}",
            holder_label(target),
            invocation(target),
        ));
    }
    None
}

/// The target as a resolution quotes it back inside an invocation:
/// unquoted, because what follows the word is a command line to paste
/// (`gitchange add unassigned`), where [`holder_label`] is how a holder is
/// *named* in prose.
fn invocation(target: Option<&str>) -> &str {
    target.unwrap_or(UNASSIGNED)
}

/// One invocation's commit message, owned so the borrow core delivers
/// outlives whatever the bytes were read from.
pub enum Message {
    Given(String),
    Kept,
}

impl Message {
    pub fn source(&self) -> CommitMessage<'_> {
        match self {
            Message::Given(text) => CommitMessage::Given(text),
            Message::Kept => CommitMessage::Kept,
        }
    }
}

/// The message the named source holds. Exactly one source can arrive —
/// none, or two, died at parse (#140) — so these arms are exhaustive over
/// what an invocation can carry rather than over what could be spelled.
///
/// Resolved after the guard stack, deliberately: `-F -` drains stdin, and
/// a command on its way to a refusal has no business consuming its
/// caller's heredoc.
pub fn message(args: &CommitArgs) -> anyhow::Result<Message> {
    if args.no_edit {
        return Ok(Message::Kept);
    }
    match &args.file {
        Some(file) => Ok(Message::Given(read(file)?)),
        // git's `-m` grammar wholesale: the value is whatever bytes the
        // shell delivered, so a multiline message needs no flag, and
        // repeats concatenate as paragraphs — a blank line between them,
        // which is what git does with its own.
        None => Ok(Message::Given(args.message.join("\n\n"))),
    }
}

/// `-F <file>`, verbatim — `-` reading stdin, which is the
/// `git commit -F - <<'EOF'` habit an agent already has. Relative to the
/// caller's cwd, and after `-C` the cwd *is* `<dir>` (#139), so nothing
/// here reads the flag.
fn read(file: &str) -> anyhow::Result<String> {
    if file == "-" {
        let mut message = String::new();
        std::io::stdin()
            .read_to_string(&mut message)
            .context("cannot read the commit message from stdin")?;
        return Ok(message);
    }
    std::fs::read_to_string(file)
        .with_context(|| format!("cannot read the commit message from '{file}'"))
}
