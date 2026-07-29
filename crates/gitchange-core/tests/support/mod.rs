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
