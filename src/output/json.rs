//! Handles export of results in JSON format

use crate::output::ResultsFormatter;
use std::collections::BTreeMap;

#[derive(Default)]
pub struct JsonFormatter {
    file_map: BTreeMap<String, String>,
}

impl ResultsFormatter for JsonFormatter {
    fn header(&mut self) -> Option<String> {
        Some("[".to_owned())
    }

    fn row_started(&mut self) -> Option<String> {
        None
    }

    fn format_element(&mut self, name: &str, record: &str, _is_last: bool) -> Option<String> {
        self.file_map.insert(name.to_owned(), record.to_owned());
        None
    }

    fn row_ended(&mut self) -> Option<String> {
        let result = serde_json::to_string(&self.file_map).unwrap();
        self.file_map.clear();
        Some(result)
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
    use crate::output::json::JsonFormatter;
    use crate::output::test::write_test_items;

    #[test]
    fn test() {
        let result = write_test_items(&mut JsonFormatter::default());
        assert_eq!(
            r#"[{"bar":"BAR value","foo":"foo_value"},{"bar":"","foo":"123"}]"#,
            result
        );
    }
}
