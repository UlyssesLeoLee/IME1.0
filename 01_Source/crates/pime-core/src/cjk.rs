//! The dictionaries of the input method: the Chinese (pinyin), the Japanese (kana) and the English words.
//!
//! An input method is supposed to have its **own** dictionaries: the user types pinyin and gets Chinese, the user
//! types romaji and gets kana, and the user does not need another input method for that. IME1.0 does not borrow the
//! dictionaries of the system - that is what the system's own input methods are for - it has its own.
//!
//! The tables are data files next to this file. They are read once, when the first candidate is asked for, and the
//! candidates themselves are computed by pure functions (so that they can be tested without a screen and without a
//! keyboard).
//!
//! The three dictionaries answer three questions:
//!
//! * [`pinyin_candidates`]: the user typed pinyin, which Chinese does it mean?
//! * [`japanese_candidates`]: the user typed romaji, which kana does it mean?
//! * [`english_candidates`]: the user typed letters, which English word does it mean?

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

/// The characters of every syllable; the first line holds the characters in the order of their frequency.
const PINYIN_SYLLABLES: &str = include_str!("cjk/pinyin-syllables.txt");
/// The phrases of every pinyin (the most frequent first).
const PINYIN_PHRASES: &str = include_str!("cjk/pinyin-phrases.txt");
/// The romaji pieces and the kana of them.
const ROMAJI_KANA: &str = include_str!("cjk/romaji-kana.txt");
/// The words of the Japanese dictionary (the kana -> the kanji forms).
const JAPANESE_WORDS: &str = include_str!("cjk/japanese-words.txt");
/// The English words, the most frequent first.
const ENGLISH_WORDS: &str = include_str!("cjk/english-words.txt");

/// How many candidates a dictionary answers with.
pub const MAX_CANDIDATES: usize = 24;

/// How many characters of a syllable are used for the combinations.
const COMBINATION_WIDTH: usize = 4;

/// The profile of the Chinese dictionary.
pub const PROFILE_ZH: &str = "zh-CN";

/// The profile of the Japanese dictionary.
pub const PROFILE_JA: &str = "ja-JP";

/// The profile of the code (the English profile of the input method).
pub const PROFILE_CODE: &str = "rust";

/// The dictionaries of the input method, read once.
struct Dictionaries {
    /// The characters of every syllable.
    syllables: HashMap<String, String>,
    /// The syllables, sorted (a prefix of a syllable is looked up in them).
    syllable_keys: Vec<String>,
    /// The characters in the order of their frequency.
    order: Vec<char>,
    /// The phrases of every pinyin.
    phrases: HashMap<String, Vec<String>>,
    /// The pinyin of the phrases, sorted.
    phrase_keys: Vec<String>,
    /// The romaji pieces and their kana.
    kana: HashMap<String, String>,
    /// The Japanese words (the kana -> the kanji forms).
    japanese: HashMap<String, Vec<String>>,
    /// The kana of the Japanese words, sorted.
    japanese_keys: Vec<String>,
    /// The English words, the most frequent first.
    english: Vec<String>,
}

/// The dictionaries, read the first time a candidate is asked for.
fn dictionaries() -> &'static Dictionaries {
    static DICTIONARIES: OnceLock<Dictionaries> = OnceLock::new();
    DICTIONARIES.get_or_init(build)
}

/// Read the tables.
fn build() -> Dictionaries {
    let mut syllables: HashMap<String, String> = HashMap::new();
    let mut syllable_keys: Vec<String> = Vec::new();
    let mut order: Vec<char> = Vec::new();
    for line in PINYIN_SYLLABLES.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key == "chars" {
            // the characters in the order of their frequency
            order = value.chars().collect();
            continue;
        }
        if value.is_empty() {
            continue;
        }
        syllables.insert(key.to_string(), value.to_string());
        syllable_keys.push(key.to_string());
    }
    syllable_keys.sort();

    let mut phrases: HashMap<String, Vec<String>> = HashMap::new();
    let mut phrase_keys: Vec<String> = Vec::new();
    for line in PINYIN_PHRASES.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if value.is_empty() {
            continue;
        }
        let list: Vec<String> = value.split(',').filter(|w| !w.is_empty()).map(String::from).collect();
        if list.is_empty() {
            continue;
        }
        phrases.insert(key.to_string(), list);
        phrase_keys.push(key.to_string());
    }
    phrase_keys.sort();

    let mut kana: HashMap<String, String> = HashMap::new();
    for line in ROMAJI_KANA.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.is_empty() || value.is_empty() {
            continue;
        }
        kana.insert(key.to_string(), value.to_string());
    }

    let mut japanese: HashMap<String, Vec<String>> = HashMap::new();
    let mut japanese_keys: Vec<String> = Vec::new();
    for line in JAPANESE_WORDS.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.is_empty() || value.is_empty() {
            continue;
        }
        japanese.insert(key.to_string(), vec![value.to_string()]);
        japanese_keys.push(key.to_string());
    }
    japanese_keys.sort();

    let english: Vec<String> = ENGLISH_WORDS
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .map(String::from)
        .collect();

    Dictionaries {
        syllables,
        syllable_keys,
        order,
        phrases,
        phrase_keys,
        kana,
        japanese,
        japanese_keys,
        english,
    }
}

