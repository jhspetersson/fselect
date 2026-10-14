use std::fs;
use std::process::Command;

fn fselect(query: &str) -> (Option<i32>, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_fselect"))
        .args(["--nocolor", query])
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
fn deferred_content_columns_render_in_every_format() {
    let dir = std::env::temp_dir().join("fselect_it_deferred_columns");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for (name, lines) in [("a.txt", 1), ("b.txt", 2), ("c.txt", 3)] {
        fs::write(dir.join(name), "line\n".repeat(lines)).unwrap();
    }
    let root = dir.display();

    let json = fselect(&format!("select name, line_count from {root} order by name desc limit 2 into json"));
    let csv = fselect(&format!("select name, line_count, word_count from {root} order by name limit 1 offset 1 into csv"));
    let tabs = fselect(&format!("select line_count, name from {root} order by size desc limit 2"));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(json.0, Some(0), "{}", json.2);
    assert_eq!(json.1.trim(), r#"[{"name":"c.txt","linecount":"3"},{"name":"b.txt","linecount":"2"}]"#);
    assert_eq!(csv.0, Some(0), "{}", csv.2);
    assert_eq!(csv.1, "b.txt,2,2\n");
    assert_eq!(tabs.0, Some(0), "{}", tabs.2);
    assert_eq!(tabs.1, "3\tc.txt\n2\tb.txt\n");
}
