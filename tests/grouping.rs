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

#[test]
fn grouped_comparisons_over_missing_metadata_are_false_both_ways() {
    let dir = std::env::temp_dir().join("fselect_it_grouping_missing_metadata");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for name in ["a.txt", "b.txt", "c.txt", "d.txt"] {
        fs::write(dir.join(name), "text").unwrap();
    }
    let root = dir.display();

    let shown = fselect(&format!("select width > 0, width <= 0, count(*) from {root} group by width > 0"));
    let having = fselect(&format!("select width > 0, width <= 0, count(*) from {root} group by width > 0 having width <= 0"));
    let negated = fselect(&format!("select width > 0, not (width > 0), count(*) from {root} group by width > 0"));
    let _ = fs::remove_dir_all(&dir);

    for result in [&shown, &having, &negated] {
        assert_eq!(result.0, Some(0), "{}", result.2);
    }
    assert_eq!(shown.1, "false\tfalse\t4\n");
    assert_eq!(having.1, "");
    assert_eq!(negated.1, "false\tfalse\t4\n", "neither side of an unknown comparison holds");
}

#[test]
fn negated_grouped_fields_are_computed_from_the_grouped_value() {
    let dir = std::env::temp_dir().join("fselect_it_grouping_negated_fields");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("a"), "x").unwrap();
    fs::write(dir.join("bb"), "xx").unwrap();
    fs::write(dir.join("cc"), "xx").unwrap();
    let root = dir.display();

    let field = fselect(&format!("select -size, count(*) from {root} group by size order by size"));
    let function = fselect(&format!("select -length(name), count(*) from {root} group by length(name) order by length(name)"));
    let having = fselect(&format!("select size, count(*) from {root} group by size having -size < -1"));
    let _ = fs::remove_dir_all(&dir);

    for result in [&field, &function, &having] {
        assert_eq!(result.0, Some(0), "{}", result.2);
    }
    assert_eq!(field.1, "-1\t1\n-2\t2\n");
    assert_eq!(function.1, "-1\t1\n-2\t2\n");
    assert_eq!(having.1, "2\t2\n");
}

#[test]
fn groups_separate_equal_texts_of_different_types_in_any_root_order() {
    let dir = std::env::temp_dir().join("fselect_it_grouping_typed_keys");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("texts")).unwrap();
    fs::create_dir_all(dir.join("images")).unwrap();
    fs::write(dir.join("texts").join("144"), "x").unwrap();
    fs::copy(
        concat!(env!("CARGO_MANIFEST_DIR"), "/resources/test/image/rust-logo-blk.png"),
        dir.join("images").join("logo.png"),
    )
    .unwrap();
    let texts = dir.join("texts").display().to_string();
    let images = dir.join("images").display().to_string();

    let forward = fselect(&format!("select coalesce(width, name) as v, count(*) from {texts}, {images} group by v order by v"));
    let backward = fselect(&format!("select coalesce(width, name) as v, count(*) from {images}, {texts} group by v order by v"));
    let forward_first = fselect(&format!("select coalesce(width, name) as v from {texts}, {images} group by v order by v desc limit 1"));
    let backward_first = fselect(&format!("select coalesce(width, name) as v from {images}, {texts} group by v order by v desc limit 1"));
    let _ = fs::remove_dir_all(&dir);

    for result in [&forward, &backward, &forward_first, &backward_first] {
        assert_eq!(result.0, Some(0), "{}", result.2);
    }
    assert_eq!(forward.1, "144\t1\n144\t1\n", "a number and a text are different groups");
    assert_eq!(backward.1, forward.1);
    assert_eq!(backward_first.1, forward_first.1);
}

#[test]
fn grouped_in_lists_convert_operands_like_the_row_filter() {
    let dir = std::env::temp_dir().join("fselect_it_grouping_in_conversion");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("1.0"), "x").unwrap();
    fs::write(dir.join("other"), "xy").unwrap();
    let root = dir.display();

    let filtered = fselect(&format!("select name from {root} where name in (size)"));
    let grouped = fselect(&format!("select name, size from {root} group by name, size having name in (size)"));
    let grouped_not = fselect(&format!("select name, size from {root} group by name, size having name not in (size)"));
    let _ = fs::remove_dir_all(&dir);

    for result in [&filtered, &grouped, &grouped_not] {
        assert_eq!(result.0, Some(0), "{}", result.2);
    }
    assert_eq!(filtered.1, "1.0\n");
    assert_eq!(grouped.1, "1.0\t1\n");
    assert_eq!(grouped_not.1, "other\t2\n");
}

