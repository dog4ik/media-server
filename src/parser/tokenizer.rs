use crate::parser::symbol::Symbol;

pub const OPEN_BRACKETS: [char; 3] = ['(', '[', '{'];
pub const CLOSE_BRACKETS: [char; 3] = [')', ']', '}'];
pub const SEPARATORS: [char; 4] = ['-', '_', ' ', '.'];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token<'a> {
    /// Represents anything that is group or separator. It "may" contain show title
    Symbol(Symbol<'a>),
    GroupStart(char),
    /// Separator that have separators as neighbors
    ///
    /// For example in `Show - S02E3.mkv` `-` is explicit separator
    ExplicitSeparator,
    GroupEnd(char),
}

#[derive(Clone)]
pub struct Tokenizer<'a> {
    tokens: Vec<Token<'a>>,
    /// Index of the next unconsumed token
    pos: usize,
}

impl std::fmt::Debug for Tokenizer<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokenizer")
            .field("pos", &self.pos)
            .field("consumed", &&self.tokens[..self.pos])
            .field("remaining", &self.remaining())
            .finish()
    }
}

impl<'a> Tokenizer<'a> {
    pub fn new(file_name: &'a str) -> Self {
        let mut tokens = Vec::new();
        let mut token_start_byte = 0;

        let bytes = file_name.as_bytes();
        let mut iter = file_name.char_indices();
        while let Some((byte_idx, char)) = iter.next() {
            if OPEN_BRACKETS.contains(&char) {
                if byte_idx - token_start_byte != 0 {
                    let token = &file_name[token_start_byte..byte_idx];
                    tokens.push(Token::Symbol(Symbol(token)));
                }
                tokens.push(Token::GroupStart(char));
                token_start_byte = byte_idx + 1;
                continue;
            }
            if CLOSE_BRACKETS.contains(&char) {
                if byte_idx - token_start_byte != 0 {
                    let token = &file_name[token_start_byte..byte_idx];
                    tokens.push(Token::Symbol(Symbol(token)));
                }
                tokens.push(Token::GroupEnd(char));
                token_start_byte = byte_idx + 1;
                continue;
            }
            if SEPARATORS.contains(&char) {
                if byte_idx - token_start_byte != 0 {
                    let token = &file_name[token_start_byte..byte_idx];
                    tokens.push(Token::Symbol(Symbol(token)));
                }

                if let Some((prev, next)) = byte_idx
                    .checked_sub(1)
                    .and_then(|idx| bytes.get(idx))
                    .zip(bytes.get(byte_idx + 1))
                {
                    if *prev == b' ' && next == prev {
                        tokens.push(Token::ExplicitSeparator);
                        token_start_byte = byte_idx + 2;
                        iter.next();
                        continue;
                    }
                };
                token_start_byte = byte_idx + 1;
                continue;
            }
        }

        if token_start_byte != file_name.len() {
            let remainder = &file_name[token_start_byte..file_name.len()];
            tokens.push(Token::Symbol(Symbol(remainder)));
        }

        Self { tokens, pos: 0 }
    }

    /// Every token, regardless of how far the cursor has advanced
    pub fn tokens(&self) -> &[Token<'a>] {
        &self.tokens
    }

    /// Tokens from the cursor onwards
    pub fn remaining(&self) -> &[Token<'a>] {
        &self.tokens[self.pos..]
    }

    /// The next unconsumed token
    pub fn peek(&self) -> Option<Token<'a>> {
        self.peek_nth(0)
    }

    /// The token `n` places past the cursor, where `0` is [`Self::peek`]
    pub fn peek_nth(&self, n: usize) -> Option<Token<'a>> {
        self.tokens.get(self.pos + n).copied()
    }

    pub fn advance(&mut self) -> Option<Token<'a>> {
        let token = self.peek()?;
        self.pos += 1;
        Some(token)
    }

    /// Cursor position, to be handed back to [`Self::seek`]
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Move the cursor, saturating at the end of the stream
    pub fn seek(&mut self, pos: usize) {
        self.pos = pos.min(self.tokens.len());
    }

    /// Advance cursor by n positions
    pub fn advance_by(&mut self, n: usize) {
        self.seek(self.position() + n);
    }

    /// Rewind the cursor so the stream can be walked again
    pub fn rewind(&mut self) {
        self.pos = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokenize_test<'a>(tests: impl IntoIterator<Item = (&'a str, Vec<Token<'a>>)>) {
        for (test_input, expected) in tests {
            let tokenizer = Tokenizer::new(test_input);
            assert_eq!(
                expected.as_slice(),
                tokenizer.tokens(),
                "token mismatch for {test_input:?}"
            );
        }
    }

    #[test]
    pub fn tokenize_shows() {
        let tests = [
            (
                "Cyberpunk.Edgerunners.S01E02.DUBBED.1080p.WEBRip.x265-RARBG[eztv.re]",
                vec![
                    Token::Symbol(Symbol("Cyberpunk")),
                    Token::Symbol(Symbol("Edgerunners")),
                    Token::Symbol(Symbol("S01E02")),
                    Token::Symbol(Symbol("DUBBED")),
                    Token::Symbol(Symbol("1080p")),
                    Token::Symbol(Symbol("WEBRip")),
                    Token::Symbol(Symbol("x265")),
                    Token::Symbol(Symbol("RARBG")),
                    Token::GroupStart('['),
                    Token::Symbol(Symbol("eztv")),
                    Token::Symbol(Symbol("re")),
                    Token::GroupEnd(']'),
                ],
            ),
            (
                "shogun.2024.s01e05.2160p.web.h265-successfulcrab",
                vec![
                    Token::Symbol(Symbol("shogun")),
                    Token::Symbol(Symbol("2024")),
                    Token::Symbol(Symbol("s01e05")),
                    Token::Symbol(Symbol("2160p")),
                    Token::Symbol(Symbol("web")),
                    Token::Symbol(Symbol("h265")),
                    Token::Symbol(Symbol("successfulcrab")),
                ],
            ),
            (
                "Foo (2019).S04E03",
                vec![
                    Token::Symbol(Symbol("Foo")),
                    Token::GroupStart('('),
                    Token::Symbol(Symbol("2019")),
                    Token::GroupEnd(')'),
                    Token::Symbol(Symbol("S04E03")),
                ],
            ),
            (
                "Inception - 2010 - 1080p - BluRay - x264 - YIFY",
                vec![
                    Token::Symbol(Symbol("Inception")),
                    Token::ExplicitSeparator,
                    Token::Symbol(Symbol("2010")),
                    Token::ExplicitSeparator,
                    Token::Symbol(Symbol("1080p")),
                    Token::ExplicitSeparator,
                    Token::Symbol(Symbol("BluRay")),
                    Token::ExplicitSeparator,
                    Token::Symbol(Symbol("x264")),
                    Token::ExplicitSeparator,
                    Token::Symbol(Symbol("YIFY")),
                ],
            ),
        ];
        tokenize_test(tests);
    }

    #[test]
    fn tokenize_movies() {
        let tests = [(
            "Aladdin.WEB-DL.KP.1080p-SOFCJ",
            vec![
                Token::Symbol(Symbol("Aladdin")),
                Token::Symbol(Symbol("WEB")),
                Token::Symbol(Symbol("DL")),
                Token::Symbol(Symbol("KP")),
                Token::Symbol(Symbol("1080p")),
                Token::Symbol(Symbol("SOFCJ")),
            ],
        )];
        tokenize_test(tests);
    }
}
