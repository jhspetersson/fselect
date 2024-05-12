pub fn contains_japanese(s: &str) -> bool {
    s.chars().any(wana_kana::utils::is_char_japanese)
}

pub fn contains_hiragana(s: &str) -> bool {
    s.chars().any(wana_kana::utils::is_char_hiragana)
}

pub fn contains_katakana(s: &str) -> bool {
    s.chars().any(wana_kana::utils::is_char_katakana)
}

pub fn contains_kana(s: &str) -> bool {
    s.chars().any(wana_kana::utils::is_char_kana)
}

pub fn contains_kanji(s: &str) -> bool {
    s.chars().any(wana_kana::utils::is_char_kanji)
}
