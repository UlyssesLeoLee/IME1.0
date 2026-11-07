//! The vertical writing mode: a text that is written as a column instead of as a line.
//!
//! The user asked for it: "书写自然语言时写好之后，可以自动把文字纵向排成一列，并且确保前列内容不受干扰，
//! tab或者空格也能自动保留成和前列一样的形式，只是单纯输入一列纵向文字排在前列后方", and then the shape of an
//! English sentence: "英文的话以单词为单位横向书写，但是不同单词之间是纵向，比如输入 Who I'm I 就把它纵向排列成
//! Who / I / 'm / I".
//!
//! So a column is a stack of **tokens**, one token per line:
//!
//! * a **word** of the Latin script stays on one line (it is written horizontally, the way a word is read);
//! * a **character** of a script that is written vertically (Chinese, Japanese, Korean) stands on a line of its own;
//! * a **run of whitespace** is a line of its own, and it is written the way the **previous column** wrote it (a tab
//!   stays a tab, a space stays a space, an ideographic space stays an ideographic space).
//!
//! Nothing here touches the document: the writer only says what the text of the next column is. The column is written
//! where the caret is, which is **after** the previous column, and the previous column is left exactly as it was.

/// How a whitespace is written in a column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Whitespace {
    /// A tab character.
    Tab,
    /// A space character.
    Space,
    /// An ideographic space (U+3000): what a space looks like in a column of a text that is written vertically.
    FullWidthSpace,
    /// A tab written as an ideographic space.
    FullWidthTab,
}

impl Whitespace {
    /// The character that stands for it.
    fn character(self) -> char {
        match self {
            Whitespace::Tab => '\t',
            Whitespace::Space => ' ',
            Whitespace::FullWidthSpace | Whitespace::FullWidthTab => '\u{3000}',
        }
    }
}

/// What a column looks like: how it writes the whitespace.
///
/// The profile of a column is learned from the column before it, so that a tab in the new column looks like the tab
/// of the old one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColumnProfile {
    /// How a tab is written.
    pub tab: Whitespace,
    /// How a space is written.
    pub space: Whitespace,
}

impl Default for ColumnProfile {
    fn default() -> Self {
        ColumnProfile {
            tab: Whitespace::Tab,
            space: Whitespace::Space,
        }
    }
}

impl ColumnProfile {
    /// The profile of a column that has been written: the whitespace it used tells how the next one writes it.
    ///
    /// A column that has no whitespace at all says nothing: the default profile is kept.
    pub fn learn(column: &str) -> ColumnProfile {
        let mut profile = ColumnProfile::default();
        let mut seen_tab = false;
        let mut seen_space = false;
        let mut seen_full = false;
        for ch in column.chars() {
            match ch {
                '\t' => seen_tab = true,
                ' ' => seen_space = true,
                '\u{3000}' => seen_full = true,
                _ => {}
            }
        }
        if seen_full {
            // the column writes the ideographic space: a tab in it is the ideographic space as well
            profile.space = Whitespace::FullWidthSpace;
            profile.tab = Whitespace::FullWidthTab;
        } else {
            if seen_space {
                profile.space = Whitespace::Space;
            }
            if seen_tab {
                profile.tab = Whitespace::Tab;
            }
        }
        profile
    }

    /// How a run of whitespace is written in this column.
    fn write(&self, raw: &str) -> String {
        let mut out = String::new();
        for ch in raw.chars() {
            let form = match ch {
                '\t' => self.tab,
                _ => self.space,
            };
            out.push(form.character());
        }
        out
    }
}

/// One piece of the text that stands on a line of its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token {
    /// A word of the Latin script: it is written horizontally, on one line.
    Word(String),
    /// A character of a script that is written vertically: it stands on a line of its own.
    Character(String),
    /// A run of whitespace.
    Space(String),
}

/// Whether the character is written vertically (one character on a line of its own).
fn is_vertical_character(ch: char) -> bool {
    let code = ch as u32;
    // Han (and the extension A), the kana, the Hangul, the CJK punctuation, the full width forms
    (0x3400..=0x4DBF).contains(&code)
        || (0x4E00..=0x9FFF).contains(&code)
        || (0x3000..=0x303F).contains(&code)
        || (0x3040..=0x30FF).contains(&code)
        || (0x31F0..=0x31FF).contains(&code)
        || (0xAC00..=0xD7AF).contains(&code)
        || (0xFF01..=0xFF60).contains(&code)
        || (0xFFE0..=0xFFE6).contains(&code)
}

