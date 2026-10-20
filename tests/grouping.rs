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

#[test]
fn grouped_coalesce_alias_keeps_a_string_that_reads_as_boolean() {
    let dir = std::env::temp_dir().join("fselect_it_grouping_coalesce_alias_type");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("true"), "x").unwrap();
    fs::write(dir.join("a.txt"), "yy").unwrap();
    let root = dir.display();

    let arithmetic = fselect(&format!(
        "select coalesce(name, is_file) as n, t.n + 0 from {root} as t group by coalesce(name, is_file) order by n"
    ));
    let having = fselect(&format!(
        "select coalesce(name, is_file) as n, count(*) from {root} as t group by coalesce(name, is_file) having t.n + 0 = 0 order by n"
    ));
    let boolean = fselect(&format!(
        "select is_file as f, t.f + 0 from {root} as t group by is_file"
    ));
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(arithmetic.0, Some(0), "{}", arithmetic.2);
    assert_eq!(arithmetic.1, "a.txt\t0\ntrue\t0\n");
    assert_eq!(having.0, Some(0), "{}", having.2);
    assert_eq!(having.1, "a.txt\t1\ntrue\t1\n");
    assert_eq!(boolean.0, Some(0), "{}", boolean.2);
    assert_eq!(boolean.1, "true\t1\n");
}

#[test]
fn sorted_scalar_subquery_takes_the_type_of_the_row_it_returns() {
    let dir = std::env::temp_dir().join("fselect_it_subquery_row_type");
    let _ = fs::remove_dir_all(&dir);
    for sub in ["strings", "flags", "other"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    fs::write(dir.join("strings").join("true"), "xx").unwrap();
    fs::write(dir.join("flags").join("item.txt"), "xxx").unwrap();
    fs::write(dir.join("other").join("o"), "x").unwrap();
    let strings = dir.join("strings");
    let flags = dir.join("flags");
    let other = dir.join("other");
    let (strings, flags, other) = (strings.display(), flags.display(), other.display());

    let query = |order: &str| {
        fselect(&format!(
            "select (select coalesce(r.name, is_file) from {strings} as r, {flags} order by size {order}) + 0 from {other}"
        ))
    };
    let boolean_first = query("desc");
    let string_first = query("asc");
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(boolean_first.0, Some(0), "{}", boolean_first.2);
    assert_eq!(boolean_first.1, "1\n");
    assert_eq!(string_first.0, Some(0), "{}", string_first.2);
    assert_eq!(string_first.1, "0\n");
}

#[test]
fn conditions_group_and_filter_in_having() {
    let dir = std::env::temp_dir().join("fselect_it_grouping_conditions");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("sub")).unwrap();
    fs::write(dir.join("f1"), "xx").unwrap();
    fs::write(dir.join("f2"), "xxx").unwrap();
    let root = dir.display();

    let negated = fselect(&format!(
        "select not is_file as d, count(*) from {root} group by not is_file order by d"
    ));
    let compared = fselect(&format!(
        "select is_file and size > 2 as k, count(*) from {root} group by is_file and size > 2 order by k"
    ));
    let having = fselect(&format!(
        "select size > 2 as big, count(*) from {root} group by size > 2 having size > 2"
    ));
    let having_not = fselect(&format!(
        "select size > 2 as big, count(*) from {root} group by size > 2 having not big"
    ));
    let _ = fs::remove_dir_all(&dir);

    for result in [&negated, &compared, &having, &having_not] {
        assert_eq!(result.0, Some(0), "{}", result.2);
    }
    assert_eq!(negated.1, "false\t2\ntrue\t1\n");
    assert_eq!(compared.1, "false\t2\ntrue\t1\n");
    assert_eq!(having.1, "true\t1\n");
    assert_eq!(having_not.1, "false\t2\n");
}

#[test]
fn unary_minus_turns_a_condition_into_zero_or_minus_one() {
    let dir = std::env::temp_dir().join("fselect_it_negated_condition");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("f1"), "xx").unwrap();
    fs::write(dir.join("f2"), "xxx").unwrap();
    let root = dir.display();

    let column = fselect(&format!("select name, -(size > 2) from {root} order by name"));
    let filter = fselect(&format!("select name from {root} where -(size > 2) < 0"));
    let having = fselect(&format!(
        "select size > 2 as big, count(*) from {root} group by size > 2 having -(size > 2) < 0"
    ));
    let _ = fs::remove_dir_all(&dir);

    for result in [&column, &filter, &having] {
        assert_eq!(result.0, Some(0), "{}", result.2);
    }
    assert_eq!(column.1, "f1\t0\nf2\t-1\n");
    assert_eq!(filter.1, "f2\n");
    assert_eq!(having.1, "true\t1\n");
}
