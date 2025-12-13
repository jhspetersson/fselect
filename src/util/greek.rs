pub fn contains_greek(s: &str) -> bool {
    s.chars().any(is_greek_char)
}

fn is_greek_char(c: char) -> bool {
    // Greek and Coptic: U+0370–U+03FF
    // Greek Extended: U+1F00–U+1FFF
    matches!(c, '\u{0370}'..='\u{03FF}' | '\u{1F00}'..='\u{1FFF}')
}
