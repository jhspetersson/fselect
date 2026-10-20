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
    tracked: Option<TrackedPaths>,
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
    /// Paths that must not resolve to a not-yet-processed commit because a
    /// merge above it was tree-same to another parent for them.
    blocked: HashMap<git2::Oid, PathSet>,
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
        let rel = to_git_path(&rel);
        let RepoEntry { repo, tracked, .. } = &mut self.repos[idx];
        if tracked.get_or_insert_with(|| TrackedPaths::load(repo)).contains(&rel) {
            return Some(false);
        }
        repo.is_path_ignored(rel).ok()
    }

    /// The last commit that touched the file (or directory), like
    /// `git log -1 -- path`.
    pub fn last_commit(&mut self, path: &Path) -> Option<CommitInfo> {
        if let Some((cached_path, commit)) = &self.last_commit
            && cached_path == path {
                return commit.clone();
            }
        let commit = self.locate(path).and_then(|(idx, rel)| match rel.as_os_str().is_empty() {
            true => head_commit_info(&self.repos[idx].repo),
            false => resolve_last_commit(&mut self.repos[idx], &to_git_path(&rel)),
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
        if path.join(".git").exists() {
            return self.locate_root(path);
        }
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

    fn locate_root(&mut self, root: &Path) -> Option<(usize, PathBuf)> {
        if !self.dirs.contains_key(root) {
            let resolved = fs::canonicalize(root).ok().and_then(|canonical| {
                let repo_idx = self.repo_index_for(&canonical)?;
                Some((repo_idx, canonical))
            });
            self.dirs.insert(root.to_path_buf(), resolved);
        }
        let (repo_idx, _) = self.dirs.get(root)?.as_ref()?;
        Some((*repo_idx, PathBuf::new()))
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
            tracked: None,
            branch: None,
            history: HistoryScan::default(),
        });
        Some(self.repos.len() - 1)
    }
}

/// Ignore rules of one repository, applied the way `git status` does: a path
/// that is already in the index, or a directory holding such paths, is never
/// ignored, whatever the patterns say.
pub struct IgnoreRules {
    repo: Repository,
    workdir: PathBuf,
    tracked: TrackedPaths,
}

impl IgnoreRules {
    pub fn open(dir: impl AsRef<Path>) -> Option<IgnoreRules> {
        Repository::open(dir).ok().and_then(IgnoreRules::from_repository)
    }

    pub fn discover(dir: impl AsRef<Path>) -> Option<IgnoreRules> {
        Repository::discover(dir).ok().and_then(IgnoreRules::from_repository)
    }

    fn from_repository(repo: Repository) -> Option<IgnoreRules> {
        let workdir = repo.workdir()?.to_path_buf();
        let workdir = PathBuf::from(crate::util::canonical_path(&workdir).unwrap_or_else(|_| workdir.to_string_lossy().into_owned()));
        let tracked = TrackedPaths::load(&repo);
        Some(IgnoreRules { repo, workdir, tracked })
    }

    pub fn is_ignored(&self, path: &Path) -> bool {
        if let Ok(rel) = path.strip_prefix(&self.workdir)
            && self.tracked.contains(&to_git_path(rel))
        {
            return false;
        }
        self.repo.is_path_ignored(path).unwrap_or(false)
    }
}

/// Sorted repository-relative paths of the index, lowercased when the
/// repository has `core.ignorecase` so a case-only rename still matches.
struct TrackedPaths {
    paths: Vec<String>,
    ignore_case: bool,
}

impl TrackedPaths {
    fn load(repo: &Repository) -> TrackedPaths {
        let ignore_case = repo.config().and_then(|config| config.get_bool("core.ignorecase")).unwrap_or(false);
        let fold = |path: String| if ignore_case { path.to_ascii_lowercase() } else { path };
        let mut paths: Vec<String> = repo
            .index()
            .map(|index| index.iter().map(|entry| fold(String::from_utf8_lossy(&entry.path).into_owned())).collect())
            .unwrap_or_default();
        paths.sort();
        paths.dedup();
        TrackedPaths { paths, ignore_case }
    }

