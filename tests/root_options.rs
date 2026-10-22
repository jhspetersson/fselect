use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn fselect(query: &str) -> Run {
    fselect_with(&[], query)
}

fn fselect_with(flags: &[&str], query: &str) -> Run {
    let output = Command::new(env!("CARGO_BIN_EXE_fselect"))
        .arg("--nocolor")
        .args(flags)
        .arg(query)
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run fselect");
    Run {
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    }
}

fn ok(run: &Run) -> &str {
    assert_eq!(run.code, Some(0), "stdout: {}\nstderr: {}", run.stdout, run.stderr);
    &run.stdout
}

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_files(dir: &Path, files: &[(&str, &str)]) {
    for (name, content) in files {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }
}

fn nested_dir(name: &str) -> PathBuf {
    let dir = fresh_dir(name);
    write_files(&dir, &[("top.txt", "x"), ("l1/mid.txt", "x"), ("l1/l2/low.txt", "x")]);
    dir
}

#[test]
fn depth_options_bound_the_traversal() {
    let dir = nested_dir("fselect_it_root_depth");
    let root = dir.display();

    let depth_one = fselect(&format!("select name from {root} depth 1 order by name"));
    let min_two = fselect(&format!("select name from {root} mindepth 2 order by name"));
    let max_two = fselect(&format!("select name from {root} maxdepth 2 where is_file order by name"));
    let unbounded = fselect(&format!("select name from {root} where is_file order by name"));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(ok(&depth_one), "l1\ntop.txt\n");
    assert_eq!(ok(&min_two), "l2\nlow.txt\nmid.txt\n");
    assert_eq!(ok(&max_two), "mid.txt\ntop.txt\n");
    assert_eq!(ok(&unbounded), "low.txt\nmid.txt\ntop.txt\n");
}

#[test]
fn gitignore_option_honours_ignore_rules_inside_a_repository() {
    let dir = nested_dir("fselect_it_root_gitignore");
    write_files(&dir, &[("skip.log", "x"), ("l1/also.log", "x"), (".gitignore", "*.log\n")]);
    let initialised = Command::new("git").args(["init", "-q"]).current_dir(&dir).status().map(|s| s.success()).unwrap_or(false);
    if !initialised {
        let _ = fs::remove_dir_all(&dir);
        return;
    }
    let root = dir.display();

    let ignored = fselect(&format!("select name from {root} gitignore where is_file order by name"));
    let plain = fselect(&format!("select name from {root} nogitignore where is_file and ext = log order by name"));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(ok(&ignored), ".gitignore\nlow.txt\nmid.txt\ntop.txt\n");
    assert_eq!(ok(&plain), "also.log\nskip.log\n");
}

#[test]
fn symlinks_option_follows_links() {
    let dir = fresh_dir("fselect_it_root_symlinks");
    write_files(&dir, &[("real/inner.txt", "x")]);
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(dir.join("real"), dir.join("linkdir")).is_ok()
        && std::os::unix::fs::symlink(dir.join("real").join("inner.txt"), dir.join("linkfile")).is_ok();
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_dir(dir.join("real"), dir.join("linkdir")).is_ok()
        && std::os::windows::fs::symlink_file(dir.join("real").join("inner.txt"), dir.join("linkfile")).is_ok();
    if !linked {
        let _ = fs::remove_dir_all(&dir);
        return;
    }
    let root = dir.display();

    let unfollowed = fselect(&format!("select name from {root} where is_file order by name"));
    let followed = fselect(&format!("select name from {root} symlinks where is_file order by name"));
    let links = fselect(&format!("select name from {root} depth 1 where is_symlink order by name"));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(ok(&unfollowed), "inner.txt\n", "links are listed but not followed by default");
    assert_eq!(ok(&followed), "inner.txt\ninner.txt\nlinkfile\n");
    assert_eq!(ok(&links), "linkdir\nlinkfile\n");
}

#[test]
fn each_root_keeps_its_own_options_and_paging_spans_them() {
    let dir = fresh_dir("fselect_it_root_multiple");
    write_files(&dir, &[("one/a.txt", "x"), ("one/deep/b.txt", "x"), ("two/c.txt", "x"), ("two/deep/d.txt", "x")]);
    let one = dir.join("one").display().to_string();
    let two = dir.join("two").display().to_string();

    let mixed_depths = fselect(&format!("select name from {one} depth 1, {two} where is_file order by name"));
    let paged = fselect(&format!("select name from {one}, {two} where is_file order by name limit 2 offset 1"));
    let counted = fselect(&format!("select count(*) from {one}, {two} where is_file"));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(ok(&mixed_depths), "a.txt\nc.txt\nd.txt\n");
    assert_eq!(ok(&paged), "b.txt\nc.txt\n");
    assert_eq!(ok(&counted), "4\n");
}

