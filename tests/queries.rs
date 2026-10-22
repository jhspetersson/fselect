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

#[test]
fn hash_fields_digest_file_contents() {
    let dir = fresh_dir("fselect_it_query_hashes");
    write_files(&dir, &[("two_lines.txt", "hello world\nsecond line\n"), ("copy.txt", "hello world\nsecond line\n"), ("empty.txt", "")]);
    let root = dir.display();

    let digests = fselect(&format!("select sha1, sha256, sha512 from {root} where name = two_lines.txt"));
    let empty = fselect(&format!("select sha1 from {root} where name = empty.txt"));
    let duplicates = fselect(&format!("select sha256, count(*) from {root} where is_file group by sha256 having count(*) > 1"));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(
        ok(&digests),
        "ac481614b7dc8fa938c04b13510057fe3572177e\t1cf34cbeaca6f2ce821c4b6369c37c583b1cc10846122a8c77a4df77d0d5b7b8\t039445c839a89b3d7acbc2cc4e646845abad5ed16e54d21d2246785c9b0fa93360dd5f08ec4dfe5763a77aaf61b4ea4ce658c37da5453e339d6a5a653c62e15e\n"
    );
    assert_eq!(ok(&empty), "da39a3ee5e6b4b0d3255bfef95601890afd80709\n");
    assert_eq!(ok(&duplicates), "1cf34cbeaca6f2ce821c4b6369c37c583b1cc10846122a8c77a4df77d0d5b7b8\t2\n");
}

#[test]
fn text_analysis_fields_describe_file_contents() {
    let dir = fresh_dir("fselect_it_query_text_fields");
    write_files(&dir, &[("lf.txt", "hello world\nsecond line\n"), ("crlf.txt", "one\r\ntwo\r\n"), ("bom.txt", "\u{feff}bom text\n"), ("bare.txt", "abc")]);
    fs::write(dir.join("bin.dat"), [0u8, 1, 2, 3, b'b', b'i', b'n']).unwrap();
    let root = dir.display();

    let described = fselect(&format!(
        "select name, line_count, word_count, char_count, encoding, has_bom, line_ending, is_text, is_binary, mime from {root} where is_file order by name"
    ));
    let binary = fselect(&format!("select name from {root} where is_binary"));
    let crlf = fselect(&format!("select name from {root} where line_ending = CRLF"));
    let bom = fselect(&format!("select name from {root} where has_bom"));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(
        ok(&described),
        "bare.txt\t1\t1\t3\tASCII\tfalse\t\ttrue\tfalse\ttext/plain\n\
         bin.dat\t0\t\t\t\tfalse\t\tfalse\ttrue\tapplication/octet-stream\n\
         bom.txt\t1\t2\t9\tUTF-8\ttrue\tLF\ttrue\tfalse\ttext/plain\n\
         crlf.txt\t2\t2\t10\tASCII\tfalse\tCRLF\ttrue\tfalse\ttext/plain\n\
         lf.txt\t2\t4\t24\tASCII\tfalse\tLF\ttrue\tfalse\ttext/plain\n"
    );
    assert_eq!(ok(&binary), "bin.dat\n");
    assert_eq!(ok(&crlf), "crlf.txt\n");
    assert_eq!(ok(&bom), "bom.txt\n");
}

#[test]
fn hidden_files_follow_platform_conventions() {
    let dir = fresh_dir("fselect_it_query_hidden");
    write_files(&dir, &[(".dot", "x"), ("plain.txt", "x"), ("flagged.txt", "x")]);
    #[cfg(windows)]
    {
        let flagged = Command::new("attrib").arg("+h").arg(dir.join("flagged.txt")).status().map(|status| status.success()).unwrap_or(false);
        if !flagged {
            let _ = fs::remove_dir_all(&dir);
            return;
        }
    }
    let root = dir.display();

    let listed = fselect(&format!("select name, is_hidden from {root} order by name"));
    let hidden = fselect(&format!("select name from {root} where is_hidden order by name"));
    let visible = fselect(&format!("select name from {root} where not is_hidden order by name"));
    let _ = fs::remove_dir_all(&dir);

    #[cfg(unix)]
    {
        assert_eq!(ok(&listed), ".dot\ttrue\nflagged.txt\tfalse\nplain.txt\tfalse\n");
        assert_eq!(ok(&hidden), ".dot\n");
        assert_eq!(ok(&visible), "flagged.txt\nplain.txt\n");
    }
    #[cfg(windows)]
    {
        assert_eq!(ok(&listed), ".dot\tfalse\nflagged.txt\ttrue\nplain.txt\tfalse\n", "Windows hides by attribute, not by a leading dot");
        assert_eq!(ok(&hidden), "flagged.txt\n");
        assert_eq!(ok(&visible), ".dot\nplain.txt\n");
    }
}

#[test]
fn keywords_fields_and_functions_are_case_insensitive() {
    let dir = query_dir("fselect_it_query_case");
    let root = dir.display();

    let upper = fselect(&format!("SELECT NAME, SIZE FROM {root} WHERE IS_FILE AND EXT = txt ORDER BY SIZE DESC, PATH"));
    let mixed = fselect(&format!("Select Upper(Name) From {root} Where Name Like 'al%' And Size > 1"));
    let format = fselect(&format!("select name from {root} where name = beta.md INTO JSON"));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(ok(&upper), "alpha.txt\t11\nalpha.txt\t1\n");
    assert_eq!(ok(&mixed), "ALPHA.TXT\n");
    assert_eq!(ok(&format), "[{\"name\":\"beta.md\"}]");
}

#[test]
fn null_tests_detect_missing_metadata() {
    let dir = query_dir("fselect_it_query_nulls");
    let root = dir.display();

    let missing = fselect(&format!("select name from {root} where width is null and ext = txt order by path"));
    let present = fselect(&format!("select name from {root} where width is not null"));
    let never_null = fselect(&format!("select name from {root} where name is null"));
    let column = fselect(&format!("select name, width is null, name is null from {root} where name = beta.md"));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(ok(&missing), "alpha.txt\nalpha.txt\n");
    assert_eq!(ok(&present), "");
    assert_eq!(ok(&never_null), "");
    assert_eq!(ok(&column), "beta.md\ttrue\tfalse\n");
}