#[test]
fn group_identity_does_not_depend_on_whether_where_read_the_key() {
    let dir = std::env::temp_dir().join("fselect_it_grouping_identity_where");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for name in ["a.txt", "b.txt", "c.txt", "d.txt"] {
        fs::write(dir.join(name), "xx").unwrap();
    }
    let root = dir.display();

    let by_size = fselect(&format!("select size, count(*) from {root} where name = 'a.txt' or size > 0 group by size order by 2"));
    let by_ext = fselect(&format!("select ext, count(*) from {root} where name = 'a.txt' or ext = 'txt' group by ext"));
    let by_type = fselect(&format!("select is_file, count(*) from {root} where name = 'a.txt' or is_file group by is_file"));
    let having = fselect(&format!("select size, count(*) from {root} where name = 'a.txt' or size > 0 group by size having count(*) = 4"));
    let _ = fs::remove_dir_all(&dir);

    for result in [&by_size, &by_ext, &by_type, &having] {
        assert_eq!(result.0, Some(0), "{}", result.2);
    }
    assert_eq!(by_size.1, "2\t4\n");
    assert_eq!(by_ext.1, "txt\t4\n");
    assert_eq!(by_type.1, "true\t4\n");
    assert_eq!(having.1, "2\t4\n");
}

#[test]
fn grouping_keeps_computed_values_built_on_unknown_comparisons() {
    let dir = std::env::temp_dir().join("fselect_it_grouping_computed_unknown");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for name in ["a.txt", "b.txt", "c.txt", "d.txt"] {
        fs::write(dir.join(name), "text").unwrap();
    }
    let root = dir.display();

    let compared = fselect(&format!("select (width > 0) = false, count(*) from {root} group by (width > 0) = false"));
    let negated = fselect(&format!("select -(width > 0), count(*) from {root} group by -(width > 0)"));
    let absolute = fselect(&format!("select abs(-(width > 0)), count(*) from {root} group by -(width > 0)"));
    let having = fselect(&format!("select -(width > 0), count(*) from {root} group by -(width > 0) having -(width > 0) = 0"));
    let _ = fs::remove_dir_all(&dir);

    for result in [&compared, &negated, &absolute, &having] {
        assert_eq!(result.0, Some(0), "{}", result.2);
    }
    assert_eq!(compared.1, "true\t4\n");
    assert_eq!(negated.1, "0\t4\n");
    assert_eq!(absolute.1, "0\t4\n");
    assert_eq!(having.1, "0\t4\n");
}

#[test]
fn unknown_comparisons_stay_unknown_through_in_lists_compounds_and_aliases() {
    let dir = std::env::temp_dir().join("fselect_it_grouping_unknown_forms");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for name in ["a.txt", "b.txt", "c.txt", "d.txt"] {
        fs::write(dir.join(name), "text").unwrap();
    }
    let root = dir.display();

    let in_list = fselect(&format!("select width in (0), width not in (0), count(*) from {root} group by width in (0)"));
    let compound_unknown = fselect(&format!(
        "select width > 0 and is_file, not (width > 0 and is_file), count(*) from {root} group by width > 0 and is_file"
    ));
    let compound_false = fselect(&format!(
        "select width > 0 and size > 100, not (width > 0 and size > 100), count(*) from {root} group by width > 0 and size > 100"
    ));
    let through_where = fselect(&format!(
        "select width > 0 as w, width <= 0, count(*) from {root} where width > 0 or size > 0 group by w"
    ));
    let _ = fs::remove_dir_all(&dir);

    for result in [&in_list, &compound_unknown, &compound_false, &through_where] {
        assert_eq!(result.0, Some(0), "{}", result.2);
    }
    assert_eq!(in_list.1, "false\tfalse\t4\n");
    assert_eq!(compound_unknown.1, "false\tfalse\t4\n", "unknown AND true is unknown, so its negation is not true");
    assert_eq!(compound_false.1, "false\ttrue\t4\n", "unknown AND false is false, so its negation is true");
    assert_eq!(through_where.1, "false\tfalse\t4\n");
}
