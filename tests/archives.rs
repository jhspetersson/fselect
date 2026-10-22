use std::fs;
use std::io::Write;
use std::path::{PathBuf, MAIN_SEPARATOR_STR as SEP};
use std::process::Command;
use std::time::{Duration, Instant};

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

fn rows(run: &Run) -> Vec<&str> {
    run.stdout.lines().collect()
}

fn make_archive_dir(name: &str, text_members: usize) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    let mut zip = zip::ZipWriter::new(fs::File::create(dir.join("pack.zip")).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    for i in 0..text_members {
        zip.start_file(format!("m{i:05}.txt"), options).unwrap();
        zip.write_all(format!("hello {i}\nworld\n").as_bytes()).unwrap();
    }
    zip.start_file("img.png", options).unwrap();
    zip.write_all(b"\x89PNG\r\n\x1a\n").unwrap();
    zip.write_all(&[0u8; 64]).unwrap();
    zip.add_directory("sub", options).unwrap();
    zip.start_file("sub/deep.txt", options).unwrap();
    zip.write_all(b"deep\n").unwrap();
    zip.finish().unwrap();
    dir
}

#[test]
fn subqueries_resolve_archive_members_whose_names_start_with_a_dot_segment() {
    let dir = std::env::temp_dir().join("fselect_it_archive_dot_segment");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let options = zip::write::SimpleFileOptions::default();
    let mut zip = zip::ZipWriter::new(fs::File::create(dir.join("collision.zip")).unwrap());
    zip.start_file("same.txt", options).unwrap();
    zip.write_all(b"nine byte").unwrap();
    zip.start_file("./same.txt", options).unwrap();
    zip.write_all(b"abc").unwrap();
    zip.finish().unwrap();
    let mut zip = zip::ZipWriter::new(fs::File::create(dir.join("lonely.zip")).unwrap());
    zip.start_file("./only.txt", options).unwrap();
    zip.write_all(b"abcd").unwrap();
    zip.finish().unwrap();
    let root = dir.display();

    let sum = fselect(&format!(
        "select sum(size) from (select path from {root} archives where ext = txt and path like '%collision%')"
    ));
    let lonely = fselect(&format!(
        "select size from (select path from {root} archives where ext = txt and path like '%lonely%')"
    ));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(sum.code, Some(0), "{}", sum.stderr);
    assert_eq!(rows(&sum), vec!["12"], "{}", sum.stderr);
    assert_eq!(lonely.code, Some(0), "{}", lonely.stderr);
    assert_eq!(rows(&lonely), vec!["4"], "{}", lonely.stderr);
}

#[test]
fn subquery_values_keep_their_own_trailing_line_breaks() {
    let dir = std::env::temp_dir().join("fselect_it_subquery_trailing_newlines");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let options = zip::write::SimpleFileOptions::default();
    let mut zip = zip::ZipWriter::new(fs::File::create(dir.join("names.zip")).unwrap());
    for name in ["hello\n", "hello\r\n", "world"] {
        zip.start_file(name, options).unwrap();
        zip.write_all(b"x").unwrap();
    }
    zip.finish().unwrap();
    let root = dir.display();

    let via_in = fselect(&format!(
        "select length(name) from {root} archives where name in (select name from {root} archives) order by length(name)"
    ));
    let scalar = fselect(&format!(
        "select length((select name from {root} archives where name like 'hello%' order by name desc limit 1)) from {root} where name = 'names.zip'"
    ));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(via_in.code, Some(0), "{}", via_in.stderr);
    assert_eq!(rows(&via_in), vec!["5", "6", "7", "9"], "{}", via_in.stderr);
    assert_eq!(scalar.code, Some(0), "{}", scalar.stderr);
    assert_eq!(rows(&scalar), vec!["7"], "{}", scalar.stderr);
}

#[test]
fn from_subqueries_keep_archive_members_whose_names_contain_tabs() {
    let dir = std::env::temp_dir().join("fselect_it_archive_tab_names");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let options = zip::write::SimpleFileOptions::default();
    let mut zip = zip::ZipWriter::new(fs::File::create(dir.join("tabzip.zip")).unwrap());
    zip.start_file("before	after.txt", options).unwrap();
    zip.write_all(b"abc").unwrap();
    zip.start_file("plain.txt", options).unwrap();
    zip.write_all(b"abcde").unwrap();
    zip.finish().unwrap();
    let root = dir.display();

    let direct = fselect(&format!("select sum(size) from {root} archives where ext = txt"));
    let wrapped = fselect(&format!("select sum(size) from (select path from {root} archives where ext = txt)"));
    let names = fselect(&format!("select size from (select path from {root} archives where ext = txt) order by size"));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(direct.code, Some(0), "{}", direct.stderr);
    assert_eq!(rows(&direct), vec!["8"]);
    assert_eq!(wrapped.code, Some(0), "{}", wrapped.stderr);
    assert_eq!(rows(&wrapped), vec!["8"], "{}", wrapped.stderr);
    assert_eq!(rows(&names), vec!["3", "5"], "{}", names.stderr);
}

