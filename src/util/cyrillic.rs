const CYRILLIC: [(u32, u32); 8] = [
    (0x0400, 0x052F),
    (0x1C80, 0x1C8F),
    (0x1D2B, 0x1D2B),
    (0x1D78, 0x1D78),
    (0x2DE0, 0x2DFF),
    (0xA640, 0xA69F),
    (0xFE2E, 0xFE2F),
    (0x1E030, 0x1E08F),
];

pub fn contains_cyrillic(s: &str) -> bool {
    s.chars().any(|c| {
        let code = c as u32;
        CYRILLIC.iter().any(|&(start, end)| start <= code && code <= end)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cyrillic_covers_every_cyrillic_block() {
        for s in [
            "Привет",
            "ёж",
            "Ґ",
            "ѣ",
            "\u{0400}",
            "\u{04FF}",
            "\u{0500}",
            "\u{052F}",
            "\u{1C80}",
            "\u{1C8F}",
            "\u{1D2B}",
            "\u{1D78}",
            "\u{2DE0}",
            "\u{2DFF}",
            "\u{A640}",
            "\u{A69F}",
            "\u{FE2E}",
            "\u{FE2F}",
            "\u{1E030}",
            "\u{1E08F}",
            "file_Я.txt",
        ] {
            assert!(contains_cyrillic(s), "cyrillic({:?}) should be true", s);
        }
        for s in [
            "abc",
            "Ελληνικά",
            "日本",
            "é",
            "",
            "\u{03FF}",
            "\u{0530}",
            "\u{1C7F}",
            "\u{1C90}",
            "\u{2DDF}",
            "\u{2E00}",
            "\u{A63F}",
            "\u{A6A0}",
            "\u{1E02F}",
            "\u{1E090}",
        ] {
            assert!(!contains_cyrillic(s), "cyrillic({:?}) should be false", s);
        }
    }
}
