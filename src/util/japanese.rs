pub fn contains_japanese(s: &str) -> bool {
    let tokens = wana_kana::tokenize::tokenize(s);
    let result = tokens.iter().any(|token|
        wana_kana::is_kana::is_kana(token) | wana_kana::is_kanji::is_kanji(token)
    );

    return result;
}

pub fn contains_hiragana(s: &str) -> bool {
    let tokens = wana_kana::tokenize::tokenize(s);
    let result = tokens.iter().any(|token| wana_kana::is_hiragana::is_hiragana(token));

    return result;
}

pub fn contains_katakana(s: &str) -> bool {
    let tokens = wana_kana::tokenize::tokenize(s);
    let result = tokens.iter().any(|token| wana_kana::is_katakana::is_katakana(token));

    return result;
}

pub fn contains_kana(s: &str) -> bool {
    let tokens = wana_kana::tokenize::tokenize(s);
    let result = tokens.iter().any(|token| wana_kana::is_kana::is_kana(token));

    return result;
}

pub fn contains_kanji(s: &str) -> bool {
    let tokens = wana_kana::tokenize::tokenize(s);
    let result = tokens.iter().any(|token| wana_kana::is_kanji::is_kanji(token));

    return result;
}