/// The text the user typed, in the shape the dictionaries want it: lower case, without the apostrophes and without
/// the spaces (a pinyin is written without them).
fn normalise(text: &str) -> String {
    text.trim()
        .to_lowercase()
        .chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '\'' && *ch != '\u{2019}')
        .collect()
}

/// Add a candidate, if it is not there yet.
fn push(out: &mut Vec<String>, seen: &mut HashSet<String>, candidate: String) {
    if candidate.is_empty() {
        return;
    }
    if seen.insert(candidate.clone()) {
        out.push(candidate);
    }
}

/// The syllables of the pinyin that was typed: the complete ones and, at the end, the part that is not a syllable
/// yet (`ni` / `hao` / `zh`).
fn split_syllables(typed: &str) -> (Vec<String>, String) {
    let dict = dictionaries();
    let mut complete: Vec<String> = Vec::new();
    let mut rest = typed;
    while !rest.is_empty() {
        let mut found: Option<&str> = None;
        for length in (1..=6.min(rest.len())).rev() {
            let candidate = &rest[..length];
            if dict.syllables.contains_key(candidate) {
                found = Some(candidate);
                break;
            }
        }
        match found {
            Some(syllable) => {
                complete.push(syllable.to_string());
                rest = &rest[syllable.len()..];
            }
            None => break,
        }
    }
    (complete, rest.to_string())
}

/// The characters of the syllables that start with the text, the most frequent first.
///
/// The order of the characters is the one of the whole dictionary (the most frequent character of every syllable
/// comes first), so that the common characters are the ones the user sees.
fn characters_starting_with(prefix: &str) -> Vec<char> {
    let dict = dictionaries();
    let mut found: Vec<char> = Vec::new();
    for key in dict.syllable_keys.iter() {
        if !key.starts_with(prefix) {
            continue;
        }
        let Some(chars) = dict.syllables.get(key) else {
            continue;
        };
        for ch in chars.chars() {
            if !found.contains(&ch) {
                found.push(ch);
            }
        }
    }
    // the order of the frequency of the characters
    let mut ordered: Vec<char> = Vec::new();
    for ch in &dict.order {
        if found.contains(ch) {
            ordered.push(*ch);
        }
    }
    for ch in found {
        if !ordered.contains(&ch) {
            ordered.push(ch);
        }
    }
    ordered
}