#[test]
fn traversal_order_follows_the_search_mode() {
    let dir = nested_dir("fselect_it_root_traversal");
    let root = dir.display();

    let default = fselect(&format!("select name, depth from {root}"));
    let bfs = fselect(&format!("select name, depth from {root} bfs"));
    let dfs = fselect(&format!("select name from {root} dfs"));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(ok(&default), ok(&bfs), "breadth-first is the default");
    let depths: Vec<u32> = ok(&bfs).lines().map(|line| line.split('\t').nth(1).unwrap().parse().unwrap()).collect();
    assert_eq!(depths, vec![1, 1, 2, 2, 3], "{}", bfs.stdout);

    let names: Vec<&str> = ok(&dfs).lines().collect();
    assert_eq!(names.len(), 5, "{}", dfs.stdout);
    let l1 = names.iter().position(|name| *name == "l1").unwrap();
    let mut subtree = names[l1 + 1..l1 + 4].to_vec();
    subtree.sort_unstable();
    assert_eq!(subtree, ["l2", "low.txt", "mid.txt"], "descendants follow their directory: {}", dfs.stdout);
    let l2 = names.iter().position(|name| *name == "l2").unwrap();
    assert_eq!(names[l2 + 1], "low.txt", "{}", dfs.stdout);
}

#[test]
fn hgignore_and_dockerignore_options_honour_their_ignore_files() {
    let dir = fresh_dir("fselect_it_root_hg_docker");
    write_files(
        &dir,
        &[
            ("hg/keep.txt", "x"),
            ("hg/skip.log", "x"),
            ("hg/sub/nested.log", "x"),
            ("hg/.hgignore", "syntax: glob\n*.log\n"),
            ("plain/keep.txt", "x"),
            ("plain/skip.log", "x"),
            ("plain/.hgignore", "syntax: glob\n*.log\n"),
            ("dock/keep.txt", "x"),
            ("dock/skip.log", "x"),
            ("dock/sub/nested.log", "x"),
            ("dock/.dockerignore", "*.log\n"),
        ],
    );
    fs::create_dir_all(dir.join("hg").join(".hg")).unwrap();
    let hg = dir.join("hg").display().to_string();
    let plain = dir.join("plain").display().to_string();
    let dock = dir.join("dock").display().to_string();

    let hg_ignored = fselect(&format!("select name from {hg} hgignore where is_file order by name"));
    let hg_alias = fselect(&format!("select name from {hg} hg where is_file order by name"));
    let hg_disabled = fselect(&format!("select name from {hg} nohgignore where is_file order by name"));
    let no_repository = fselect(&format!("select name from {plain} hgignore where is_file order by name"));
    let docker_ignored = fselect(&format!("select name from {dock} dockerignore where is_file order by name"));
    let docker_alias = fselect(&format!("select name from {dock} dock where is_file order by name"));
    let docker_disabled = fselect(&format!("select name from {dock} nodockerignore where is_file order by name"));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(ok(&hg_ignored), ".hgignore\nkeep.txt\n");
    assert_eq!(ok(&hg_alias), ".hgignore\nkeep.txt\n");
    assert_eq!(ok(&hg_disabled), ".hgignore\nkeep.txt\nnested.log\nskip.log\n");
    assert_eq!(ok(&no_repository), ".hgignore\nkeep.txt\nskip.log\n", "an .hgignore outside a Mercurial repository is not applied");
    assert_eq!(ok(&docker_ignored), ".dockerignore\nkeep.txt\nnested.log\n", "a bare glob matches the top level only, as in Docker");
    assert_eq!(ok(&docker_alias), ".dockerignore\nkeep.txt\nnested.log\n");
    assert_eq!(ok(&docker_disabled), ".dockerignore\nkeep.txt\nnested.log\nskip.log\n");
}

