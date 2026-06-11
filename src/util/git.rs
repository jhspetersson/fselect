//! Lazy per-repository cache backing the git-related fields.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use git2::{Repository, Sort, Status};

/// Information about the last commit that touched a particular file.
#[derive(Clone)]
pub struct CommitInfo {
    pub hash: String,
    /// Commit time as seconds since the Unix epoch.
    pub time: i64,
    pub author: String,
}

struct RepoEntry {
    repo: Repository,
    /// Canonicalized work tree root, used to derive repository-relative paths.
    workdir: PathBuf,
    /// Memoised branch name; the outer `Option` tracks whether it was computed.
    branch: Option<Option<String>>,
}

/// Caches opened repositories across files of the same traversal, plus the
/// most recent per-file lookups so that several git fields evaluated for the
/// same entry (e.g. in both WHERE and SELECT) don't repeat the work.
#[derive(Default)]
pub struct GitCache {
    /// Maps a directory as seen during traversal to the repository covering it
    /// and the directory's canonical path, or `None` when the directory does
    /// not belong to any work tree.
    dirs: HashMap<PathBuf, Option<(usize, PathBuf)>>,
    repos: Vec<RepoEntry>,
    last_status: Option<(PathBuf, Option<Status>)>,
    last_commit: Option<(PathBuf, Option<CommitInfo>)>,
}

impl GitCache {
    pub fn new() -> GitCache {
        Default::default()
    }

    /// Current branch of the repository containing `path` ("HEAD" when
    /// detached, the unborn branch name in an empty repository).
    pub fn branch(&mut self, path: &Path) -> Option<String> {
        let (idx, _) = self.locate(path)?;
        if self.repos[idx].branch.is_none() {
            self.repos[idx].branch = Some(compute_branch(&self.repos[idx].repo));
        }
        self.repos[idx].branch.clone().unwrap()
    }

    /// Work tree status flags of the file, `None` when the path is not inside
    /// a repository or the status can't be computed (e.g. for directories).
    pub fn status(&mut self, path: &Path) -> Option<Status> {
        if let Some((cached_path, status)) = &self.last_status
            && cached_path == path {
                return *status;
            }
        let status = self.compute_status(path);
        self.last_status = Some((path.to_path_buf(), status));
        status
    }

    pub fn is_tracked(&mut self, path: &Path) -> Option<bool> {
        let status = self.status(path)?;
        Some(!status.is_wt_new() && !status.is_ignored())
    }

    pub fn is_ignored(&mut self, path: &Path) -> Option<bool> {
        let (idx, rel) = self.locate(path)?;
        self.repos[idx].repo.is_path_ignored(to_git_path(&rel)).ok()
    }

    /// The last commit that touched the file, like `git log -1 -- path`.
    pub fn last_commit(&mut self, path: &Path) -> Option<CommitInfo> {
        if let Some((cached_path, commit)) = &self.last_commit
            && cached_path == path {
                return commit.clone();
            }
        let commit = self.locate(path).and_then(|(idx, rel)| {
            find_last_commit(&self.repos[idx].repo, Path::new(&to_git_path(&rel)))
        });
        self.last_commit = Some((path.to_path_buf(), commit.clone()));
        commit
    }

    fn compute_status(&mut self, path: &Path) -> Option<Status> {
        let (idx, rel) = self.locate(path)?;
        self.repos[idx].repo.status_file(Path::new(&to_git_path(&rel))).ok()
    }

    /// Resolves the repository covering `path` and the path relative to its
    /// work tree root. Only the parent directory is canonicalized (and cached),
    /// so the cost is paid once per directory rather than once per file.
    fn locate(&mut self, path: &Path) -> Option<(usize, PathBuf)> {
        let parent = path.parent()?;
        let file_name = path.file_name()?;

        if !self.dirs.contains_key(parent) {
            let resolved = fs::canonicalize(parent).ok().and_then(|canonical| {
                let repo_idx = self.repo_index_for(&canonical)?;
                Some((repo_idx, canonical))
            });
            self.dirs.insert(parent.to_path_buf(), resolved);
        }

        let (repo_idx, canonical) = self.dirs.get(parent)?.as_ref()?;
        let rel = canonical
            .join(file_name)
            .strip_prefix(&self.repos[*repo_idx].workdir)
            .ok()?
            .to_path_buf();
        Some((*repo_idx, rel))
    }