/// The combinations of the characters of the syllables (the most frequent four of each).
fn combinations(syllables: &[String]) -> Vec<String> {
    let dict = dictionaries();
    let mut lists: Vec<Vec<char>> = Vec::new();
    for syllable in syllables {
        let Some(chars) = dict.syllables.get(syllable) else {
            return Vec::new();
        };
        lists.push(chars.chars().take(COMBINATION_WIDTH).collect());
    }
    if lists.iter().any(|list| list.is_empty()) {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    let mut index = vec![0usize; lists.len()];
    loop {
        out.push(lists.iter().zip(&index).map(|(list, i)| list[*i]).collect());
        let mut raised = false;
        for position in (0..lists.len()).rev() {
            if index[position] + 1 < lists[position].len() {
                index[position] += 1;
                raised = true;
                break;
            }
            index[position] = 0;
        }
        if !raised {
            break;
        }
    }
    out
}

/// The candidates of the pinyin that was typed.
///
/// The rules are the ones of every pinyin input method:
/// * one syllable: the characters of that syllable, the most frequent first;
/// * more syllables: the phrases whose pinyin starts with what was typed, and then the combinations of the
///   characters of the syllables.
pub fn pinyin_candidates(prefix: &str) -> Vec<String> {
    let typed = normalise(prefix);
    if typed.is_empty() {
        return Vec::new();
    }
    let dict = dictionaries();
    let (complete, rest) = split_syllables(&typed);
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    if complete.len() > 1 && rest.is_empty() {
        // the pinyin is a whole word: the phrase comes first (你好), then the combinations of the characters
        push_phrases(&mut out, &mut seen, &typed, dict);
        for word in combinations(&complete) {
            push(&mut out, &mut seen, word);
        }
    } else {
        // one syllable, or one that is not finished: the characters come first, the phrases after them
        if complete.len() == 1 && rest.is_empty() {
            if let Some(chars) = dict.syllables.get(&complete[0]) {
                for ch in chars.chars() {
                    push(&mut out, &mut seen, ch.to_string());
                }
            }
        } else if !rest.is_empty() {
            for ch in characters_starting_with(&rest) {
                push(&mut out, &mut seen, ch.to_string());
            }
        }
        push_phrases(&mut out, &mut seen, &typed, dict);
    }

    out.truncate(MAX_CANDIDATES);
    out
}

/// The phrases whose pinyin starts with what was typed.
fn push_phrases(
    out: &mut Vec<String>,
    seen: &mut HashSet<String>,
    typed: &str,
    dict: &Dictionaries,
) {
    for key in dict.phrase_keys.iter().filter(|key| key.starts_with(typed)) {
        let Some(list) = dict.phrases.get(key) else {
            continue;
        };
        for phrase in list {
            push(out, seen, phrase.clone());
        }
    }
}

/// The kana of the romaji that was typed.
///
/// The romaji is read piece by piece (the longest piece that fits wins), a doubled consonant is the small っ and
/// the piece that does not fit yet is left out (the user is still typing it).
///
/// The small っ is looked for **before** the pieces: otherwise the "n" of "nn" would be read as ん and the user
/// would get こん where he wanted きって. The "n" is the one exception (it is ん on its own), so "konnichiha" is
/// こんにちは and not こんっにちは.
pub fn romaji_to_kana(prefix: &str) -> String {
    let typed = normalise(prefix);
    let dict = dictionaries();
    let mut out = String::new();
    let mut rest = typed.as_str();
    while !rest.is_empty() {
        // a doubled consonant is the small っ
        if let Some((kana, after)) = sokuon(rest, dict) {
            out.push('っ');
            out.push_str(&kana);
            rest = after;
            continue;
        }
        // the longest piece that fits
        let mut found: Option<&str> = None;
        for length in (1..=4.min(rest.len())).rev() {
            let candidate = &rest[..length];
            if dict.kana.contains_key(candidate) {
                found = Some(candidate);
                break;
            }
        }
        if let Some(piece) = found {
            out.push_str(dict.kana.get(piece).map(String::as_str).unwrap_or(""));
            rest = &rest[piece.len()..];
            continue;
        }
        // the piece is not finished yet
        break;
    }
    out
}

/// The small っ of a doubled consonant, with the kana of the piece that follows it.
///
/// The "n" is not one of them: it is ん on its own (the "n" of a word like "konnichiha" is the ん of こん).
fn sokuon<'a>(rest: &'a str, dict: &Dictionaries) -> Option<(String, &'a str)> {
    let mut chars = rest.chars();
    let first = chars.next()?;
    let second = chars.next()?;
    if first != second || first == 'n' || "aiueo".contains(first) {
        return None;
    }
    // the syllable that follows starts with the **second** consonant ("tte" is the "te" after the っ)
    let after = &rest[first.len_utf8()..];
    for length in (1..=4.min(after.len())).rev() {
        let candidate = &after[..length];
        if let Some(kana) = dict.kana.get(candidate) {
            return Some((kana.clone(), &after[candidate.len()..]));
        }
    }
    None
}

/// The candidates of the romaji that was typed.
///
/// The words of the dictionary come first (their kanji forms), then the kana itself: a user who types romaji and
/// does not find the word he wants can still take the kana.
pub fn japanese_candidates(prefix: &str) -> Vec<String> {
    let typed = normalise(prefix);
    if typed.is_empty() {
        return Vec::new();
    }
    let dict = dictionaries();
    let kana = romaji_to_kana(&typed);
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for key in dict.japanese_keys.iter().filter(|key| key.starts_with(&kana)) {
        let Some(list) = dict.japanese.get(key) else {
            continue;
        };
        for word in list {
            push(&mut out, &mut seen, word.clone());
        }
    }
    if !kana.is_empty() {
        push(&mut out, &mut seen, kana);
    }
    out.truncate(MAX_CANDIDATES);
    out
}

