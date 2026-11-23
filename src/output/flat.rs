//! Handles export of results in line-separated, list-separated, and tab-separated formats

use crate::output::ResultsFormatter;

pub struct FlatWriter {
    record_separator: &'static str,
    line_separator: &'static str,
}

fn line_ending(crlf: bool) -> &'static str {
    if crlf { "\r\n" } else { "\n" }
}

impl FlatWriter {
    pub fn lines(crlf: bool) -> FlatWriter {
        FlatWriter {
            record_separator: line_ending(crlf),
            line_separator: line_ending(crlf),
        }
    }

    pub fn list() -> FlatWriter {
        FlatWriter {
            record_separator: "\0",
            line_separator: "\0",
        }
    }

    pub fn tabs(crlf: bool) -> FlatWriter {
        FlatWriter {
            record_separator: "\t",
            line_separator: line_ending(crlf),
        }
    }
}

impl ResultsFormatter for FlatWriter {
    fn header(&mut self, _: &str, _: usize) -> Option<String> {
        None
    }

    fn row_started(&mut self) -> Option<String> {
        None
    }

    fn format_element(&mut self, _: &str, record: &str, is_last: bool) -> Option<String> {
        match is_last {
            true => Some(record.to_string()),
            false => Some(format!("{}{}", record, self.record_separator)),
        }
    }

    fn row_ended(&mut self) -> Option<String> {
        Some(self.line_separator.to_string())
    }

    fn footer(&mut self) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod test {
    use crate::output::flat::FlatWriter;
    use crate::output::test::write_test_items;

    #[test]
    fn test_lines() {
        let result = write_test_items(&mut FlatWriter::lines(false));
        assert_eq!("foo_value\nBAR value\n123\n\n", result);
    }

    #[test]
    fn test_lines_crlf() {
        let result = write_test_items(&mut FlatWriter::lines(true));
        assert_eq!("foo_value\r\nBAR value\r\n123\r\n\r\n", result);
    }

    #[test]
    fn test_list() {
        let result = write_test_items(&mut FlatWriter::list());
        assert_eq!("foo_value\x00BAR value\x00123\x00\x00", result);
    }

    #[test]
    fn test_tab() {
        let result = write_test_items(&mut FlatWriter::tabs(false));
        assert_eq!("foo_value\tBAR value\n123\t\n", result);
    }

    #[test]
    fn test_tab_crlf() {
        let result = write_test_items(&mut FlatWriter::tabs(true));
        assert_eq!("foo_value\tBAR value\r\n123\t\r\n", result);
    }
}
