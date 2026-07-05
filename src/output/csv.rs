//! Handles export of results in CSV format

use crate::output::ResultsFormatter;

#[derive(Default)]
pub struct CsvFormatter {
    records: Vec<String>,
}

impl ResultsFormatter for CsvFormatter {
    fn header(&mut self, _: &str, _: usize) -> Option<String> {
        None
    }

    fn row_started(&mut self) -> Option<String> {
        None
    }

    fn format_element(&mut self, _: &str, record: &str, _is_last: bool) -> Option<String> {
        self.records.push(record.to_owned());
        None
    }

    fn row_ended(&mut self) -> Option<String> {
        // Write into a plain byte buffer: the csv writer flushes its internal
        // 8KB buffer at arbitrary byte offsets, which a UTF-8-validating sink
        // would reject mid-character, silently dropping the whole row.
        let mut csv_writer = csv::Writer::from_writer(Vec::new());
        let write_result = csv_writer.write_record(&self.records);
        self.records.clear();
        if write_result.is_err() {
            return None;
        }

        match csv_writer.into_inner() {
            Ok(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
            Err(_) => None,
        }
    }

    fn footer(&mut self) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod test {
    use crate::output::csv::CsvFormatter;
    use crate::output::test::write_test_items;

    #[test]
    fn test() {
        let result = write_test_items(&mut CsvFormatter::default());
        assert_eq!("foo_value,BAR value\n123,\n", result);
    }

    #[test]
    fn multibyte_row_larger_than_internal_buffer_is_not_dropped() {
        use crate::output::ResultsFormatter;

        // A row larger than the csv writer's 8KB internal buffer used to be
        // flushed at a non-character byte boundary and silently discarded.
        let mut formatter = CsvFormatter::default();
        let big_value = "é".repeat(6000); // 12000 bytes
        formatter.format_element("col1", &big_value, false);
        formatter.format_element("col2", "x", true);

        let row = formatter.row_ended().expect("row must be produced");
        assert!(row.contains(&big_value));
        assert!(row.ends_with("x\n"));
    }
}
