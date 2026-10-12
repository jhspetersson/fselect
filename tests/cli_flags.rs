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