/// The candidates of the English word that was typed.
pub fn english_candidates(prefix: &str) -> Vec<String> {
    let typed = normalise(prefix);
    if typed.is_empty() {
        return Vec::new();
    }
    let dict = dictionaries();
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for word in dict.english.iter().filter(|word| word.starts_with(&typed)) {
        push(&mut out, &mut seen, word.clone());
        if out.len() >= MAX_CANDIDATES {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_syllable_gives_the_characters_of_it() {
        let candidates = pinyin_candidates("ni");
        assert!(candidates.contains(&"你".to_string()), "你 is the first one");
        assert!(candidates.len() > 3, "a syllable has more than one character");
    }

    #[test]
    fn a_whole_word_gives_the_phrase_of_it() {
        let candidates = pinyin_candidates("nihao");
        assert!(
            candidates.contains(&"你好".to_string()),
            "the phrase of the pinyin is the first candidate: {candidates:?}"
        );
    }

    #[test]
    fn a_syllable_that_is_not_finished_gives_the_characters_that_start_with_it() {
        let candidates = pinyin_candidates("zh");
        assert!(!candidates.is_empty(), "there are characters that start with zh");
        assert!(
            candidates.contains(&"中".to_string()),
            "the most frequent character of the syllables that start with zh: {candidates:?}"
        );
    }

    #[test]
    fn the_romaji_gives_the_kana_of_it() {
        assert_eq!(romaji_to_kana("nihon"), "にほん");
        assert_eq!(romaji_to_kana("konnichiha"), "こんにちは");
        assert_eq!(romaji_to_kana("kitte"), "きって");
        assert_eq!(romaji_to_kana("kyou"), "きょう");
    }

    /// **Every** key of the table gives its kana.
    ///
    /// The table is data, and a typo in it is invisible to the tests that only type whole words: the red-green
    /// round showed that `a` could map to 「い」 and the whole suite would still be green (the user typed 「我」,
    /// took the first candidate and got `w1o`). So every key of the basic syllables is pinned here.
    #[test]
    fn every_key_of_the_table_gives_its_kana() {
        for (romaji, kana) in [
            ("a", "あ"),
            ("i", "い"),
            ("u", "う"),
            ("e", "え"),
            ("o", "お"),
            ("ka", "か"),
            ("ki", "き"),
            ("ku", "く"),
            ("ke", "け"),
            ("ko", "こ"),
            ("sa", "さ"),
            ("si", "し"),
            ("shi", "し"),
            ("su", "す"),
            ("se", "せ"),
            ("so", "そ"),
            ("ta", "た"),
            ("ti", "ち"),
            ("chi", "ち"),
            ("tu", "つ"),
            ("tsu", "つ"),
            ("te", "て"),
            ("to", "と"),
            ("na", "な"),
            ("ni", "に"),
            ("nu", "ぬ"),
            ("ne", "ね"),
            ("no", "の"),
            ("ha", "は"),
            ("hi", "ひ"),
            ("hu", "ふ"),
            ("fu", "ふ"),
            ("he", "へ"),
            ("ho", "ほ"),
            ("ma", "ま"),
            ("mi", "み"),
            ("mu", "む"),
            ("me", "め"),
            ("mo", "も"),
            ("ya", "や"),
            ("yu", "ゆ"),
            ("yo", "よ"),
            ("ra", "ら"),
            ("ri", "り"),
            ("ru", "る"),
            ("re", "れ"),
            ("ro", "ろ"),
            ("wa", "わ"),
            ("wo", "を"),
            ("n", "ん"),
            ("m", "ん"),
        ] {
            assert_eq!(romaji_to_kana(romaji), kana, "the romaji {romaji} gives {kana}");
        }
    }

    #[test]
    fn the_japanese_words_come_with_their_kanji() {
        let candidates = japanese_candidates("nihon");
        assert!(
            candidates.contains(&"日本".to_string()),
            "the word of the kana is a candidate: {candidates:?}"
        );
    }

    #[test]
    fn the_english_words_are_the_ones_that_start_with_what_was_typed() {
        let candidates = english_candidates("comp");
        assert!(candidates.contains(&"computer".to_string()), "{candidates:?}");
        assert!(
            candidates.iter().all(|word| word.starts_with("comp")),
            "every candidate starts with what was typed"
        );
    }

    #[test]
    fn nothing_typed_gives_nothing() {
        assert!(pinyin_candidates("").is_empty());
        assert!(japanese_candidates("").is_empty());
        assert!(english_candidates("").is_empty());
    }
}