    fn contains(&self, rel: &str) -> bool {
        if rel.is_empty() {
            return !self.paths.is_empty();
        }
        let folded = if self.ignore_case { rel.to_ascii_lowercase() } else { rel.to_string() };
        let rel = folded.as_str();
        let at = self.paths.partition_point(|path| path.as_str() < rel);
        if self.paths.get(at).is_some_and(|path| path == rel) {
            return true;
        }
        let prefix = format!("{rel}/");
        let at = self.paths.partition_point(|path| path.as_str() < prefix.as_str());
        self.paths.get(at).is_some_and(|path| path.starts_with(&prefix))
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
fn commit_info(commit: &git2::Commit) -> CommitInfo {
    let author = commit.author();
    let author_name = author
        .name()
        .ok()
        .or_else(|| author.email().ok())
        .map(String::from)
        .unwrap_or_default();
    CommitInfo {
        hash: commit.id().to_string(),
        // Author time, to match what `git log` displays by default.
        time: author.when().seconds(),
        author: author_name,
    }
}

fn head_commit_info(repo: &Repository) -> Option<CommitInfo> {
    let commit = repo.head().ok()?.peel_to_commit().ok()?;
    Some(commit_info(&commit))
}

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
    let blocked = entry.history.blocked.remove(&oid).unwrap_or_default();

    let Ok(commit) = repo.find_commit(oid) else {
        return true;
    };
    let changes = commit_changes(repo, &commit);
    let touched = changes.touched();
    for (parent, pruned) in commit.parent_ids().zip(changes.pruned_per_parent()) {
        let inherited = blocked.clone().union(pruned);
        entry
            .history
            .blocked
            .entry(parent)
            .and_modify(|existing| *existing = std::mem::take(existing).intersection(inherited.clone()))
            .or_insert(inherited);
    }
    if touched.is_empty() {
        return true;
    }

    let info = Rc::new(commit_info(&commit));

    for path in touched {
        if blocked.contains(&path) {
            continue;
        }
        entry
            .history
            .resolved
            .entry(path)
            .or_insert_with(|| info.clone());
    }

    true
}

/// A set of repository paths that is either finite or the complement of a
/// finite set, so that "everything except what a merge changed" stays cheap.
#[derive(Clone, Debug)]
enum PathSet {
    Only(HashSet<String>),
    AllExcept(HashSet<String>),
}

impl Default for PathSet {
    fn default() -> PathSet {
        PathSet::Only(HashSet::new())
    }
}

impl PathSet {
    fn all() -> PathSet {
        PathSet::AllExcept(HashSet::new())
    }

    fn contains(&self, path: &str) -> bool {
        match self {
            PathSet::Only(set) => set.contains(path),
            PathSet::AllExcept(set) => !set.contains(path),
        }
    }

    fn complement(self) -> PathSet {
        match self {
            PathSet::Only(set) => PathSet::AllExcept(set),
            PathSet::AllExcept(set) => PathSet::Only(set),
        }
    }

    fn union(self, other: PathSet) -> PathSet {
        match (self, other) {
            (PathSet::Only(a), PathSet::Only(b)) => PathSet::Only(&a | &b),
            (PathSet::Only(a), PathSet::AllExcept(b)) | (PathSet::AllExcept(b), PathSet::Only(a)) => {
                PathSet::AllExcept(&b - &a)
            }
            (PathSet::AllExcept(a), PathSet::AllExcept(b)) => PathSet::AllExcept(&a & &b),
        }
    }

    fn intersection(self, other: PathSet) -> PathSet {
        match (self, other) {
            (PathSet::Only(a), PathSet::Only(b)) => PathSet::Only(&a & &b),
            (PathSet::Only(a), PathSet::AllExcept(b)) | (PathSet::AllExcept(b), PathSet::Only(a)) => {
                PathSet::Only(&a - &b)
            }
            (PathSet::AllExcept(a), PathSet::AllExcept(b)) => PathSet::AllExcept(&a | &b),
        }
    }
}

/// What a commit changed relative to each parent (with every ancestor
/// directory of each changed path included), or the whole tree for a root
/// commit.
enum CommitChanges {
    Root(Vec<String>),
    PerParent(Vec<HashSet<String>>),
}

impl CommitChanges {
    /// The paths whose content differs between the commit and *all* of its
    /// parents, which is what `git log` lists the commit for.
    fn touched(&self) -> Vec<String> {
        match self {
            CommitChanges::Root(paths) => paths.clone(),
            CommitChanges::PerParent(changes) => {
                let mut intersection: Option<HashSet<String>> = None;
                for changed in changes {
                    intersection = Some(match intersection {
                        None => changed.clone(),
                        Some(previous) => &previous & changed,
                    });
                }
                intersection.map(Vec::from_iter).unwrap_or_default()
            }
        }
    }