#[test]
fn contains_respects_text_encodings_inside_archives() {
    let dir = std::env::temp_dir().join("fselect_it_archive_contains_encodings");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let options = zip::write::SimpleFileOptions::default();
    let mut zip = zip::ZipWriter::new(fs::File::create(dir.join("texts.zip")).unwrap());
    let mut utf16le = vec![0xFF, 0xFE];
    for unit in "hello world".encode_utf16() {
        utf16le.extend_from_slice(&unit.to_le_bytes());
    }
    zip.start_file("hello16.txt", options).unwrap();
    zip.write_all(&utf16le).unwrap();
    zip.start_file("kanji16.txt", options).unwrap();
    zip.write_all(&[0xFF, 0xFE, 0x41, 0x42]).unwrap();
    zip.start_file("overlap16.txt", options).unwrap();
    zip.write_all(&[0xFF, 0xFE, 0x0A, 0x4E, 0x4E, 0x4E]).unwrap();
    zip.start_file("latin1.txt", options).unwrap();
    zip.write_all(b"un caf\xe9 noir").unwrap();
    zip.start_file("latin1_end.txt", options).unwrap();
    zip.write_all(b"caf\xe9").unwrap();
    zip.start_file("late_nul.txt", options).unwrap();
    let mut late_nul = b"caf\xe9 ".to_vec();
    late_nul.extend(std::iter::repeat_n(b'x', 16_000));
    late_nul.push(0);
    zip.write_all(&late_nul).unwrap();
    zip.finish().unwrap();
    let root = dir.display();
    let cafe = fselect(&format!("select name from {root} archives where ext = txt and contains('caf\u{e9}') order by name"));
    assert_eq!(rows(&cafe), vec!["latin1.txt", "latin1_end.txt"], "{}", cafe.stderr);

    let hello = fselect(&format!("select name from {root} archives where ext = txt and contains('hello') order by name"));
    let ab = fselect(&format!("select name from {root} archives where ext = txt and contains('AB') order by name"));
    let overlap = fselect(&format!("select name from {root} archives where ext = txt and contains('\u{4E4E}') order by name"));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(rows(&hello), vec!["hello16.txt"], "{}", hello.stderr);
    assert_eq!(rows(&ab), Vec::<&str>::new(), "{}", ab.stderr);
    assert_eq!(rows(&overlap), vec!["overlap16.txt"], "{}", overlap.stderr);
}

#[test]
fn from_subqueries_resolve_archive_members_by_index_not_by_normalized_name() {
    let dir = std::env::temp_dir().join("fselect_it_archive_member_identity");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let options = zip::write::SimpleFileOptions::default();
    let mut zip = zip::ZipWriter::new(fs::File::create(dir.join("collision.zip")).unwrap());
    zip.start_file("dir/item.txt", options).unwrap();
    zip.write_all(b"abc").unwrap();
    zip.start_file("dir\\item.txt", options).unwrap();
    zip.write_all(b"abcdef").unwrap();
    zip.finish().unwrap();
    let root = dir.display();

    let six = fselect(&format!("select name, size from (select path from {root} archives where ext = txt and size = 6)"));
    let sum = fselect(&format!("select sum(size) from (select path from {root} archives where ext = txt)"));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(six.code, Some(0), "{}", six.stderr);
    assert_eq!(rows(&six), vec!["item.txt\t6"], "{}", six.stderr);
    assert_eq!(rows(&sum), vec!["9"], "{}", sum.stderr);
}

#[test]
fn archive_members_report_mime_and_content_fields() {
    let dir = make_archive_dir("fselect_it_archive_fields", 2);
    let root = dir.display();

    let by_type = fselect(&format!(
        "select path, mime, is_text, is_binary, contains('hello') from {root} arc where ext in ('txt', 'png') order by path"
    ));
    let text_only = fselect(&format!(
        "select name, size, line_count, has_bom from {root} arc where ext = txt order by name"
    ));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(by_type.code, Some(0), "{}", by_type.stderr);
    assert_eq!(rows(&by_type), vec![
        format!("pack.zip{SEP}img.png\timage/png\tfalse\ttrue\tfalse"),
        format!("pack.zip{SEP}m00000.txt\ttext/plain\ttrue\tfalse\ttrue"),
        format!("pack.zip{SEP}m00001.txt\ttext/plain\ttrue\tfalse\ttrue"),
        format!("pack.zip{SEP}sub{SEP}deep.txt\ttext/plain\ttrue\tfalse\tfalse"),
    ]);

    assert_eq!(text_only.code, Some(0), "{}", text_only.stderr);
    assert_eq!(rows(&text_only), vec![
        "deep.txt\t5\t1\tfalse",
        "m00000.txt\t14\t2\tfalse",
        "m00001.txt\t14\t2\tfalse",
    ]);
}

