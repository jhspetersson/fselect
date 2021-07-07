use crate::output::ResultsFormatter;

pub const LINES_FORMATTER: FlatWriter = FlatWriter {
    record_separator: '\n',
    line_separator: None,
};


pub const LIST_FORMATTER: FlatWriter = FlatWriter {
    record_separator: '\0',
    line_separator: None,
};


pub const TABS_FORMATTER: FlatWriter = FlatWriter {
    record_separator: '\t',
    line_separator: Some('\n'),
};


pub struct FlatWriter {
    record_separator: char,
    line_separator: Option<char>,
}

impl ResultsFormatter for FlatWriter {
    fn header(&mut self) -> Option<String> {
        None
    }

    fn row_started(&mut self) -> Option<String> {
        None
    }

    fn format_element(&mut self, _: &str, record: &str) -> Option<String> {
        Some(format!("{}{}", record, self.record_separator))
    }

    fn row_ended(&mut self) -> Option<String> {
        self.line_separator.map(String::from)
    }

    fn footer(&mut self) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod test {
    #![allow(const_item_mutation)]
    use crate::output::flat::{LIST_FORMATTER, LINES_FORMATTER, TABS_FORMATTER};
    use crate::output::test::write_test_items;

    #[test]
    fn test_lines() {
        let result = write_test_items(&mut LINES_FORMATTER);
        assert_eq!("foo_value\nBAR value\n123\n\n", result);
    }

    #[test]
    fn test_list() {
        let result = write_test_items(&mut LIST_FORMATTER);
        assert_eq!("foo_value\0BAR value\0123\0\0", result);
    }

    #[test]
    fn test_tab() {
        let result = write_test_items(&mut TABS_FORMATTER);
        assert_eq!("foo_value\tBAR value\t\n123\t\t\n", result);
    }
}
