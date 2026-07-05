//! Lazy per-repository cache backing the git-related fields.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use git2::{Commit, ObjectType, Repository, Sort, Status, TreeWalkMode, TreeWalkResult};

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
    history: HistoryScan,
}

/// Incremental "last commit that touched each path" resolver. History is
/// walked from HEAD at most once per repository, no matter how many files are
/// queried: each processed commit records itself as the answer for every path
/// it touched (first writer wins, and the walk is newest-first). A per-file
/// `git log -1 -- path` emulation would instead cost
/// O(files x history x tree depth).
#[derive(Default)]
struct HistoryScan {
    /// Commit ids from HEAD in time order; collected lazily on first use
    /// (a plain revwalk without loading commits is cheap even for large
    /// histories, and avoids a self-referential borrow of `repo`).
    oids: Option<Vec<git2::Oid>>,
    /// Index of the next unprocessed commit in `oids`.
    next: usize,
    /// Resolved last-touch info per repository-relative path (and per
    /// directory: a commit touching `a/b/c.rs` also touches `a` and `a/b`).
    resolved: HashMap<String, Rc<CommitInfo>>,
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

    /// The last commit that touched the file (or directory), like
    /// `git log -1 -- path`.
    pub fn last_commit(&mut self, path: &Path) -> Option<CommitInfo> {
        if let Some((cached_path, commit)) = &self.last_commit
            && cached_path == path {
                return commit.clone();
            }
        let commit = self.locate(path).and_then(|(idx, rel)| {
            resolve_last_commit(&mut self.repos[idx], &to_git_path(&rel))
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
            history: HistoryScan::default(),
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

/// Advances the repository's history scan until `key` resolves or the walk is
/// exhausted (then the path was never committed).
fn resolve_last_commit(entry: &mut RepoEntry, key: &str) -> Option<CommitInfo> {
    loop {
        if let Some(info) = entry.history.resolved.get(key) {
            return Some((**info).clone());
        }
        if !advance_history(entry) {
            return None;
        }
    }
}

/// Processes the next unseen commit of the walk, recording it as the
/// last-touch answer for every path it changed. Returns false once history is
/// exhausted.
fn advance_history(entry: &mut RepoEntry) -> bool {
    let repo = &entry.repo;
    let oids = entry.history.oids.get_or_insert_with(|| {
        let mut oids = Vec::new();
        if let Ok(mut revwalk) = repo.revwalk()
            && revwalk.push_head().is_ok()
        {
            // Topological + time matches `git log` ordering; time alone
            // tie-breaks same-second commits arbitrarily, which would pick
            // the wrong "last" commit within such a chain.
            let _ = revwalk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME);
            oids = revwalk.flatten().collect();
        }
        oids
    });

    let Some(&oid) = oids.get(entry.history.next) else {
        return false;
    };
    entry.history.next += 1;

    let Ok(commit) = repo.find_commit(oid) else {
        return true;
    };
    let touched = commit_touched_paths(repo, &commit);
    if touched.is_empty() {
        return true;
    }

    let author = commit.author();
    let author_name = author
        .name()
        .ok()
        .or_else(|| author.email().ok())
        .map(String::from)
        .unwrap_or_default();
    let info = Rc::new(CommitInfo {
        hash: commit.id().to_string(),
        // Author time, to match what `git log` displays by default.
        time: author.when().seconds(),
        author: author_name,
    });

    for path in touched {
        entry
            .history
            .resolved
            .entry(path)
            .or_insert_with(|| info.clone());
    }

    true
}

/// The paths whose content differs between the commit and *all* of its parents
/// (matching `git log`'s default no-simplification behavior for a single
/// pathspec), plus every ancestor directory of each, so that directory
/// queries resolve to the newest commit touching anything beneath them. The
/// root commit touches everything in its tree.
fn commit_touched_paths(repo: &Repository, commit: &Commit) -> Vec<String> {
    let Ok(tree) = commit.tree() else {
        return Vec::new();
    };

    if commit.parent_count() == 0 {
        let mut paths = Vec::new();
        let _ = tree.walk(TreeWalkMode::PreOrder, |dir, entry| {
            if (entry.kind() == Some(ObjectType::Blob) || entry.kind() == Some(ObjectType::Tree))
                && let Ok(name) = entry.name()
            {
                paths.push(format!("{}{}", dir, name));
            }
            TreeWalkResult::Ok
        });
        return paths;
    }

    let mut intersection: Option<HashSet<String>> = None;
    for parent in commit.parents() {
        let Ok(parent_tree) = parent.tree() else {
            continue;
        };
        let Ok(diff) = repo.diff_tree_to_tree(Some(&parent_tree), Some(&tree), None) else {
            continue;
        };

        let mut changed = HashSet::new();
        for delta in diff.deltas() {
            for path in [delta.old_file().path(), delta.new_file().path()]
                .into_iter()
                .flatten()
            {
                add_with_ancestors(&mut changed, &path.to_string_lossy());
            }
        }

        intersection = Some(match intersection {
            None => changed,
            Some(previous) => previous.intersection(&changed).cloned().collect(),
        });
    }

    intersection.map(Vec::from_iter).unwrap_or_default()
}

/// Inserts a '/'-separated path and each of its parent directories.
fn add_with_ancestors(set: &mut HashSet<String>, path: &str) {
    let mut end = path.len();
    loop {
        set.insert(path[..end].to_string());
        match path[..end].rfind('/') {
            Some(idx) if idx > 0 => end = idx,
            _ => break,
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use git2::{IndexAddOption, Signature, Time};

    /// Stages everything and commits with a distinct timestamp — the walk is
    /// time-sorted, so equal-second commits would tie-break arbitrarily.
    fn commit_all(repo: &Repository, msg: &str, secs: i64) -> git2::Oid {
        let sig = Signature::new("test", "test@test", &Time::new(secs, 0)).unwrap();
        let mut index = repo.index().unwrap();
        index
            .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &parents)
            .unwrap()
    }

    #[test]
    fn last_commit_resolves_per_path() {
        let dir = std::env::temp_dir().join("fselect_test_git_last_commit");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub")).unwrap();
        let repo = Repository::init(&dir).unwrap();

        fs::write(dir.join("a.txt"), "one").unwrap();
        let c1 = commit_all(&repo, "c1", 1_000_000_000);

        fs::write(dir.join("b.txt"), "two").unwrap();
        let c2 = commit_all(&repo, "c2", 1_000_000_100);

        fs::write(dir.join("sub").join("c.txt"), "three").unwrap();
        fs::write(dir.join("a.txt"), "one-modified").unwrap();
        let c3 = commit_all(&repo, "c3", 1_000_000_200);

        fs::write(dir.join("untracked.txt"), "x").unwrap();

        let mut cache = GitCache::new();
        assert_eq!(
            cache.last_commit(&dir.join("a.txt")).unwrap().hash,
            c3.to_string(),
            "modified file resolves to the modifying commit"
        );
        assert_eq!(
            cache.last_commit(&dir.join("b.txt")).unwrap().hash,
            c2.to_string(),
            "untouched-since file resolves to its introducing commit"
        );
        assert_eq!(
            cache.last_commit(&dir.join("sub").join("c.txt")).unwrap().hash,
            c3.to_string()
        );
        assert_eq!(
            cache.last_commit(&dir.join("sub")).unwrap().hash,
            c3.to_string(),
            "directory resolves to the newest commit under it"
        );
        assert!(
            cache.last_commit(&dir.join("untracked.txt")).is_none(),
            "never-committed file has no last commit"
        );
        // Second query for the same repository must reuse the finished walk.
        assert_eq!(
            cache.last_commit(&dir.join("a.txt")).unwrap().hash,
            c3.to_string()
        );
        let _ = c1;

        drop(repo);
        drop(cache);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn last_commit_root_commit_touches_all_paths() {
        let dir = std::env::temp_dir().join("fselect_test_git_root_commit");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("nested")).unwrap();
        let repo = Repository::init(&dir).unwrap();

        fs::write(dir.join("top.txt"), "t").unwrap();
        fs::write(dir.join("nested").join("deep.txt"), "d").unwrap();
        let c1 = commit_all(&repo, "root", 1_000_000_000);

        let mut cache = GitCache::new();
        assert_eq!(
            cache.last_commit(&dir.join("top.txt")).unwrap().hash,
            c1.to_string()
        );
        assert_eq!(
            cache
                .last_commit(&dir.join("nested").join("deep.txt"))
                .unwrap()
                .hash,
            c1.to_string()
        );
        assert_eq!(
            cache.last_commit(&dir.join("nested")).unwrap().hash,
            c1.to_string()
        );

        drop(repo);
        drop(cache);
        let _ = fs::remove_dir_all(&dir);
    }
}
