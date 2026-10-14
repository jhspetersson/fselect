//! Handles export of results in JSON format

use std::collections::HashSet;

use crate::output::ResultsFormatter;

#[derive(Default)]
pub struct JsonFormatter {
    /// Column values in the order they were emitted. Stored as a `Vec` rather
    /// than a map so we preserve the user's SELECT column order and so we can
    /// disambiguate duplicate column names below.
    row: Vec<(String, String)>,
}

impl ResultsFormatter for JsonFormatter {
    fn header(&mut self, _: &str, _: usize) -> Option<String> {
        Some("[".to_owned())
    }

    fn row_started(&mut self) -> Option<String> {
        None
    }

    fn format_element(&mut self, name: &str, record: &str, _is_last: bool) -> Option<String> {
        self.row.push((name.to_owned(), record.to_owned()));
        None
    }

    fn row_ended(&mut self) -> Option<String> {
        // Resolve duplicate column names by suffixing repeats with `_2`,
        // `_3`, ... so the emitted object is always valid JSON with unique
        // keys. The first occurrence keeps its original name.
        let names: HashSet<&str> = self.row.iter().map(|(name, _)| name.as_str()).collect();
        let mut emitted: HashSet<String> = HashSet::new();
        let mut out = String::from("{");
        for (i, (name, value)) in self.row.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            let resolved = if emitted.contains(name) {
                (2..)
                    .map(|n| format!("{}_{}", name, n))
                    .find(|candidate| !names.contains(candidate.as_str()) && !emitted.contains(candidate))
                    .unwrap()
            } else {
                name.clone()
            };
            emitted.insert(resolved.clone());
            out.push_str(&serde_json::to_string(&resolved).unwrap_or_else(|_| "\"\"".to_string()));
            out.push(':');
            out.push_str(&serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string()));
        }
        out.push('}');
        self.row.clear();
        Some(out)
    }

    fn footer(&mut self) -> Option<String> {
        Some("]".to_owned())
    }

    fn row_separator(&self) -> Option<String> {
        Some(",".to_owned())
    }
}

#[cfg(test)]
mod test {
    use crate::output::ResultsFormatter;
    use crate::output::json::JsonFormatter;
    use crate::output::test::write_test_items;

    #[test]
    fn test() {
        let result = write_test_items(&mut JsonFormatter::default());
        // Columns must appear in the order they were emitted (foo, bar),
        // not alphabetised by a BTreeMap.
        assert_eq!(
            r#"[{"foo":"foo_value","bar":"BAR value"},{"foo":"123","bar":""}]"#,
            result
        );
    }

    #[test]
    fn duplicate_column_names_are_suffixed() {
        // Duplicate column names within one row must produce a *valid* JSON
        // object: first occurrence keeps the name, subsequent ones are
        // suffixed with `_2`, `_3`, ...
        let mut f = JsonFormatter::default();
        let mut out = String::new();
        if let Some(s) = f.header("", 3) { out.push_str(&s); }
        if let Some(s) = f.row_started() { out.push_str(&s); }
        if let Some(s) = f.format_element("name", "first", false) { out.push_str(&s); }
        if let Some(s) = f.format_element("name", "second", false) { out.push_str(&s); }
        if let Some(s) = f.format_element("name", "third", true) { out.push_str(&s); }
        if let Some(s) = f.row_ended() { out.push_str(&s); }
        if let Some(s) = f.footer() { out.push_str(&s); }
        assert_eq!(
            out,
            r#"[{"name":"first","name_2":"second","name_3":"third"}]"#,
        );
    }

    #[test]
    fn generated_suffixes_never_collide_with_explicit_names() {
        let render = |columns: &[(&str, &str)]| {
            let mut f = JsonFormatter::default();
            for (name, value) in columns {
                f.format_element(name, value, false);
            }
            f.row_ended().unwrap()
        };
        assert_eq!(
            render(&[("name", "a"), ("name", "b"), ("name_2", "c")]),
            r#"{"name":"a","name_3":"b","name_2":"c"}"#,
        );
        assert_eq!(
            render(&[("name", "a"), ("name_2", "c"), ("name", "b")]),
            r#"{"name":"a","name_2":"c","name_3":"b"}"#,
        );
        assert_eq!(
            render(&[("name", "a"), ("name", "b"), ("name_2", "c"), ("name_3", "d"), ("name", "e")]),
            r#"{"name":"a","name_4":"b","name_2":"c","name_3":"d","name_5":"e"}"#,
        );
    }

    #[test]
    fn duplicate_counter_resets_between_rows() {
        // The duplicate-suffix counter is per-row state and must not leak
        // into subsequent rows.
        let mut f = JsonFormatter::default();
        let mut out = String::new();
        if let Some(s) = f.header("", 1) { out.push_str(&s); }
        if let Some(s) = f.row_started() { out.push_str(&s); }
        if let Some(s) = f.format_element("name", "a", false) { out.push_str(&s); }
        if let Some(s) = f.format_element("name", "b", true) { out.push_str(&s); }
        if let Some(s) = f.row_ended() { out.push_str(&s); }
        if let Some(s) = f.row_separator() { out.push_str(&s); }
        if let Some(s) = f.row_started() { out.push_str(&s); }
        if let Some(s) = f.format_element("name", "c", true) { out.push_str(&s); }
        if let Some(s) = f.row_ended() { out.push_str(&s); }
        if let Some(s) = f.footer() { out.push_str(&s); }
        assert_eq!(
            out,
            r#"[{"name":"a","name_2":"b"},{"name":"c"}]"#,
        );
    }
}