#[test]
fn regexp_roots_expand_to_the_directories_they_match() {
    let dir = fresh_dir("fselect_it_root_regexp");
    write_files(&dir, &[("alpha/one.txt", "x"), ("alpha/sub1/s1.txt", "x"), ("beta/two.txt", "x"), ("beta/sub2/s2.txt", "x"), ("other/three.txt", "x")]);
    let rx = dir.display().to_string().replace('\\', "/");

    let alternatives = fselect(&format!("select name from {rx}/alpha|beta regexp where is_file order by name"));
    let quoted = fselect(&format!("select name from '{rx}/(alpha|beta)' rx where is_file order by name"));
    let class = fselect(&format!("select name from {rx}/[ab]lpha regexp where is_file order by name"));
    let nested = fselect(&format!("select name from {rx}/.*pha/sub.* regexp where is_file order by name"));
    let unmatched = fselect(&format!("select name from {rx}/zzz.* regexp where is_file"));
    let files_are_not_roots = fselect(&format!("select name from {rx}/alpha/.*\\.txt regexp where is_file"));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(ok(&alternatives), "one.txt\ns1.txt\ns2.txt\ntwo.txt\n");
    assert_eq!(ok(&quoted), "one.txt\ns1.txt\ns2.txt\ntwo.txt\n");
    assert_eq!(ok(&class), "one.txt\ns1.txt\n");
    assert_eq!(ok(&nested), "s1.txt\n");
    assert_eq!(unmatched.code, Some(1), "{}", unmatched.stderr);
    assert_eq!(unmatched.stdout, "");
    assert!(unmatched.stderr.contains("no directory matches 'zzz.*'"), "{}", unmatched.stderr);
    assert_eq!(files_are_not_roots.code, Some(1), "only directories can match a root pattern: {}", files_are_not_roots.stderr);
    assert_eq!(files_are_not_roots.stdout, "");
}

#[test]
fn quoted_roots_may_contain_spaces() {
    let dir = fresh_dir("fselect_it_root_quoted");
    write_files(&dir, &[("dir with space/sp.txt", "x")]);
    let spaced = dir.join("dir with space").display().to_string();

    let single = fselect(&format!("select name from '{spaced}'"));
    let double = fselect(&format!("select name from \"{spaced}\""));
    let with_options = fselect(&format!("select name from '{spaced}' depth 1 where is_file"));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(ok(&single), "sp.txt\n");
    assert_eq!(ok(&double), "sp.txt\n");
    assert_eq!(ok(&with_options), "sp.txt\n");
}

fn fselect_in(dir: &Path, query: &str) -> Run {
    let output = Command::new(env!("CARGO_BIN_EXE_fselect"))
        .arg("--nocolor")
        .arg(query)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run fselect");
    Run {
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    }
}

#[test]
fn the_current_directory_is_the_default_root() {
    let dir = fresh_dir("fselect_it_root_default");
    write_files(&dir, &[("a.txt", "x"), ("sub/b.md", "yy")]);
    let sep = std::path::MAIN_SEPARATOR;

    let implicit = fselect_in(&dir, "select name, path where is_file order by name");
    let dot = fselect_in(&dir, "select name, path from . where is_file order by name");
    let relative = fselect_in(&dir, "select path, abspath from sub");
    let dotted = fselect_in(&dir, "select path from ./sub");
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(ok(&implicit), format!("a.txt\ta.txt\nb.md\tsub{sep}b.md\n"));
    assert_eq!(ok(&dot), format!("a.txt\ta.txt\nb.md\tsub{sep}b.md\n"));
    assert_eq!(ok(&relative), format!("b.md\t{}\n", dir.join("sub").join("b.md").display()));
    assert_eq!(ok(&dotted), "b.md\n");
}

#[test]
fn paths_are_relative_to_their_root() {
    let dir = fresh_dir("fselect_it_root_relative_paths");
    write_files(&dir, &[("sub/b.md", "x")]);
    let root = dir.display();
    let sep = std::path::MAIN_SEPARATOR;
    let absolute = dir.join("sub").join("b.md").display().to_string();

    let plain = fselect(&format!("select path, abspath from {root} where name = b.md"));
    let trailing = fselect(&format!("select path, abspath from {root}{sep} where name = b.md"));
    let deeper = fselect(&format!("select path, abspath from {root}{sep}sub where name = b.md"));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(ok(&plain), format!("sub{sep}b.md\t{absolute}\n"));
    assert_eq!(ok(&trailing), format!("sub{sep}b.md\t{absolute}\n"), "a trailing separator changes nothing");
    assert_eq!(ok(&deeper), format!("b.md\t{absolute}\n"));
}
