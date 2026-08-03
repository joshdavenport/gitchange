//! Shared test support (ADR 0008): programmatically built temp repos,
//! one per test. Grows `with_hook()` and staged-state helpers with later
//! tickets.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

pub struct RepoFixture {
    dir: TempDir,
    repo: git2::Repository,
}

/// Holds the object store unwritable; restores the original mode on
/// drop. See [`RepoFixture::unwritable_odb`].
#[cfg(unix)]
pub struct UnwritableOdb {
    path: PathBuf,
    original: fs::Permissions,
}

#[cfg(unix)]
impl Drop for UnwritableOdb {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.path, self.original.clone());
    }
}

impl RepoFixture {
    /// Init a fresh repo in a temp dir with a committing identity set.
    ///
    /// The pinned knobs below are the same set xtask's `Sandbox::init`
    /// pins (crates/xtask/src/sandbox/builder.rs) — both exist so the
    /// host's global config can't reach the repo. Add a knob here and add
    /// it there too; a knob pinned on only one side leaves the other
    /// silently inheriting the host. The two builders are otherwise
    /// deliberately separate: this one builds through libgit2 in-process
    /// and panics, the sandbox shells out throughout (mirroring ADR 0004)
    /// and returns `anyhow` errors.
    ///
    /// The exception is `git_output` below, which shells out for the
    /// in-progress operation states libgit2 can't produce and cuts the
    /// host's config out by environment instead. That cut-out is
    /// deliberately not mirrored in the sandbox: a sandbox repo is meant
    /// to behave like the user's own, config and all.
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("create temp dir");
        // Pin the initial branch: plain `init` takes it from the host's
        // `init.defaultBranch`, so fixtures were `main` locally but
        // `master` on unconfigured CI runners.
        let mut init_options = git2::RepositoryInitOptions::new();
        init_options.initial_head("main");
        let repo = git2::Repository::init_opts(dir.path(), &init_options).expect("git init");
        {
            let mut config = repo.config().expect("open repo config");
            config.set_str("user.name", "gitchange-tests").unwrap();
            config
                .set_str("user.email", "tests@gitchange.invalid")
                .unwrap();
            // Pin the libgit2 default explicitly: a runner's global
            // `autocrlf=true` (git-for-Windows) would normalize the
            // corpus's byte-exact CRLF assertions away on checkin.
            config.set_bool("core.autocrlf", false).unwrap();
            // The commit shell-out (ADR 0004) inherits real git config:
            // pin the knobs a developer's global config could leak in.
            config.set_bool("commit.gpgsign", false).unwrap();
            config.set_str("core.hooksPath", ".git/hooks").unwrap();
        }
        Self { dir, repo }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Write `content` to a repo-relative path, creating parent dirs.
    #[allow(dead_code)]
    pub fn write(&self, rel: &str, content: &str) -> &Self {
        let path = self.dir.path().join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
        self
    }

    /// Write raw bytes to a repo-relative path, creating parent dirs —
    /// for binary fixtures.
    #[allow(dead_code)]
    pub fn write_bytes(&self, rel: &str, content: &[u8]) -> &Self {
        let path = self.dir.path().join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
        self
    }

    /// Move a repo-relative path to another, git none the wiser — the
    /// rename ADR 0011 sees as a delete plus an add.
    #[allow(dead_code)]
    pub fn rename(&self, from: &str, to: &str) -> &Self {
        let target = self.dir.path().join(to);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::rename(self.dir.path().join(from), target).unwrap();
        self
    }

