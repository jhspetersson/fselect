use std::io::Write;
use crate::query::OutputFormat;
use crate::output::flat::{TABS_FORMATTER, LINES_FORMATTER, LIST_FORMATTER};
use crate::output::json::JsonFormatter;
use crate::output::html::HtmlFormatter;
use crate::output::csv::CsvFormatter;

mod json;
mod flat;
mod html;
mod csv;

pub trait ResultsFormatter {
    fn header(&mut self) -> Option<String>;
    fn row_started(&mut self) -> Option<String>;
    fn format_element(&mut self, name: &str, record: &str) -> Option<String>;
    fn row_ended(&mut self) -> Option<String>;
    fn footer(&mut self) -> Option<String>;

    fn row_separator(&self) -> Option<String> {
        None
    }
}

pub struct ResultsWriter {
    formatter: Box<dyn ResultsFormatter>,
}

impl ResultsWriter {
    pub fn new(format: &OutputFormat) -> ResultsWriter {
        ResultsWriter { formatter: select_formatter(format) }
    }

    pub fn write_header(&mut self, writer: &mut dyn Write) -> std::io::Result<()> {
        self.formatter.header()
            .map_or(Ok(()), |value| write!(writer, "{}", value))
    }

    pub fn write_row_separator(&mut self, writer: &mut dyn Write) -> std::io::Result<()> {
        self.formatter.row_started()
            .map_or(Ok(()), |value| write!(writer, "{}", value))
    }

    pub fn write_row(&mut self, writer: &mut dyn Write, values: Vec<(String, String)>) -> std::io::Result<()> {
        self.write_row_start(writer)?;
        for (name, value) in values {
            self.write_row_item(writer, &name, &value)?;
        }
        self.write_row_end(writer)
    }

    pub fn write_footer(&mut self, writer: &mut dyn Write) -> std::io::Result<()> {
        self.formatter.footer()
            .map_or(Ok(()), |value| write!(writer, "{}", value))
    }

    fn write_row_start(&mut self, writer: &mut dyn Write) -> std::io::Result<()> {
        self.formatter.row_started()
            .map_or(Ok(()), |value| write!(writer, "{}", value))
    }
    fn write_row_item(&mut self, writer: &mut dyn Write, name: &str, value: &str) -> std::io::Result<()> {
        self.formatter.format_element(name, value)
            .map_or(Ok(()), |value| write!(writer, "{}", value))
    }

    fn write_row_end(&mut self, writer: &mut dyn Write) -> std::io::Result<()> {
        self.formatter.row_ended()
            .map_or(Ok(()), |value| write!(writer, "{}", value))
    }
}


fn select_formatter(format: &OutputFormat) -> Box<dyn ResultsFormatter> {
    match format {
        OutputFormat::Tabs => Box::new(TABS_FORMATTER),
        OutputFormat::Lines => Box::new(LINES_FORMATTER),
        OutputFormat::List => Box::new(LIST_FORMATTER),
        OutputFormat::Csv => Box::new(CsvFormatter::default()),
        OutputFormat::Json => Box::new(JsonFormatter::default()),
        OutputFormat::Html => Box::new(HtmlFormatter),
    }
}

#[cfg(test)]
mod test {
    use crate::output::ResultsFormatter;

    pub(crate) fn write_test_items<T: ResultsFormatter>(under_test: &mut T) -> String {
        let mut result = String::from("");
        under_test.header().and_then(|s| Some(result.push_str(&s)));
        under_test.row_started().and_then(|s| Some(result.push_str(&s)));
        under_test.format_element("foo", "foo_value").and_then(|s| Some(result.push_str(&s)));
        under_test.format_element("bar", "BAR value").and_then(|s| Some(result.push_str(&s)));
        under_test.row_ended().and_then(|s| Some(result.push_str(&s)));
        under_test.row_separator().and_then(|s| Some(result.push_str(&s)));
        under_test.row_started().and_then(|s| Some(result.push_str(&s)));
        under_test.format_element("foo", "123").and_then(|s| Some(result.push_str(&s)));
        under_test.format_element("bar", "").and_then(|s| Some(result.push_str(&s)));
        under_test.row_ended().and_then(|s| Some(result.push_str(&s)));
        under_test.footer().and_then(|s| Some(result.push_str(&s)));
        result
    }
}

