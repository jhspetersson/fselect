use std::fs;
use std::path::{Path, PathBuf};
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

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("resources").join("test").join("image").join(name)
}

fn mixed_roots(name: &str) -> (PathBuf, [String; 3]) {
    let dir = std::env::temp_dir().join(name);
    let _ = fs::remove_dir_all(&dir);
    for sub in ["png", "svg", "text"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    fs::copy(fixture("rust-logo-blk.png"), dir.join("png").join("logo.png")).unwrap();
    fs::copy(fixture("rect.svg"), dir.join("svg").join("rect.svg")).unwrap();
    fs::write(dir.join("text").join("150"), "x").unwrap();
    let roots = [dir.join("png"), dir.join("svg"), dir.join("text")].map(|p| p.display().to_string());
    (dir, roots)
}

fn permutations(roots: &[String; 3]) -> Vec<String> {
    let mut result = Vec::new();
    for a in 0..3 {
        for b in 0..3 {
            for c in 0..3 {
                if a != b && b != c && a != c {
                    result.push(format!("{}, {}, {}", roots[a], roots[b], roots[c]));
                }
            }
        }
    }
    result
}

#[test]
fn mixed_type_sort_keys_are_stable_under_root_order() {
    let (dir, roots) = mixed_roots("fselect_it_ordering_mixed_keys");

    let mut outcomes = Vec::new();
    for from in permutations(&roots) {
        let by_width_then_name = fselect(&format!("select name from {from} order by coalesce(width, name)"));
        let first_desc = fselect(&format!("select name from {from} order by coalesce(width, name) desc limit 1"));
        let by_width_then_date = fselect(&format!("select name from {from} order by coalesce(width, modified)"));
        outcomes.push((from, by_width_then_name, first_desc, by_width_then_date));
    }
    let _ = fs::remove_dir_all(&dir);

    for (from, by_width_then_name, first_desc, by_width_then_date) in &outcomes {
        for result in [by_width_then_name, first_desc, by_width_then_date] {
            assert_eq!(result.0, Some(0), "{from}: {}", result.2);
        }
        assert_eq!(by_width_then_name.1, "logo.png\nrect.svg\n150\n", "{from}: numbers before text");
        assert_eq!(first_desc.1, "150\n", "{from}");
        assert_eq!(by_width_then_date.1, "logo.png\nrect.svg\n150\n", "{from}: numbers before dates");
    }
}

#[test]
fn grouped_ordering_uses_the_same_mixed_key_rules() {
    let (dir, roots) = mixed_roots("fselect_it_ordering_grouped_mixed_keys");

    let mut outcomes = Vec::new();
    for from in permutations(&roots) {
        let grouped = fselect(&format!("select coalesce(width, name) as v, count(*) from {from} group by v order by v"));
        let last = fselect(&format!("select coalesce(width, name) as v from {from} group by v order by v desc limit 1"));
        outcomes.push((from, grouped, last));
    }
    let _ = fs::remove_dir_all(&dir);

    for (from, grouped, last) in &outcomes {
        assert_eq!(grouped.0, Some(0), "{from}: {}", grouped.2);
        assert_eq!(grouped.1, "144\t1\n200\t1\n150\t1\n", "{from}");
        assert_eq!(last.1, "150\n", "{from}");
    }
}

#[test]
fn text_results_of_value_functions_sort_as_text() {
    let dir = std::env::temp_dir().join("fselect_it_ordering_text_results");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for name in ["1.0", "2", "10"] {
        fs::write(dir.join(name), "x").unwrap();
    }
    let root = dir.display();

    let coalesced = fselect(&format!("select name from {root} order by coalesce(name, size)"));
    let least_first = fselect(&format!("select name from {root} order by least(name, 'z') desc limit 1"));
    let greatest = fselect(&format!("select name from {root} order by greatest(name, '.')"));
    let numeric = fselect(&format!("select name from {root} order by least(size, 5), name"));
    let _ = fs::remove_dir_all(&dir);

    for result in [&coalesced, &least_first, &greatest, &numeric] {
        assert_eq!(result.0, Some(0), "{}", result.2);
    }
    assert_eq!(coalesced.1, "1.0\n10\n2\n", "an unused numeric fallback does not make the key numeric");
    assert_eq!(least_first.1, "2\n");
    assert_eq!(greatest.1, "1.0\n10\n2\n");
    assert_eq!(numeric.1, "1.0\n10\n2\n", "equal numeric keys fall back to the next column");
}