    /// Install a git hook (e.g. `pre-commit`) with the given script body,
    /// marked executable — the `with_hook` fixture ADR 0008 promised.
    #[allow(dead_code)]
    pub fn with_hook(&self, name: &str, script: &str) -> &Self {
        let dir = self.repo.path().join("hooks");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&path).unwrap().permissions();
            perms.set_mode(perms.mode() | 0o755);
            fs::set_permissions(&path, perms).unwrap();
        }
        self
    }

    /// Make the object store unwritable until the returned guard drops —
    /// the only condition found that makes a real libgit2 apply refuse
    /// without a test-only seam (issue #58): an apply computes every
    /// postimage blob into the odb before it writes anything else, so an
    /// odb it cannot write to refuses the apply itself rather than
    /// something around it.
    ///
    /// One mode on `objects/` is enough because libgit2 stages every new
    /// object as a temp file in that directory before renaming it into
    /// its fanout subdirectory — the write it is denied is always there,
    /// whichever fanout dirs already exist.
    ///
    /// Panics where the mode fails to take — root in a container ignores
    /// it — rather than skipping: a test that quietly asserts a refusal
    /// over a perfectly successful apply is exactly the vacuous pass
    /// ADR 0008 has builders panic to prevent.
    ///
    /// Restoring on drop matters twice over: a panicking assertion must
    /// not leave the temp dir undeletable, and `TempDir`'s own teardown
    /// needs the write bit back.
    #[cfg(unix)]
    #[allow(dead_code)]
    pub fn unwritable_odb(&self) -> UnwritableOdb {
        use std::os::unix::fs::PermissionsExt;
        let path = self.repo.path().join("objects");
        let original = fs::metadata(&path).unwrap().permissions();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o500)).unwrap();
        // Built before the probe, so a panic below still restores.
        let guard = UnwritableOdb {
            path: path.clone(),
            original,
        };
        let probe = path.join("write-probe");
        if fs::write(&probe, b"probe").is_ok() {
            let _ = fs::remove_file(&probe);
            panic!(
                "the object store is still writable at mode 0o500 — this \
                 environment ignores directory permissions (running as \
                 root?), and the apply under test would succeed"
            );
        }
        guard
    }

    /// File names in the per-worktree gitchange state dir. ADR 0004's
    /// "every failure discards a temp file" is an assertion about this
    /// listing: the commit temp index and message file live here, and
    /// nothing but `state.json` may outlive a commit attempt.
    #[allow(dead_code)]
    pub fn state_dir_entries(&self) -> Vec<String> {
        let dir = self.repo.path().join("gitchange");
        let Ok(entries) = fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// HEAD tree's blob bytes for a repo-relative path, `None` when the
    /// path (or HEAD) doesn't exist — commit-result ground truth.
    #[allow(dead_code)]
    pub fn head_bytes(&self, rel: &str) -> Option<Vec<u8>> {
        let tree = self.repo.head().ok()?.peel_to_tree().ok()?;
        let entry = tree.get_path(Path::new(rel)).ok()?;
        let blob = self.repo.find_blob(entry.id()).unwrap();
        Some(blob.content().to_vec())
    }

    /// The HEAD commit's message.
    #[allow(dead_code)]
    pub fn head_message(&self) -> String {
        self.repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .message()
            .unwrap()
            .to_owned()
    }

    /// Number of commits reachable from HEAD; 0 on an unborn branch.
    #[allow(dead_code)]
    pub fn commit_count(&self) -> usize {
        let Ok(head) = self.repo.head() else {
            return 0;
        };
        let mut walk = self.repo.revwalk().unwrap();
        walk.push(head.peel_to_commit().unwrap().id()).unwrap();
        walk.count()
    }

    /// The current HEAD commit id, in the form the state file stores.
    #[allow(dead_code)]
    pub fn head_oid(&self) -> String {
        self.repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string()
    }

    /// Stage one path, exactly as `git add <rel>` would.
    #[allow(dead_code)]
    pub fn stage(&self, rel: &str) -> &Self {
        let mut index = self.repo.index().unwrap();
        index.add_path(Path::new(rel)).unwrap();
        index.write().unwrap();
        self
    }

    /// Stage one path's deletion, exactly as `git add <rel>` after `rm`.
    #[allow(dead_code)]
    pub fn stage_removal(&self, rel: &str) -> &Self {
        let mut index = self.repo.index().unwrap();
        index.remove_path(Path::new(rel)).unwrap();
        index.write().unwrap();
        self
    }

    /// The index entry's filemode (e.g. 0o100644 / 0o100755), `None` when
    /// no entry exists — for mode-change assertions.
    #[allow(dead_code)]
    pub fn index_mode(&self, rel: &str) -> Option<u32> {
        let mut index = self.repo.index().unwrap();
        index.read(false).unwrap();
        Some(index.get_path(Path::new(rel), 0)?.mode)
    }

    /// How many objects the object database holds, loose and packed —
    /// the ground truth for ADR 0002's "no git objects written". Counted
    /// through a freshly opened handle: this fixture's own handle caches
    /// its odb backends, and a cached pack listing could mask exactly the
    /// write being asserted against.
    #[allow(dead_code)]
    pub fn odb_object_count(&self) -> usize {
        let repo = git2::Repository::open(self.repo.path()).expect("reopen repo");
        let odb = repo.odb().unwrap();
        let mut count = 0;
        odb.foreach(|_| {
            count += 1;
            true
        })
        .unwrap();
        count
    }

    /// Add a linked worktree named `name`, returning its path. Requires
    /// at least one commit (git refuses worktrees on unborn branches).
    #[allow(dead_code)]
    pub fn add_worktree(&self, name: &str) -> std::path::PathBuf {
        let path = self.dir.path().join(format!("{name}-worktree"));
        self.repo
            .worktree(name, &path, None)
            .expect("add linked worktree");
        path
    }

    /// Stage a blob at a repo-relative path given as raw bytes, without
    /// touching the worktree — the only portable way to conjure a
    /// non-UTF-8 path into a repository.
    #[allow(dead_code)]
    pub fn stage_blob_at_raw_path(&self, path_bytes: &[u8], content: &str) -> &Self {
        let mut index = self.repo.index().unwrap();
        let entry = git2::IndexEntry {
            ctime: git2::IndexTime::new(0, 0),
            mtime: git2::IndexTime::new(0, 0),
            dev: 0,
            ino: 0,
            mode: 0o100644,
            uid: 0,
            gid: 0,
            file_size: content.len() as u32,
            id: git2::Oid::zero(),
            flags: 0,
            flags_extended: 0,
            path: path_bytes.to_vec(),
        };
        index.add_frombuffer(&entry, content.as_bytes()).unwrap();
        index.write().unwrap();
        self
    }

    /// The real index's content for a repo-relative path, `None` when no
    /// entry exists — the ground truth write-through assertions check.
    #[allow(dead_code)]
    pub fn index_content(&self, rel: &str) -> Option<String> {
        Some(String::from_utf8(self.index_bytes(rel)?).unwrap())
    }

    /// Verbatim index blob bytes — for asserting non-UTF-8 content
    /// survives staging without lossy round-trips.
    #[allow(dead_code)]
    pub fn index_bytes(&self, rel: &str) -> Option<Vec<u8>> {
        let mut index = self.repo.index().unwrap();
        // Reload from disk: gitchange ops write through a separate
        // repository handle.
        index.read(false).unwrap();
        let entry = index.get_path(Path::new(rel), 0)?;
        let blob = self.repo.find_blob(entry.id).unwrap();
        Some(blob.content().to_vec())
    }

    /// `git stash`: shelve tracked worktree + index changes.
    #[allow(dead_code)]
    pub fn stash(&mut self) -> &mut Self {
        let signature = self.repo.signature().unwrap();
        self.repo.stash_save(&signature, "wip", None).unwrap();
        self
    }

    /// `git stash pop`: restore the newest stash and drop it.
    #[allow(dead_code)]
    pub fn stash_pop(&mut self) -> &mut Self {
        self.repo.stash_apply(0, None).unwrap();
        self.repo.stash_drop(0).unwrap();
        self
    }

    /// Commit exactly what the index holds, worktree untouched — the
    /// external-partial-commit simulation (`git commit` without `-a`).
    #[allow(dead_code)]
    pub fn commit_index(&self, message: &str) -> &Self {
        let mut index = self.repo.index().unwrap();
        // Reload from disk: staging may have gone through another handle.
        index.read(false).unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = self.repo.find_tree(tree_id).unwrap();
        let signature = self.repo.signature().unwrap();
        let parent = self.repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        self.repo
            .commit(
                Some("HEAD"),
                &signature,
                &signature,
                message,
                &tree,
                &parents,
            )
            .unwrap();
        self
    }

    /// Create a branch named `name` at the current HEAD.
    #[allow(dead_code)]
    pub fn branch(&self, name: &str) -> &Self {
        let commit = self.repo.head().unwrap().peel_to_commit().unwrap();
        self.repo.branch(name, &commit, false).unwrap();
        self
    }

    /// Check out branch `name`, force-updating the worktree.
    #[allow(dead_code)]
    pub fn checkout(&self, name: &str) -> &Self {
        self.repo.set_head(&format!("refs/heads/{name}")).unwrap();
        let mut options = git2::build::CheckoutBuilder::new();
        options.force();
        self.repo.checkout_head(Some(&mut options)).unwrap();
        self
    }

    /// `git reset -- <rel>`: unstage one path — index entry := HEAD's,
    /// worktree untouched. The `git reset` half of ADR 0003's absorption
    /// rule, and the exact inverse of [`RepoFixture::stage`].
    #[allow(dead_code)]
    pub fn reset_path(&self, rel: &str) -> &Self {
        let head = self.repo.head().unwrap().peel_to_commit().unwrap();
        self.repo
            .reset_default(Some(head.as_object()), [rel])
            .unwrap();
        self
    }

    /// `git merge <name>` that must conflict: leaves MERGE_HEAD, a
    /// conflicted index, and conflict markers in the worktree — the
    /// mid-merge state ADR 0007's guard and quarantine act on.
    #[allow(dead_code)]
    pub fn merge_conflicting(&self, name: &str) -> &Self {
        let branch = self
            .repo
            .find_branch(name, git2::BranchType::Local)
            .unwrap();
        let annotated = self
            .repo
            .reference_to_annotated_commit(branch.get())
            .unwrap();
        self.repo.merge(&[&annotated], None, None).unwrap();
        assert!(
            self.repo.index().unwrap().has_conflicts(),
            "fixture merge must conflict"
        );
        self
    }

    /// What libgit2 makes of the repository's on-disk operation state —
    /// the raw input to core's `GitOperation` mapping (ADR 0007). Tests
    /// assert this alongside the mapped operation so a fixture that
    /// reaches the wrong state (or none) is caught before the guard
    /// assertion passes vacuously.
    #[allow(dead_code)]
    pub fn git_state(&self) -> git2::RepositoryState {
        self.repo.state()
    }

    /// Real git in the fixture repo, with the host's global and system
    /// config cut out (a nonexistent path reads as empty config) and
    /// both editors pinned to the shell no-op so an interactive rebase
    /// never blocks on one. Everything the operations below need is in
    /// the repo's own config, pinned by `new()`.
    ///
    /// The in-progress-operation builders drive real git rather than
    /// libgit2 because it is real git's on-disk leftovers the guard
    /// reads — `.git/rebase-apply` vs `.git/rebase-merge`, the
    /// sequencer's todo, `CHERRY_PICK_HEAD` — and libgit2 has no `git
    /// am` at all. ADR 0008's fixture rule is amended for exactly this.
    fn git_output(&self, args: &[&str]) -> Output {
        let absent = self.repo.path().join("absent-config");
        Command::new("git")
            .arg("-C")
            .arg(self.dir.path())
            .args(args)
            .env("GIT_CONFIG_GLOBAL", &absent)
            .env("GIT_CONFIG_SYSTEM", &absent)
            .env("GIT_EDITOR", ":")
            .env("GIT_SEQUENCE_EDITOR", ":")
            .output()
            .expect("spawn git")
    }

    /// Run git, panicking with its stderr on a non-zero exit.
    fn git(&self, args: &[&str]) -> String {
        let output = self.git_output(args);
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Run git expecting it to *stop* part-way — a replay that conflicts,
    /// a patch that won't apply. A clean exit means the operation ran to
    /// completion and the repo is no longer mid-operation, so the fixture
    /// never reached the state it promises: fail here rather than leave a
    /// guard assertion passing over a clean repo.
    fn git_must_stop(&self, args: &[&str]) {
        let output = self.git_output(args);
        assert!(
            !output.status.success(),
            "git {args:?} must stop mid-operation, but exited 0:\n{}",
            String::from_utf8_lossy(&output.stdout).trim()
        );
    }

    /// Switch to branch `name`, *carrying* dirty worktree changes across —
    /// the branch switch ADR 0002's scope semantics are stated against.
    /// Distinct from [`RepoFixture::checkout`], whose force flag would
    /// discard the very hunks the switch is meant to carry.
    ///
    /// Shells out (ADR 0008's amendment) because libgit2's safe checkout
    /// does not reproduce the switch: it updates the index but leaves the
    /// departing branch's files in the worktree, where they then read as
    /// untracked dirt and pollute the very snapshot under test. `checkout`
    /// rather than `switch` keeps the fixture inside the documented git
    /// floor (≥ 2.5; `switch` needs 2.23) — for a branch name the two are
    /// the same operation.
    #[allow(dead_code)]
    pub fn switch_branch(&self, name: &str) -> &Self {
        self.git(&["checkout", name]);
        self
    }

    /// `git rebase <backend> <onto>` that must conflict, leaving a rebase
    /// in progress. `backend` is the flag choosing git's backend —
    /// `--apply`, `--merge` or `--interactive` — which is what decides
    /// the underlying `RepositoryState`: the apply backend writes
    /// `.git/rebase-apply/rebasing`, the other two `.git/rebase-merge`.
    /// ADR 0007 collapses all of them to one `GitOperation::Rebase`,
    /// which is only worth asserting if the fixtures genuinely differ.
    ///
    /// The check below is fixture integrity — *a* rebase is in progress.
    /// Which arm each backend reaches is the caller's assertion, since
    /// that is the thing under test.
    #[allow(dead_code)]
    pub fn rebase_conflicting(&self, backend: &str, onto: &str) -> &Self {
        self.git_must_stop(&["rebase", backend, onto]);
        assert!(
            matches!(
                self.repo.state(),
                git2::RepositoryState::Rebase
                    | git2::RepositoryState::RebaseInteractive
                    | git2::RepositoryState::RebaseMerge
            ),
            "fixture rebase {backend} must leave a rebase in progress, got {:?}",
            self.repo.state()
        );
        self
    }

    /// `git cherry-pick <revs…>` that must conflict on the first rev.
    /// One rev leaves `CHERRY_PICK_HEAD` alone; several leave the
    /// sequencer's todo beside it, which is a different
    /// `RepositoryState` arm — hence the slice rather than a single rev.
    #[allow(dead_code)]
    pub fn cherry_pick_conflicting(&self, revs: &[&str]) -> &Self {
        let mut args = vec!["cherry-pick"];
        args.extend_from_slice(revs);
        self.git_must_stop(&args);
        assert!(
            matches!(
                self.repo.state(),
                git2::RepositoryState::CherryPick | git2::RepositoryState::CherryPickSequence
            ),
            "fixture cherry-pick must leave one in progress, got {:?}",
            self.repo.state()
        );
        self
    }

    /// `git revert <revs…>` that must conflict on the first rev. As with
    /// cherry-pick, several revs reach the sequence state and one does
    /// not. `--no-edit` keeps git from wanting a message editor.
    #[allow(dead_code)]
    pub fn revert_conflicting(&self, revs: &[&str]) -> &Self {
        let mut args = vec!["revert", "--no-edit"];
        args.extend_from_slice(revs);
        self.git_must_stop(&args);
        assert!(
            matches!(
                self.repo.state(),
                git2::RepositoryState::Revert | git2::RepositoryState::RevertSequence
            ),
            "fixture revert must leave one in progress, got {:?}",
            self.repo.state()
        );
        self
    }

    /// `git format-patch -1 <rev>` as a mailbox file, returned by path —
    /// the mailbox `am_conflicting` needs. It lands inside `.git`, where
    /// neither git nor the hunk universe will see it as an untracked
    /// worktree file.
    fn format_patch(&self, rev: &str) -> PathBuf {
        let mailbox = self.git(&["format-patch", "-1", "--stdout", rev]);
        assert!(!mailbox.is_empty(), "format-patch produced no mailbox");
        let path = self.repo.path().join("test-mailbox.patch");
        fs::write(&path, mailbox).unwrap();
        path
    }

    /// `git am` of `rev`'s patch, which must fail to apply: git stops
    /// with `.git/rebase-apply/applying` left behind — the `git am`
    /// state, distinct from the same directory's rebase form.
    #[allow(dead_code)]
    pub fn am_conflicting(&self, rev: &str) -> &Self {
        let mailbox = self.format_patch(rev);
        let mailbox = mailbox.to_str().expect("temp dir path is UTF-8");
        self.git_must_stop(&["am", mailbox]);
        assert!(
            matches!(
                self.repo.state(),
                git2::RepositoryState::ApplyMailbox | git2::RepositoryState::ApplyMailboxOrRebase
            ),
            "fixture am must leave one in progress, got {:?}",
            self.repo.state()
        );
        self
    }

    /// Conjure stage-1/2/3 conflict entries for `rel` without any merge
    /// in progress — the stash-pop-style unmerged state (quarantine with
    /// no operation pin). The worktree content is left untouched.
    #[allow(dead_code)]
    pub fn add_index_conflict(&self, rel: &str) -> &Self {
        let mut index = self.repo.index().unwrap();
        index.read(false).unwrap();
        let _ = index.remove(Path::new(rel), 0);
        for stage in 1..=3u16 {
            let content = format!("side {stage}\n");
            let blob = self.repo.blob(content.as_bytes()).unwrap();
            let entry = git2::IndexEntry {
                ctime: git2::IndexTime::new(0, 0),
                mtime: git2::IndexTime::new(0, 0),
                dev: 0,
                ino: 0,
                mode: 0o100644,
                uid: 0,
                gid: 0,
                file_size: content.len() as u32,
                id: blob,
                flags: stage << 12,
                flags_extended: 0,
                path: rel.as_bytes().to_vec(),
            };
            // `add` (unlike `add_frombuffer`) honours the stage bits.
            index.add(&entry).unwrap();
        }
        index.write().unwrap();
        assert!(index.has_conflicts(), "conjured conflict must register");
        self
    }

    /// Detach HEAD at the current commit, as `git checkout --detach`.
    #[allow(dead_code)]
    pub fn detach_head(&self) -> &Self {
        let oid = self.repo.head().unwrap().peel_to_commit().unwrap().id();
        self.repo.set_head_detached(oid).unwrap();
        self
    }

    /// Stage everything and commit it.
    pub fn commit_all(&self, message: &str) -> &Self {
        let mut index = self.repo.index().unwrap();
        index
            .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = self.repo.find_tree(tree_id).unwrap();
        let signature = self.repo.signature().unwrap();
        let parent = self.repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        self.repo
            .commit(
                Some("HEAD"),
                &signature,
                &signature,
                message,
                &tree,
                &parents,
            )
            .unwrap();
        self
    }
}