/// Whether the character is a quote that begins a new word ("I'm" is the word "I" and the word "'m").
fn is_quote(ch: char) -> bool {
    matches!(ch, '\'' | '\u{2019}' | '`' | '\u{2018}' | '\u{201C}' | '\u{201D}')
}

/// Whether the character belongs to a word (letters, digits and the marks inside a word).
fn is_word_character(ch: char) -> bool {
    !ch.is_whitespace() && !is_vertical_character(ch) && !is_quote(ch)
}

/// The pieces of the text, one per line of the column.
///
/// * a word of the Latin script is one piece (it stays horizontal);
/// * a character of a vertical script is one piece (it stands on a line of its own);
/// * a quote begins a new piece ("I'm" is "I" and "'m");
/// * a run of whitespace is one piece.
pub fn tokenize(text: &str) -> Vec<Token> {
    let mut tokens: Vec<Token> = Vec::new();
    let mut word = String::new();
    let mut space = String::new();
    for ch in text.chars() {
        if ch == '\n' {
            // the line break of the text is the line break of the column
            if !word.is_empty() {
                tokens.push(Token::Word(std::mem::take(&mut word)));
            }
            if !space.is_empty() {
                tokens.push(Token::Space(std::mem::take(&mut space)));
            }
            continue;
        }
        if ch.is_whitespace() {
            if !word.is_empty() {
                tokens.push(Token::Word(std::mem::take(&mut word)));
            }
            space.push(ch);
            continue;
        }
        if !space.is_empty() {
            tokens.push(Token::Space(std::mem::take(&mut space)));
        }
        if is_vertical_character(ch) {
            if !word.is_empty() {
                tokens.push(Token::Word(std::mem::take(&mut word)));
            }
            tokens.push(Token::Character(ch.to_string()));
            continue;
        }
        if is_quote(ch) {
            if !word.is_empty() {
                tokens.push(Token::Word(std::mem::take(&mut word)));
            }
            // the quote begins the next word
            word.push(ch);
            continue;
        }
        if is_word_character(ch) {
            word.push(ch);
            continue;
        }
        // anything else (a symbol that is not part of a word) ends the word
        if !word.is_empty() {
            tokens.push(Token::Word(std::mem::take(&mut word)));
        }
        tokens.push(Token::Character(ch.to_string()));
    }
    if !word.is_empty() {
        tokens.push(Token::Word(word));
    }
    if !space.is_empty() {
        tokens.push(Token::Space(space));
    }
    tokens
}

/// The writer of the columns.
///
/// It remembers how the last column wrote its whitespace, so that the next column looks the same.
#[derive(Clone, Debug, Default)]
pub struct VerticalWriter {
    profile: ColumnProfile,
    /// Whether a column has been written already (the next one starts on a line of its own).
    written: bool,
}

impl VerticalWriter {
    /// A writer that has not written a column yet.
    pub fn new() -> VerticalWriter {
        VerticalWriter {
            profile: ColumnProfile::default(),
            written: false,
        }
    }

    /// How the columns are written at the moment.
    pub fn profile(&self) -> ColumnProfile {
        self.profile
    }

    /// Write the whitespace in another way (the user changed the setting).
    pub fn set_profile(&mut self, profile: ColumnProfile) {
        self.profile = profile;
    }