#[test]
fn large_archives_are_scanned_in_linear_time() {
    let members = 3000;
    let dir = make_archive_dir("fselect_it_archive_linear", members);
    let root = dir.display();

    let started = Instant::now();
    let run = fselect(&format!(
        "select name, line_count, mime, has_bom, contains('hello') from {root} arc where ext = txt"
    ));
    let elapsed = started.elapsed();
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(rows(&run).len(), members + 1, "every text member plus sub/deep.txt");
    assert!(
        elapsed < Duration::from_secs(30),
        "reading content fields of {members} members took {elapsed:?}; re-opening the archive per member would take minutes"
    );
}

fn set_unix_modes(archive: &std::path::Path, modes: &[(&str, u32)]) {
    let mut bytes = fs::read(archive).unwrap();
    let mut i = 0;
    while i + 46 <= bytes.len() {
        if &bytes[i..i + 4] != b"PK\x01\x02" {
            i += 1;
            continue;
        }
        let name_len = u16::from_le_bytes([bytes[i + 28], bytes[i + 29]]) as usize;
        let extra_len = u16::from_le_bytes([bytes[i + 30], bytes[i + 31]]) as usize;
        let comment_len = u16::from_le_bytes([bytes[i + 32], bytes[i + 33]]) as usize;
        let name = String::from_utf8_lossy(&bytes[i + 46..i + 46 + name_len]).to_string();
        if let Some((_, mode)) = modes.iter().find(|(entry, _)| *entry == name) {
            bytes[i + 5] = 3;
            bytes[i + 38..i + 42].copy_from_slice(&(mode << 16).to_le_bytes());
        }
        i += 46 + name_len + extra_len + comment_len;
    }
    fs::write(archive, bytes).unwrap();
}

#[test]
fn archive_member_types_follow_their_unix_modes() {
    let dir = std::env::temp_dir().join("fselect_it_archive_member_types");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let archive = dir.join("pack.zip");
    let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, content) in [("link", "target.txt"), ("fifo", ""), ("plain.txt", "hi"), ("full", ""), ("full/child.txt", "x"), ("empty", "")] {
        zip.start_file(name, options).unwrap();
        zip.write_all(content.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
    set_unix_modes(&archive, &[
        ("link", 0o120777),
        ("fifo", 0o10644),
        ("plain.txt", 0o100644),
        ("full", 0o40755),
        ("full/child.txt", 0o100644),
        ("empty", 0o40755),
    ]);
    let root = dir.display();

    let kinds = fselect(&format!(
        "select name, is_file, is_symlink, is_pipe, is_dir from {root} archives where name != 'pack.zip' order by name"
    ));
    let symlinks = fselect(&format!("select name from {root} archives where is_symlink"));
    let empty_dirs = fselect(&format!("select name from {root} archives where is_dir and is_empty"));
    let files = fselect(&format!("select name from {root} archives where is_file and name != 'pack.zip' order by name"));
    let _ = fs::remove_dir_all(&dir);

    for run in [&kinds, &symlinks, &empty_dirs, &files] {
        assert_eq!(run.code, Some(0), "{}", run.stderr);
    }
    assert_eq!(
        rows(&kinds),
        vec![
            "child.txt\ttrue\tfalse\tfalse\tfalse",
            "empty\tfalse\tfalse\tfalse\ttrue",
            "fifo\tfalse\tfalse\ttrue\tfalse",
            "full\tfalse\tfalse\tfalse\ttrue",
            "link\tfalse\ttrue\tfalse\tfalse",
            "plain.txt\ttrue\tfalse\tfalse\tfalse",
        ]
    );
    assert_eq!(rows(&symlinks), vec!["link"]);
    assert_eq!(rows(&empty_dirs), vec!["empty"], "a populated directory known only by its mode is not empty");
    assert_eq!(rows(&files), vec!["child.txt", "plain.txt"]);
}

#[cfg(windows)]
#[test]
fn archive_members_do_not_inherit_the_containers_attributes() {
    let dir = make_archive_dir("fselect_it_archive_container_attrs", 2);
    let archive = dir.join("pack.zip");
    let mut perms = fs::metadata(&archive).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(&archive, perms).unwrap();
    let root = dir.display();

    let members = fselect(&format!("select name from {root} archives where ext = txt and has_extattr('R')"));
    let container = fselect(&format!("select name from {root} where has_extattr('R')"));
    let mut perms = fs::metadata(&archive).unwrap().permissions();
    perms.set_readonly(false);
    fs::set_permissions(&archive, perms).unwrap();
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(members.code, Some(0), "{}", members.stderr);
    assert_eq!(rows(&members), Vec::<&str>::new(), "members carry no attributes of their own");
    assert_eq!(rows(&container), vec!["pack.zip"]);
}
