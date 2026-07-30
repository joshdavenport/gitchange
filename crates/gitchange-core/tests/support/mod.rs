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
        }
        Self { dir, repo }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Write `content` to a repo-relative path, creating parent dirs.
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

    /// Stage one path, exactly as `git add <rel>` would.
    #[allow(dead_code)]
    pub fn stage(&self, rel: &str) -> &Self {
        let mut index = self.repo.index().unwrap();
        index.add_path(Path::new(rel)).unwrap();
        index.write().unwrap();
        self
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
