//! Handles export of results in CSV format

use crate::output::ResultsFormatter;
use crate::util::WritableBuffer;

#[derive(Default)]
pub struct CsvFormatter {
    records: Vec<String>,
}

impl ResultsFormatter for CsvFormatter {
    fn header(&mut self) -> Option<String> {
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
        let mut csv_output = WritableBuffer::new();
        {
            let mut csv_writer = csv::Writer::from_writer(&mut csv_output);
            let _ = csv_writer.write_record(&self.records);
            self.records.clear();
        }
        Some(csv_output.into())
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
}
