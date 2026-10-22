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

fn query_dir(name: &str) -> PathBuf {
    let dir = fresh_dir(name);
    write_files(&dir, &[("alpha.txt", "hello world"), ("beta.md", "x"), ("gamma.rs", ""), ("other/alpha.txt", "q")]);
    dir
}

#[test]
fn pattern_and_list_operators_filter_rows() {
    let dir = query_dir("fselect_it_query_operators");
    let root = dir.display();

    let like = fselect(&format!("select name from {root} where name like 'al%' order by path"));
    let not_like = fselect(&format!("select name from {root} where is_file and name not like '%.txt' order by name"));
    let regex = fselect(&format!("select name from {root} where name =~ '^b'"));
    let regexp_keyword = fselect(&format!("select name from {root} where name regexp '^b'"));
    let rlike = fselect(&format!("select name from {root} where name rlike '^b'"));
    let not_rlike = fselect(&format!("select name from {root} where is_file and name not rlike '^b' order by path"));
    let rlike_column = fselect(&format!("select name rlike '^b', count(*) from {root} where is_file group by name rlike '^b' order by 1"));
    let between = fselect(&format!("select name from {root} where size between 1 and 5 order by name"));
    let in_list = fselect(&format!("select name from {root} where ext in (md, rs) order by name"));
    let not_in_list = fselect(&format!("select name from {root} where is_file and ext not in (md, rs) order by path"));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(ok(&like), "alpha.txt\nalpha.txt\n");
    assert_eq!(ok(&not_like), "beta.md\ngamma.rs\n");
    assert_eq!(ok(&regex), "beta.md\n");
    assert_eq!(ok(&regexp_keyword), "beta.md\n");
    assert_eq!(ok(&rlike), "beta.md\n");
    assert_eq!(ok(&not_rlike), "alpha.txt\ngamma.rs\nalpha.txt\n");
    assert_eq!(ok(&rlike_column), "false\t3\ntrue\t1\n");
    assert_eq!(ok(&between), "alpha.txt\nbeta.md\n");
    assert_eq!(ok(&in_list), "beta.md\ngamma.rs\n");
    assert_eq!(ok(&not_in_list), "alpha.txt\nalpha.txt\n");
}

#[test]
fn string_and_numeric_functions_render_their_results() {
    let dir = query_dir("fselect_it_query_functions");
    let root = dir.display();

    let strings = fselect(&format!(
        "select upper(name), lower(ext), length(name), concat(name, '!'), substr(name, 1, 3), replace(name, 'txt', 'text') from {root} where name = alpha.txt and size > 1"
    ));
    let numbers = fselect(&format!("select abs(-size), round(size / 3, 2), size * 2 + 1, fsize from {root} where name = alpha.txt and size > 1"));
    let dates = fselect(&format!("select year(modified) > 2000, name from {root} where name = beta.md"));
    let content = fselect(&format!("select name from {root} where contains('world')"));
    let empty = fselect(&format!("select name, is_empty from {root} where name = gamma.rs"));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(ok(&strings), "ALPHA.TXT\ttxt\t9\talpha.txt!\talp\talpha.text\n");
    assert_eq!(ok(&numbers), "11\t3.67\t23\t11B\n");
    assert_eq!(ok(&dates), "true\tbeta.md\n");
    assert_eq!(ok(&content), "alpha.txt\n");
    assert_eq!(ok(&empty), "gamma.rs\ttrue\n");
}

#[test]
fn ungrouped_aggregates_summarise_the_whole_result() {
    let dir = query_dir("fselect_it_query_aggregates");
    let root = dir.display();

    let summary = fselect(&format!("select count(*), sum(size), min(size), max(size), avg(size) from {root}"));
    let nothing = fselect(&format!("select count(*) from {root} where size > 1000"));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(ok(&summary), "5\t13\t0\t11\t2.6\n");
    assert_eq!(ok(&nothing), "0\n");
}

#[test]
fn subqueries_in_where_and_from_resolve_against_other_roots() {
    let dir = query_dir("fselect_it_query_subqueries");
    let root = dir.display();
    let other = dir.join("other").display().to_string();

    let in_subquery = fselect(&format!("select name from {root} where name in (select name from {other}) order by path"));
    let exists = fselect(&format!("select name from {root} as t where exists (select name from {other} as o where o.name = t.name) order by path"));
    let from_subquery = fselect(&format!("select name from (select path from {root} where ext = txt) order by path"));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(ok(&in_subquery), "alpha.txt\nalpha.txt\n");
    assert_eq!(ok(&exists), "alpha.txt\nalpha.txt\n");
    assert_eq!(ok(&from_subquery), "alpha.txt\nalpha.txt\n");
}

#[test]
fn ordering_limit_and_offset_page_through_results() {
    let dir = query_dir("fselect_it_query_paging");
    let root = dir.display();

    let ordered = fselect(&format!("select name, size from {root} where is_file order by size desc, name"));
    let limited = fselect(&format!("select name from {root} where is_file order by size desc, name limit 2"));
    let offset = fselect(&format!("select name from {root} where is_file order by size desc, name limit 2 offset 1"));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(ok(&ordered), "alpha.txt\t11\nalpha.txt\t1\nbeta.md\t1\ngamma.rs\t0\n");
    assert_eq!(ok(&limited), "alpha.txt\nalpha.txt\n");
    assert_eq!(ok(&offset), "alpha.txt\nbeta.md\n");
}
