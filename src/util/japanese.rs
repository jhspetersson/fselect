const HIRAGANA: [(u32, u32); 2] = [(0x3041, 0x3096), (0x30FC, 0x30FC)];

const KATAKANA: [(u32, u32); 2] = [(0x30A1, 0x30FC), (0xFF66, 0xFF9F)];

const KANJI: [(u32, u32); 5] = [
    (0x3005, 0x3007),
    (0x3400, 0x4DBF),
    (0x4E00, 0x9FFF),
    (0xF900, 0xFAFF),
    (0x20000, 0x323AF),
];

const JAPANESE_EXTRAS: [(u32, u32); 4] = [
    (0x3000, 0x30FF),
    (0xFF01, 0xFF60),
    (0xFF61, 0xFF9F),
    (0xFFE0, 0xFFEE),
];

fn in_ranges(c: char, ranges: &[(u32, u32)]) -> bool {
    let code = c as u32;
    ranges.iter().any(|&(start, end)| start <= code && code <= end)
}

fn is_hiragana(c: char) -> bool {
    in_ranges(c, &HIRAGANA)
}

fn is_katakana(c: char) -> bool {
    in_ranges(c, &KATAKANA)
}

fn is_kanji(c: char) -> bool {
    in_ranges(c, &KANJI)
}

fn is_japanese(c: char) -> bool {
    is_kanji(c) || in_ranges(c, &JAPANESE_EXTRAS)
}

pub fn contains_japanese(s: &str) -> bool {
    s.chars().any(is_japanese)
}

pub fn contains_hiragana(s: &str) -> bool {
    s.chars().any(is_hiragana)
}

pub fn contains_katakana(s: &str) -> bool {
    s.chars().any(is_katakana)
}

pub fn contains_kana(s: &str) -> bool {
    s.chars().any(|c| is_hiragana(c) || is_katakana(c))
}

pub fn contains_kanji(s: &str) -> bool {
    s.chars().any(is_kanji)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(name: &str, f: fn(&str) -> bool, matching: &[&str], not_matching: &[&str]) {
        for s in matching {
            assert!(f(s), "{}({:?}) should be true", name, s);
        }
        for s in not_matching {
            assert!(!f(s), "{}({:?}) should be false", name, s);
        }
    }

    #[test]
    fn hiragana_covers_its_block_and_the_prolonged_sound_mark() {
        check(
            "hiragana",
            contains_hiragana,
            &["こんにちは", "\u{3041}", "\u{3096}", "ー", "abcで"],
            &["カタカナ", "ｶﾀｶﾅ", "漢字", "abc", "", "\u{3040}", "\u{3097}", "、"],
        );
    }

    #[test]
    fn katakana_covers_full_width_and_half_width_forms() {
        check(
            "katakana",
            contains_katakana,
            &["カタカナ", "\u{30A1}", "\u{30FC}", "ヴ", "・", "ｶﾀｶﾅ", "\u{FF66}", "\u{FF9F}", "ｰ", "ﾞ"],
            &["ひらがな", "漢字", "abc", "", "\u{30A0}", "\u{30FD}", "\u{FF65}", "\u{FFA0}", "｡｢｣､"],
        );
    }

    #[test]
    fn kana_is_hiragana_or_katakana() {
        check(
            "kana",
            contains_kana,
            &["ひら", "カタ", "ｶﾀ", "ー"],
            &["漢字", "abc", "、。", "１２３", ""],
        );
    }

    #[test]
    fn kanji_covers_every_han_block_and_the_iteration_marks() {
        check(
            "kanji",
            contains_kanji,
            &[
                "漢字",
                "一",
                "\u{4E00}",
                "\u{9FFF}",
                "鿀",
                "\u{3400}",
                "\u{4DBF}",
                "\u{F900}",
                "\u{FAFF}",
                "\u{20000}",
                "\u{20BB7}",
                "\u{2A700}",
                "\u{30000}",
                "\u{323AF}",
                "々",
                "〆",
                "〇",
                "abc漢",
            ],
            &[
                "ひらがな",
                "カタカナ",
                "ｶﾀｶﾅ",
                "ー",
                "abc",
                "、。「」",
                "１",
                "",
                "\u{33FF}",
                "\u{4DC0}",
                "\u{A000}",
                "\u{F8FF}",
                "\u{FB00}",
                "\u{1FFFF}",
                "\u{323B0}",
                "\u{3004}",
                "\u{3008}",
                "한",
            ],
        );
    }

    #[test]
    fn japanese_covers_scripts_punctuation_and_full_width_forms() {
        check(
            "japanese",
            contains_japanese,
            &[
                "漢", "か", "カ", "ｶ", "\u{20BB7}", "々", "、", "。", "「」", "\u{3000}", "１", "Ａ", "ａ", "！", "＠", "｀",
                "￥", "｡", "\u{FF01}", "\u{FF60}", "\u{FFE0}", "\u{FFEE}",
            ],
            &["abc", "123", "", "é", "한", "\u{2FFF}", "\u{FF00}", "\u{FFDF}", "\u{FFEF}"],
        );
    }
}
