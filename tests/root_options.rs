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
