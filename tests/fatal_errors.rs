use std::fs;
use std::path::PathBuf;
use std::process::Command;

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn fselect(query: &str) -> Run {
    let output = Command::new(env!("CARGO_BIN_EXE_fselect"))
        .args(["--nocolor", query])
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run fselect");
    Run {
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    }
}

fn make_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("a.txt"), "x").unwrap();
    fs::write(dir.join("b.md"), "y").unwrap();
    dir
}

/// A query that is rejected before any file is visited must not leave a
/// half-written document on stdout: previously the JSON `[` and the HTML
/// `<html>...<table>` header were already out when the error was reported.
fn assert_rejected_without_output(query: &str, expected_error: &str) {
    let run = fselect(query);
    assert_eq!(run.code, Some(2), "{}\nstdout: {}\nstderr: {}", query, run.stdout, run.stderr);
    assert!(
        run.stderr.contains(expected_error),
        "{}\nexpected stderr to mention {:?}, got: {}", query, expected_error, run.stderr
    );
    assert_eq!(run.stdout, "", "{}\nstdout should stay empty on a fatal query error", query);
}

#[test]
fn undeclared_root_alias_is_rejected_before_the_header() {
    let dir = make_dir("fselect_test_fatal_alias");
    let dir = dir.to_string_lossy();

    assert_rejected_without_output(
        &format!("select name, x.name from {} into json", dir),
        "Invalid root alias: x",
    );
    assert_rejected_without_output(
        &format!("select name from {} where x.size > 0 into html", dir),
        "Invalid root alias: x",
    );
    // ... also when the undeclared alias hides inside a subquery scope
    assert_rejected_without_output(
        &format!("select name from {} as t1 where size in (select size from {} as t2 where t3.size > 0) into json", dir, dir),
        "Invalid root alias: t3",
    );
}

#[test]
fn grouped_query_errors_are_rejected_before_the_header() {
    let dir = make_dir("fselect_test_fatal_grouped");
    let dir = dir.to_string_lossy();

    assert_rejected_without_output(
        &format!("select count(*) from {} group by ext order by name into html", dir),
        "Cannot order by",
    );
    assert_rejected_without_output(
        &format!("select count(*) from {} group by ext having ext in ('txt') into json", dir),
        "not supported in the HAVING clause",
    );
}

#[test]
fn invalid_regex_root_is_rejected_before_the_header() {
    let dir = make_dir("fselect_test_fatal_regex_root");
    let dir = dir.to_string_lossy();

    assert_rejected_without_output(
        &format!("select name from {}/a[b regexp into json", dir),
        "invalid regex in root path",
    );
}

#[test]
fn wrong_function_argument_counts_are_rejected_before_the_header() {
    let dir = make_dir("fselect_test_fatal_arity");
    let dir = dir.to_string_lossy();

    assert_rejected_without_output(
        &format!("select lower(name, 1) from {} into json", dir),
        "Function LOWER expects 1 argument, got 2",
    );
    assert_rejected_without_output(
        &format!("select name from {} where replace(name, 'a') = 'b' into html", dir),
        "Function REPLACE expects 3 arguments, got 2",
    );
}

#[test]
fn malformed_in_lists_are_rejected_before_the_header() {
    let dir = make_dir("fselect_test_fatal_in_list");
    let dir = dir.to_string_lossy();

    assert_rejected_without_output(
        &format!("select name from {} where ext in () into json", dir),
        "IN list cannot be empty",
    );
    assert_rejected_without_output(
        &format!("select name from {} where ext in ('txt' 'md') into html", dir),
        "Expected ',' or closing parenthesis in IN list",
    );
}

#[test]
fn a_failed_first_row_leaves_no_separator_and_keeps_the_limit() {
    let dir = std::env::temp_dir().join("fselect_test_failed_first_row");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("a.txt"), "x").unwrap();
    fs::write(dir.join("b.txt"), "xxx").unwrap();
    fs::write(dir.join("c.txt"), "xxxxx").unwrap();
    let root = dir.display();

    let json = fselect(&format!("select name, 1 / (size - 1) from {root} into json"));
    let limited = fselect(&format!("select name, 1 / (size - 1) from {root} limit 2 into json"));
    let tabs = fselect(&format!("select name, 1 / (size - 1) from {root}"));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(json.code, Some(1), "{}", json.stderr);
    assert!(json.stderr.contains("Division by zero"), "{}", json.stderr);
    assert_eq!(
        json.stdout.trim(),
        r#"[{"name":"b.txt","1 / (size - 1)":"0.5"},{"name":"c.txt","1 / (size - 1)":"0.25"}]"#
    );
    assert_eq!(
        limited.stdout.trim(),
        r#"[{"name":"b.txt","1 / (size - 1)":"0.5"},{"name":"c.txt","1 / (size - 1)":"0.25"}]"#
    );
    assert_eq!(tabs.stdout, "b.txt\t0.5\nc.txt\t0.25\n");
}

#[test]
fn valid_queries_still_produce_complete_documents() {
    let dir = make_dir("fselect_test_fatal_valid");
    let dir = dir.to_string_lossy();

    let run = fselect(&format!("select name from {} order by name into json", dir));
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(run.stdout.trim(), r#"[{"name":"a.txt"},{"name":"b.md"}]"#);

    let run = fselect(&format!("select name from {} into html", dir));
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(run.stdout.starts_with("<html>") && run.stdout.trim_end().ends_with("</html>"), "{}", run.stdout);
}
