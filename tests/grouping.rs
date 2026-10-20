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
fn computed_grouping_alias_keeps_its_numeric_type_in_correlated_subqueries() {
    let dir = std::env::temp_dir().join("fselect_it_grouping_alias_type");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for (name, size) in [("a.txt", 2), ("b.txt", 3), ("c.txt", 6)] {
        fs::write(dir.join(name), "x".repeat(size)).unwrap();
    }
    let root = dir.display();

    let grouped = fselect(&format!(
        "select size * 2 as doubled, (select count(*) from {root} where t.doubled > 10) from {root} as t group by size * 2 order by doubled"
    ));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(grouped.0, Some(0), "{}", grouped.2);
    assert_eq!(grouped.1, "4\t0\n6\t0\n12\t3\n");
}
