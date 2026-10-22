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

fn sample_dir(name: &str) -> PathBuf {
    let dir = fresh_dir(name);
    write_files(&dir, &[("a, b.txt", "x"), ("c.md", "yy")]);
    dir
}

#[test]
fn tabs_is_the_default_format() {
    let dir = sample_dir("fselect_it_format_tabs");
    let run = fselect(&format!("select name, size from {} order by name", dir.display()));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(ok(&run), "a, b.txt\t1\nc.md\t2\n");
}

#[test]
fn json_is_an_array_of_objects_with_string_values() {
    let dir = sample_dir("fselect_it_format_json");
    let rows = fselect(&format!("select name, size from {} order by name into json", dir.display()));
    let empty = fselect(&format!("select name from {} where size > 100 into json", dir.display()));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(ok(&rows), "[{\"name\":\"a, b.txt\",\"size\":\"1\"},{\"name\":\"c.md\",\"size\":\"2\"}]");
    assert_eq!(ok(&empty), "[]");
}

#[test]
fn csv_quotes_only_the_fields_that_need_it() {
    let dir = sample_dir("fselect_it_format_csv");
    let run = fselect(&format!("select name, size from {} order by name into csv", dir.display()));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(ok(&run), "\"a, b.txt\",1\nc.md,2\n");
}

#[test]
fn html_is_a_single_table_document() {
    let dir = sample_dir("fselect_it_format_html");
    let run = fselect(&format!("select name, size from {} order by name into html", dir.display()));
    let _ = fs::remove_dir_all(&dir);

    let out = ok(&run);
    assert!(out.starts_with("<html><head><title>"), "{out}");
    assert!(out.ends_with("</table></body></html>"), "{out}");
    assert!(out.contains("<tr><td>a, b.txt</td><td>1</td></tr><tr><td>c.md</td><td>2</td></tr>"), "{out}");
    assert_eq!(out.matches("<table>").count(), 1, "{out}");
}

#[test]
fn lines_puts_every_value_on_its_own_line() {
    let dir = sample_dir("fselect_it_format_lines");
    let run = fselect(&format!("select name, size from {} order by name into lines", dir.display()));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(ok(&run), "a, b.txt\n1\nc.md\n2\n");
}

#[test]
fn list_separates_values_with_nul_for_xargs() {
    let dir = sample_dir("fselect_it_format_list");
    let run = fselect(&format!("select name from {} order by name into list", dir.display()));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(ok(&run), "a, b.txt\0c.md\0\n");
}
