//! Shared test support (ADR 0008): programmatically built temp repos,
//! one per test. Grows `with_hook()` and staged-state helpers with later
//! tickets.

use std::fs;
use std::path::Path;

use tempfile::TempDir;

pub struct RepoFixture {
    dir: TempDir,
    repo: git2::Repository,
}

impl RepoFixture {
    /// Init a fresh repo in a temp dir with a committing identity set.
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("create temp dir");
        let repo = git2::Repository::init(dir.path()).expect("git init");
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