    /// `git log`'s default history simplification: when a merge leaves a path
    /// identical to one of its parents, only the first such parent is
    /// followed for that path. The result holds, per parent, the paths whose
    /// history must not be resolved through that parent.
    fn pruned_per_parent(&self) -> Vec<PathSet> {
        let CommitChanges::PerParent(changes) = self else {
            return Vec::new();
        };
        (0..changes.len())
            .map(|j| {
                let same_to_earlier = changes[..j]
                    .iter()
                    .fold(PathSet::all(), |acc, changed| acc.intersection(PathSet::Only(changed.clone())))
                    .complement();
                let same_to_later = changes[j + 1..]
                    .iter()
                    .fold(PathSet::all(), |acc, changed| acc.intersection(PathSet::Only(changed.clone())))
                    .complement();
                same_to_earlier.union(PathSet::Only(changes[j].clone()).intersection(same_to_later))
            })
            .collect()
    }
}

fn commit_changes(repo: &Repository, commit: &Commit) -> CommitChanges {
    let Ok(tree) = commit.tree() else {
        return CommitChanges::PerParent(Vec::new());
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
        return CommitChanges::Root(paths);
    }

    let mut changes = Vec::new();
    for parent in commit.parents() {
        let mut changed = HashSet::new();
        if let Ok(parent_tree) = parent.tree()
            && let Ok(diff) = repo.diff_tree_to_tree(Some(&parent_tree), Some(&tree), None)
        {
            for delta in diff.deltas() {
                for path in [delta.old_file().path(), delta.new_file().path()]
                    .into_iter()
                    .flatten()
                {
                    add_with_ancestors(&mut changed, &path.to_string_lossy());
                }
            }
        }
        changes.push(changed);
    }
    CommitChanges::PerParent(changes)
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

    fn init_on_branch(dir: &Path, branch: &str, file: &str) -> Repository {
        fs::create_dir_all(dir).unwrap();
        let repo = Repository::init(dir).unwrap();
        repo.set_head(&format!("refs/heads/{branch}")).unwrap();
        fs::write(dir.join(file), "x").unwrap();
        commit_all(&repo, "init", 1_000_000_000);
        repo
    }

    #[test]
    fn tracked_paths_are_never_ignored() {
        let dir = std::env::temp_dir().join("fselect_test_git_tracked_not_ignored");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("logs")).unwrap();
        let repo = Repository::init(&dir).unwrap();
        fs::write(dir.join("tracked.log"), "t").unwrap();
        fs::write(dir.join("logs").join("keep.log"), "k").unwrap();
        commit_all(&repo, "c1", 1_000_000_000);
        fs::write(dir.join(".gitignore"), "*.log\nlogs/\n").unwrap();
        fs::write(dir.join("loose.log"), "l").unwrap();
        fs::write(dir.join("logs").join("new.log"), "n").unwrap();

        let mut cache = GitCache::new();
        assert_eq!(cache.is_ignored(&dir.join("tracked.log")), Some(false));
        assert_eq!(cache.is_ignored(&dir.join("loose.log")), Some(true));
        assert_eq!(cache.is_ignored(&dir.join("logs")), Some(false));
        assert_eq!(cache.is_ignored(&dir.join("logs").join("keep.log")), Some(false));
        assert_eq!(cache.is_ignored(&dir.join("logs").join("new.log")), Some(true));

        let rules = IgnoreRules::discover(&dir).unwrap();
        let canonical = PathBuf::from(crate::util::canonical_path(&dir).unwrap());
        assert!(!rules.is_ignored(&canonical.join("tracked.log")));
        assert!(rules.is_ignored(&canonical.join("loose.log")));
        assert!(!rules.is_ignored(&canonical.join("logs")));
        assert!(rules.is_ignored(&canonical.join("logs").join("new.log")));

        drop(repo);
        drop(cache);
        drop(rules);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tracked_paths_fold_case_when_the_repository_ignores_case() {
        let dir = std::env::temp_dir().join("fselect_test_git_tracked_icase");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("logs")).unwrap();
        let repo = Repository::init(&dir).unwrap();
        if !repo.config().and_then(|c| c.get_bool("core.ignorecase")).unwrap_or(false) {
            drop(repo);
            let _ = fs::remove_dir_all(&dir);
            return;
        }
        fs::write(dir.join("tracked.log"), "t").unwrap();
        fs::write(dir.join("logs").join("keep.log"), "k").unwrap();
        commit_all(&repo, "c1", 1_000_000_000);
        fs::write(dir.join(".gitignore"), "*.log\nlogs/\n").unwrap();
        fs::rename(dir.join("tracked.log"), dir.join("TRACKED.log")).unwrap();
        fs::rename(dir.join("logs"), dir.join("LOGS")).unwrap();
        fs::write(dir.join("LOGS").join("new.log"), "n").unwrap();

        let mut cache = GitCache::new();
        assert_eq!(cache.is_ignored(&dir.join("TRACKED.log")), Some(false));
        assert_eq!(cache.is_ignored(&dir.join("LOGS")), Some(false));
        assert_eq!(cache.is_ignored(&dir.join("LOGS").join("keep.log")), Some(false));
        assert_eq!(cache.is_ignored(&dir.join("LOGS").join("new.log")), Some(true));

        let rules = IgnoreRules::discover(&dir).unwrap();
        let canonical = PathBuf::from(crate::util::canonical_path(&dir).unwrap());
        assert!(!rules.is_ignored(&canonical.join("TRACKED.log")));
        assert!(!rules.is_ignored(&canonical.join("LOGS")));
        assert!(rules.is_ignored(&canonical.join("LOGS").join("new.log")));

        drop(repo);
        drop(cache);
        drop(rules);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn repository_directories_report_their_own_repository() {
        let dir = std::env::temp_dir().join("fselect_test_git_repo_dirs");
        let _ = fs::remove_dir_all(&dir);
        let outer = dir.join("outer");
        let nested = outer.join("nested");
        let plain = dir.join("plain");
        init_on_branch(&outer, "main", "top.txt");
        init_on_branch(&nested, "feature", "in.txt");
        init_on_branch(&plain, "solo", "p.txt");

        let mut cache = GitCache::new();
        let branches: Vec<(String, Option<String>)> = [
            ("outer", outer.clone()),
            ("outer/nested", nested.clone()),
            ("plain", plain.clone()),
            ("outer/top.txt", outer.join("top.txt")),
            ("outer/nested/in.txt", nested.join("in.txt")),
            ("plain/p.txt", plain.join("p.txt")),
            ("dir", dir.clone()),
        ]
        .iter()
        .map(|(label, path)| (label.to_string(), cache.branch(path)))
        .collect();
        let _ = fs::remove_dir_all(&dir);

        let expected = [
            ("outer", Some("main")), ("outer/nested", Some("feature")), ("plain", Some("solo")),
            ("outer/top.txt", Some("main")), ("outer/nested/in.txt", Some("feature")), ("plain/p.txt", Some("solo")),
            ("dir", None),
        ];
        for ((label, actual), (_, wanted)) in branches.iter().zip(expected) {
            assert_eq!(actual.as_deref(), wanted, "{}", label);
        }
    }

    #[test]
    fn repository_root_directories_report_head_as_their_last_commit() {
        let dir = std::env::temp_dir().join("fselect_test_git_root_last_commit");
        let _ = fs::remove_dir_all(&dir);
        let outer = dir.join("outer");
        let nested = outer.join("nested");
        let outer_repo = init_on_branch(&outer, "main", "top.txt");
        fs::write(outer.join("later.txt"), "y").unwrap();
        let outer_head = commit_all(&outer_repo, "later", 1_000_000_100);
        let nested_repo = init_on_branch(&nested, "feature", "in.txt");
        let nested_head = nested_repo.head().unwrap().peel_to_commit().unwrap().id();

        let mut cache = GitCache::new();
        let outer_commit = cache.last_commit(&outer);
        let nested_commit = cache.last_commit(&nested);
        let plain_dir = cache.last_commit(&dir);
        let file_commit = cache.last_commit(&outer.join("top.txt"));
        let _ = fs::remove_dir_all(&dir);

        assert_eq!(outer_commit.map(|c| c.hash), Some(outer_head.to_string()), "the root reports HEAD");
        assert_eq!(nested_commit.map(|c| c.hash), Some(nested_head.to_string()), "a nested root reports its own HEAD");
        assert!(plain_dir.is_none());
        assert_ne!(file_commit.map(|c| c.hash), Some(outer_head.to_string()), "files still resolve per path");
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

    fn commit_tree(repo: &Repository, msg: &str, tree: &git2::Tree, parents: &[&git2::Commit], secs: i64) -> git2::Oid {
        let sig = Signature::new("test", "test@test", &Time::new(secs, 0)).unwrap();
        let update_ref = if parents.len() > 1 { Some("HEAD") } else { None };
        repo.commit(update_ref, &sig, &sig, msg, tree, parents).unwrap()
    }

    fn tree_with<'r>(repo: &'r Repository, base: &git2::Tree, files: &[(&str, &str)]) -> git2::Tree<'r> {
        let mut builder = repo.treebuilder(Some(base)).unwrap();
        for (name, content) in files {
            let blob = repo.blob(content.as_bytes()).unwrap();
            builder.insert(name, blob, 0o100644).unwrap();
        }
        let oid = builder.write().unwrap();
        repo.find_tree(oid).unwrap()
    }

