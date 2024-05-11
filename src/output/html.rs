//! Handles export of results in HTML format

use crate::output::ResultsFormatter;

pub struct HtmlFormatter;

impl ResultsFormatter for HtmlFormatter {
    fn header(&mut self) -> Option<String> {
        Some("<html><body><table>".to_owned())
    }

    fn row_started(&mut self) -> Option<String> {
        Some("<tr>".to_owned())
    }

    fn format_element(&mut self, _: &str, record: &str, _is_last: bool) -> Option<String> {
        Some(format!("<td>{}</td>", record))
    }

    fn row_ended(&mut self) -> Option<String> {
        Some("</tr>".to_owned())
    }

    fn footer(&mut self) -> Option<String> {
        Some("</table></body></html>".to_owned())
    }
}

#[cfg(test)]
mod test {
    use crate::output::html::HtmlFormatter;
    use crate::output::test::write_test_items;

    #[test]
    fn test() {
        let result = write_test_items(&mut HtmlFormatter);
        assert_eq!("<html><body><table><tr><td>foo_value</td><td>BAR value</td></tr><tr><td>123</td><td></td></tr></table></body></html>", result);
    }
}
