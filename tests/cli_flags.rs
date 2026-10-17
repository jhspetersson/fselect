use std::process::Command;

fn fselect(args: &[&str]) -> (bool, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_fselect"))
        .args(args)
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run fselect");
    (output.status.success(), String::from_utf8_lossy(&output.stdout).to_string())
}

#[test]
fn where_clause_sharing_an_argument_with_the_root_is_not_part_of_the_path() {
    let tmp = std::env::temp_dir().join("fselect_cli_argv_split");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("dir with space")).unwrap();
    std::fs::write(tmp.join("a.md"), "").unwrap();
    std::fs::write(tmp.join("b.txt"), "").unwrap();
    std::fs::write(tmp.join("dir with space").join("c.txt"), "").unwrap();
    let root = tmp.to_string_lossy().to_string();
    let spaced = tmp.join("dir with space").to_string_lossy().to_string();

    let (ok, out) = fselect(&["--nocolor", &format!("select name from {root} where ext = md"), "limit 1"]);
    assert!(ok, "{}", out);
    assert_eq!(out.trim(), "a.md");

    let (ok, out) = fselect(&["--nocolor", "select", "name", "from", &spaced, "limit", "1"]);
    assert!(ok, "{}", out);
    assert_eq!(out.trim(), "c.txt");

    let (ok, out) = fselect(&["--nocolor", "select name from", &spaced, "where ext = txt"]);
    assert!(ok, "{}", out);
    assert_eq!(out.trim(), "c.txt");

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn help_and_version_work_as_first_argument() {
    let (ok, out) = fselect(&["--help"]);
    assert!(ok && out.contains("Usage: fselect"), "{}", out);

    let (ok, out) = fselect(&["--version"]);
    assert!(ok && out.starts_with(&format!("fselect {}", env!("CARGO_PKG_VERSION"))), "{}", out);
}

#[test]
fn help_and_version_work_after_other_flags() {
    let (ok, out) = fselect(&["--nocolor", "--help"]);
    assert!(ok && out.contains("Usage: fselect"), "--nocolor --help: {}", out);

    let (ok, out) = fselect(&["--no-errors", "--version"]);
    assert!(ok && out.starts_with(&format!("fselect {}", env!("CARGO_PKG_VERSION"))), "--no-errors --version: {}", out);

    let (ok, out) = fselect(&["--nocolor", "--us-dates", "--fields"]);
    assert!(ok && out.contains("size"), "--nocolor --us-dates --fields: {}", out);

    let (ok, out) = fselect(&["--nocolor", "help"]);
    assert!(ok && out.contains("Usage: fselect"), "--nocolor help: {}", out);
}
