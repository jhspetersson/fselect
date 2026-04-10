//! Handles export of results in HTML format

use crate::output::ResultsFormatter;

pub struct HtmlFormatter;

fn escape_html(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => result.push_str("&amp;"),
            '<' => result.push_str("&lt;"),
            '>' => result.push_str("&gt;"),
            '"' => result.push_str("&quot;"),
            _ => result.push(c),
        }
    }
    result
}

impl ResultsFormatter for HtmlFormatter {
    fn header(&mut self, raw_query: &str, col_count: usize) -> Option<String> {
        let escaped = escape_html(raw_query);
        Some(format!("<html><head><title>{}</title></head><body><table><tr><th colspan=\"{}\">{}</th></tr>", escaped, col_count, escaped))
    }

    fn row_started(&mut self) -> Option<String> {
        Some("<tr>".to_owned())
    }

    fn format_element(&mut self, _: &str, record: &str, _is_last: bool) -> Option<String> {
        Some(format!("<td>{}</td>", escape_html(record)))
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
    use super::*;
    use crate::output::html::HtmlFormatter;
    use crate::output::test::write_test_items;

    #[test]
    fn test() {
        let result = write_test_items(&mut HtmlFormatter);
        assert_eq!("<html><head><title>select key, value</title></head><body><table><tr><th colspan=\"2\">select key, value</th></tr><tr><td>foo_value</td><td>BAR value</td></tr><tr><td>123</td><td></td></tr></table></body></html>", result);
    }

    #[test]
    fn test_escape_html() {
        assert_eq!(escape_html("<script>alert(1)</script>"), "&lt;script&gt;alert(1)&lt;/script&gt;");
        assert_eq!(escape_html("a&b"), "a&amp;b");
        assert_eq!(escape_html("\"hello\""), "&quot;hello&quot;");
    }
}