    /// The text of the column for a committed text.
    ///
    /// The column is a stack of the pieces of the text, one per line. The whitespace is written the way the previous
    /// column wrote it, and the writer learns from the column it has just made.
    ///
    /// The first column starts where the caret is; every column after it starts on a **line of its own**, so that it
    /// stands behind the column before it and that column is not touched.
    pub fn column(&mut self, text: &str) -> String {
        let mut out = String::new();
        for (index, token) in tokenize(text).into_iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            match token {
                Token::Word(word) | Token::Character(word) => out.push_str(&word),
                Token::Space(raw) => out.push_str(&self.profile.write(&raw)),
            }
        }
        if self.written && !out.is_empty() {
            out.insert(0, '\n');
        }
        self.written = true;
        self.profile = ColumnProfile::learn(&out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The example of the user: "Who I'm I" stands as Who / I / 'm / I, with the spaces of the text on lines of
    /// their own (a line with a space is a line with a space: it is how a space looks in a column).
    #[test]
    fn an_english_sentence_is_a_stack_of_words() {
        let mut writer = VerticalWriter::new();
        let column = writer.column("Who I'm I");
        assert_eq!(column, "Who\n \nI\n'm\n \nI");
    }

    /// Chinese is written one character on a line of its own.
    #[test]
    fn a_chinese_text_is_a_column_of_characters() {
        let mut writer = VerticalWriter::new();
        let column = writer.column("你好世界");
        assert_eq!(column, "你\n好\n世\n界");
    }

    /// A word is not broken in the middle.
    #[test]
    fn a_word_stays_on_one_line() {
        let mut writer = VerticalWriter::new();
        let column = writer.column("programming");
        assert_eq!(column, "programming");
    }

    /// The whitespace of the column is written the way the previous column wrote it.
    #[test]
    fn the_whitespace_keeps_the_form_of_the_previous_column() {
        let mut writer = VerticalWriter::new();
        // the first column writes a tab and a space
        let first = writer.column("a\tb c");
        assert_eq!(first, "a\n\t\nb\n \nc");
        // the next column writes them the same way (and it starts on a line of its own)
        let second = writer.column("x\ty z");
        assert_eq!(second, "\nx\n\t\ny\n \nz");
        assert_eq!(writer.profile().tab, Whitespace::Tab);
        assert_eq!(writer.profile().space, Whitespace::Space);
    }

    /// When the previous column wrote the ideographic space, the next one does the same.
    #[test]
    fn the_ideographic_space_is_kept() {
        let mut writer = VerticalWriter::new();
        writer.set_profile(ColumnProfile {
            tab: Whitespace::FullWidthTab,
            space: Whitespace::FullWidthSpace,
        });
        let column = writer.column("a b");
        assert_eq!(column, "a\n\u{3000}\nb");
        // and the writer learned it
        let next = writer.column("c d");
        assert_eq!(next, "\nc\n\u{3000}\nd");
    }

    /// The profile is learned from the column that was written.
    #[test]
    fn the_profile_is_learned_from_the_column() {
        let profile = ColumnProfile::learn("a\n\u{3000}\nb");
        assert_eq!(profile.space, Whitespace::FullWidthSpace);
        assert_eq!(profile.tab, Whitespace::FullWidthTab);
        let plain = ColumnProfile::learn("a\n\t\nb");
        assert_eq!(plain.tab, Whitespace::Tab);
        assert_eq!(plain.space, Whitespace::Space);
        // a column without whitespace says nothing
        let nothing = ColumnProfile::learn("a\nb");
        assert_eq!(nothing, ColumnProfile::default());
    }

    /// The pieces of a text: a word, a word, a quote and the word of the quote.
    #[test]
    fn the_pieces_of_a_text() {
        assert_eq!(
            tokenize("Who I'm I"),
            vec![
                Token::Word("Who".to_string()),
                Token::Space(" ".to_string()),
                Token::Word("I".to_string()),
                Token::Word("'m".to_string()),
                Token::Space(" ".to_string()),
                Token::Word("I".to_string()),
            ]
        );
    }

    /// A text that is empty stays empty: nothing is written.
    #[test]
    fn an_empty_text_writes_nothing() {
        let mut writer = VerticalWriter::new();
        assert_eq!(writer.column(""), "");
    }

    /// A digit belongs to the word it stands in.
    #[test]
    fn a_number_is_a_word() {
        let mut writer = VerticalWriter::new();
        assert_eq!(writer.column("abc 123"), "abc\n \n123");
    }

    /// The second column stands behind the first one: it starts on a line of its own, and the first column is not
    /// touched.
    #[test]
    fn the_second_column_starts_on_a_line_of_its_own() {
        let mut writer = VerticalWriter::new();
        let first = writer.column("你好");
        assert_eq!(first, "你\n好");
        let second = writer.column("世界");
        assert_eq!(second, "\n世\n界");
    }
}