    #[test]
    fn last_commit_ignores_side_branches_discarded_by_a_merge() {
        let dir = std::env::temp_dir().join("fselect_test_git_merge_ours");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let repo = Repository::init(&dir).unwrap();

        fs::write(dir.join("file.txt"), "main").unwrap();
        fs::write(dir.join("other.txt"), "main").unwrap();
        let c1 = commit_all(&repo, "c1", 1_000_000_000);
        let ours = {
            let main = repo.find_commit(c1).unwrap();
            let main_tree = main.tree().unwrap();
            let side_tree = tree_with(&repo, &main_tree, &[("file.txt", "side")]);
            let c2 = commit_tree(&repo, "side", &side_tree, &[&main], 1_000_000_100);
            let side = repo.find_commit(c2).unwrap();
            commit_tree(&repo, "merge -s ours", &main_tree, &[&main, &side], 1_000_000_200)
        };

        let mut cache = GitCache::new();
        assert_eq!(cache.last_commit(&dir.join("file.txt")).unwrap().hash, c1.to_string());
        assert_eq!(cache.last_commit(&dir.join("other.txt")).unwrap().hash, c1.to_string());
        assert_eq!(cache.last_commit(&dir).unwrap().hash, ours.to_string());

        drop(repo);
        drop(cache);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn last_commit_follows_both_sides_of_a_real_merge() {
        let dir = std::env::temp_dir().join("fselect_test_git_merge_both");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let repo = Repository::init(&dir).unwrap();

        for name in ["base.txt", "main.txt", "side.txt", "both.txt"] {
            fs::write(dir.join(name), "base").unwrap();
        }
        let c1 = commit_all(&repo, "c1", 1_000_000_000);
        let (c_side, c_main, c_merge) = {
            let base = repo.find_commit(c1).unwrap();
            let base_tree = base.tree().unwrap();
            let side_tree = tree_with(&repo, &base_tree, &[("side.txt", "side"), ("both.txt", "side")]);
            let c_side = commit_tree(&repo, "side", &side_tree, &[&base], 1_000_000_100);
            let side = repo.find_commit(c_side).unwrap();
            let main_tree = tree_with(&repo, &base_tree, &[("main.txt", "main"), ("both.txt", "main")]);
            let c_main = commit_tree(&repo, "main", &main_tree, &[&base], 1_000_000_200);
            repo.reference("HEAD", c_main, true, "main").unwrap();
            let main = repo.find_commit(c_main).unwrap();
            let merged_tree = tree_with(&repo, &main_tree, &[("side.txt", "side"), ("both.txt", "merged")]);
            (c_side, c_main, commit_tree(&repo, "merge", &merged_tree, &[&main, &side], 1_000_000_300))
        };

        let mut cache = GitCache::new();
        let last = |cache: &mut GitCache, name: &str| cache.last_commit(&dir.join(name)).unwrap().hash;
        assert_eq!(last(&mut cache, "base.txt"), c1.to_string());
        assert_eq!(last(&mut cache, "main.txt"), c_main.to_string());
        assert_eq!(last(&mut cache, "side.txt"), c_side.to_string());
        assert_eq!(last(&mut cache, "both.txt"), c_merge.to_string());

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