    fn repo_index_for(&mut self, canonical_dir: &Path) -> Option<usize> {
        // Discovery has to run per directory (a nested repository or submodule
        // takes precedence over an enclosing one), but repositories themselves
        // are deduplicated by work tree root.
        let repo = Repository::discover(canonical_dir).ok()?;
        let workdir = repo.workdir()?.to_path_buf();
        let workdir = fs::canonicalize(&workdir).unwrap_or(workdir);

        if let Some(idx) = self.repos.iter().position(|r| r.workdir == workdir) {
            return Some(idx);
        }

        self.repos.push(RepoEntry {
            repo,
            workdir,
            branch: None,
        });
        Some(self.repos.len() - 1)
    }
}

pub fn status_to_string(status: Status) -> &'static str {
    if status.is_conflicted() {
        "conflicted"
    } else if status.intersects(
        Status::INDEX_NEW
            | Status::INDEX_MODIFIED
            | Status::INDEX_DELETED
            | Status::INDEX_RENAMED
            | Status::INDEX_TYPECHANGE,
    ) {
        "staged"
    } else if status.intersects(
        Status::WT_MODIFIED | Status::WT_DELETED | Status::WT_RENAMED | Status::WT_TYPECHANGE,
    ) {
        "modified"
    } else if status.is_wt_new() {
        "untracked"
    } else if status.is_ignored() {
        "ignored"
    } else {
        "clean"
    }
}

fn compute_branch(repo: &Repository) -> Option<String> {
    match repo.head() {
        Ok(head) => head.shorthand().ok().map(String::from),
        // An unborn branch (fresh repository without commits) still has a
        // symbolic HEAD pointing at the future branch name.
        Err(_) => repo
            .find_reference("HEAD")
            .ok()
            .and_then(|r| r.symbolic_target().ok().flatten().map(String::from))
            .map(|target| {
                target
                    .strip_prefix("refs/heads/")
                    .map(String::from)
                    .unwrap_or(target)
            }),
    }
}

/// Walks the history from HEAD and returns the newest commit whose tree entry
/// for `rel` differs from all of its parents, emulating `git log -1 -- path`.
fn find_last_commit(repo: &Repository, rel: &Path) -> Option<CommitInfo> {
    let mut revwalk = repo.revwalk().ok()?;
    revwalk.push_head().ok()?;
    let _ = revwalk.set_sorting(Sort::TIME);

    for oid in revwalk.flatten() {
        let Ok(commit) = repo.find_commit(oid) else {
            continue;
        };
        let Ok(tree) = commit.tree() else {
            continue;
        };
        let entry_id = tree.get_path(rel).ok().map(|e| e.id());

        let touched = if commit.parent_count() == 0 {
            entry_id.is_some()
        } else {
            commit.parents().all(|parent| {
                let parent_id = parent
                    .tree()
                    .ok()
                    .and_then(|t| t.get_path(rel).ok())
                    .map(|e| e.id());
                parent_id != entry_id
            })
        };

        if touched {
            let author = commit.author();
            let author_name = author
                .name()
                .ok()
                .or_else(|| author.email().ok())
                .map(String::from)
                .unwrap_or_default();
            return Some(CommitInfo {
                hash: commit.id().to_string(),
                // Author time, to match what `git log` displays by default.
                time: author.when().seconds(),
                author: author_name,
            });
        }
    }

    None
}

/// git2 expects repository-relative paths with forward slashes.
fn to_git_path(rel: &Path) -> String {
    let s = rel.to_string_lossy();
    if cfg!(windows) {
        s.replace('\\', "/")
    } else {
        s.into_owned()
    }
}
