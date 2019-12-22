pub fn contains_japanese(s: &str) -> bool {
    s.chars().any(|c| wana_kana::utils::is_char_japanese(c))
}

pub fn contains_hiragana(s: &str) -> bool {
    s.chars().any(|c| wana_kana::utils::is_char_hiragana(c))
}

pub fn contains_katakana(s: &str) -> bool {
    s.chars().any(|c| wana_kana::utils::is_char_katakana(c))
}

pub fn contains_kana(s: &str) -> bool {
    s.chars().any(|c| wana_kana::utils::is_char_kana(c))
}

pub fn contains_kanji(s: &str) -> bool {
    s.chars().any(|c| wana_kana::utils::is_char_kanji(c))
}