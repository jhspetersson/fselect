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

fn run_raw(args: &[&str]) -> (Option<i32>, String, String) {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_fselect"))
        .args(args)
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run fselect");
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

#[test]
fn listing_flags_print_the_vocabulary() {
    for (flag, expected) in [("--fields", "name"), ("--functions", "UPPER"), ("--root-options", "depth"), ("--output-formats", "json")] {
        let (code, out, err) = run_raw(&[flag]);
        assert_eq!(code, Some(0), "{flag}: {err}");
        assert!(out.split_whitespace().any(|word| word == expected), "{flag} output lacks {expected}: {out}");
    }
}

#[test]
fn exit_codes_distinguish_no_rows_from_io_errors() {
    let tmp = std::env::temp_dir().join("fselect_cli_exit_codes");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::write(tmp.join("a.txt"), "x").unwrap();
    let root = tmp.display();
    let missing = tmp.join("missing").display().to_string();

    let no_rows = run_raw(&["--nocolor", &format!("select name from {root} where size > 100")]);
    let io_error = run_raw(&["--nocolor", &format!("select name from {missing}")]);
    let silenced = run_raw(&["--nocolor", "--no-error", &format!("select name from {missing}")]);
    let _ = std::fs::remove_dir_all(&tmp);

    assert_eq!(no_rows.0, Some(0), "{}", no_rows.2);
    assert_eq!(no_rows.1, "");
    assert_eq!(io_error.0, Some(1), "{}", io_error.2);
    assert!(error_lines(&io_error.2).iter().any(|line| line.contains("missing")), "{}", io_error.2);
    assert_eq!(silenced.0, Some(1), "the exit code still reports the error");
    assert_eq!(error_lines(&silenced.2), Vec::<&str>::new(), "--no-error silences the message");
}

fn error_lines(stderr: &str) -> Vec<&str> {
    stderr
        .lines()
        .filter(|line| !line.is_empty())
        .filter(|line| !line.starts_with(['[', ' ', ']', ')', '"']))
        .filter(|line| !line.starts_with("Search:") && !line.starts_with("Compute:"))
        .collect()
}

#[test]
fn us_date_flag_switches_the_date_dialect() {
    use chrono::TimeZone;

    let tmp = std::env::temp_dir().join("fselect_cli_us_dates");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let path = tmp.join("june.txt");
    std::fs::write(&path, "x").unwrap();
    let june_first = chrono::NaiveDate::from_ymd_opt(2024, 6, 1).unwrap().and_hms_opt(12, 0, 0).unwrap();
    let time = std::time::SystemTime::from(chrono::Local.from_local_datetime(&june_first).single().unwrap());
    std::fs::File::options().write(true).open(&path).unwrap().set_modified(time).unwrap();
    let query = format!("select name from {} where modified = '06/01/2024'", tmp.display());

    let uk = run_raw(&["--nocolor", &query]);
    let us = run_raw(&["--nocolor", "--us-date", &query]);
    let _ = std::fs::remove_dir_all(&tmp);

    assert_eq!(uk.0, Some(0), "{}", uk.2);
    assert_eq!(uk.1, "", "without the flag 06/01 is the 6th of January");
    assert_eq!(us.0, Some(0), "{}", us.2);
    assert_eq!(us.1, "june.txt\n");
}

#[test]
fn config_file_flag_overrides_the_defaults() {
    use chrono::TimeZone;

    let tmp = std::env::temp_dir().join("fselect_cli_config");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let path = tmp.join("small.txt");
    std::fs::write(&path, "hello world\nsecond line\n").unwrap();
    let june_first = chrono::NaiveDate::from_ymd_opt(2024, 6, 1).unwrap().and_hms_opt(12, 0, 0).unwrap();
    let time = std::time::SystemTime::from(chrono::Local.from_local_datetime(&june_first).single().unwrap());
    std::fs::File::options().write(true).open(&path).unwrap().set_modified(time).unwrap();
    std::fs::write(tmp.join("custom.toml"), "us_dates = true\ndefault_file_size_format = \"k\"\ncheck_for_updates = false\n").unwrap();
    std::fs::write(tmp.join("bare.toml"), "check_for_updates = false\n").unwrap();
    let custom = tmp.join("custom.toml").display().to_string();
    let bare = tmp.join("bare.toml").display().to_string();
    let missing = tmp.join("missing.toml").display().to_string();
    let root = tmp.display();
    let sizes = format!("select fsize from {root} where ext = txt");
    let dates = format!("select name from {root} where modified = '06/01/2024'");

    let custom_sizes = run_raw(&["--nocolor", "--config", &custom, &sizes]);
    let custom_dates = run_raw(&["--nocolor", "-c", &custom, &dates]);
    let bare_sizes = run_raw(&["--nocolor", "--config", &bare, &sizes]);
    let bare_dates = run_raw(&["--nocolor", "--config", &bare, &dates]);
    let unreadable = run_raw(&["--nocolor", "--config", &missing, &sizes]);
    let _ = std::fs::remove_dir_all(&tmp);

    assert_eq!(custom_sizes.0, Some(0), "{}", custom_sizes.2);
    assert_eq!(custom_sizes.1, "0KiB\n");
    assert_eq!(custom_dates.0, Some(0), "{}", custom_dates.2);
    assert_eq!(custom_dates.1, "small.txt\n");
    assert_eq!(bare_sizes.0, Some(0), "{}", bare_sizes.2);
    assert_eq!(bare_sizes.1, "24B\n");
    assert_eq!(bare_dates.0, Some(0), "{}", bare_dates.2);
    assert_eq!(bare_dates.1, "", "without the setting 06/01 is the 6th of January");
    assert_eq!(unreadable.0, Some(0), "{}", unreadable.2);
    assert_eq!(unreadable.1, "24B\n", "a missing config file falls back to the defaults");
    assert_eq!(error_lines(&unreadable.2), vec!["Could not open config file. Using default settings."]);
}

#[test]
fn piped_output_carries_no_colour_codes_without_the_flag() {
    let tmp = std::env::temp_dir().join("fselect_cli_pipe_colour");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::write(tmp.join("a.txt"), "x").unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_fselect"))
        .arg(format!("select name, size from {}", tmp.display()))
        .env_remove("NO_COLOR")
        .output()
        .expect("failed to run fselect");
    let _ = std::fs::remove_dir_all(&tmp);

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(!stdout.contains('\u{1b}'), "{stdout:?}");
    assert_eq!(stdout, "a.txt\t1\n");
}
