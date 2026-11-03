//! The formula source as TeX tokens: control words (`\frac`), control symbols (`\,`, `\{`),
//! characters, and runs of white space. A `%` comment runs to the end of its line and is
//! skipped. The parser also reads characters one by one (in `\text{…}`), through
//! [`Lexer::peek_char`] and [`Lexer::bump`].

/// What a token is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tok<'a> {
    /// `\name`, the name without its backslash: ASCII letters only.
    Word(&'a str),
    /// `\` and one other character.
    Sym(char),
    Char(char),
    /// White space (comments included), any amount.
    Space,
    /// A `\` that ends the source.
    Bad,
    Eof,
}

/// A token and where it is in the source, in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token<'a> {
    pub tok: Tok<'a>,
    pub start: usize,
    pub end: usize,
}

/// The source, read from `pos` on. It is `Copy`, so a copy can look ahead.
#[derive(Clone, Copy, Debug)]
pub struct Lexer<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Lexer<'a> {
        Lexer { src, pos: 0 }
    }

    /// The whole source.
    pub fn source(&self) -> &'a str {
        self.src
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    /// The next character, as it is (no tokens, no comments).
    pub fn peek_char(&self) -> Option<char> {
        self.src.get(self.pos..)?.chars().next()
    }

    /// Steps over the next character.
    pub fn bump(&mut self) {
        if let Some(c) = self.peek_char() {
            self.pos += c.len_utf8();
        }
    }

    /// The next token, without reading it.
    pub fn peek(&self) -> Token<'a> {
        let mut copy = *self;
        copy.token()
    }

    /// The next token that isn't white space, without reading it.
    pub fn peek_solid(&self) -> Token<'a> {
        let mut copy = *self;
        copy.skip_space();
        copy.token()
    }

    pub fn skip_space(&mut self) {
        while self.peek().tok == Tok::Space {
            self.token();
        }
    }

    pub fn token(&mut self) -> Token<'a> {
        let start = self.pos;
        let tok = match self.peek_char() {
            None => Tok::Eof,
            Some(c) if c.is_whitespace() || c == '%' => {
                loop {
                    match self.peek_char() {
                        Some('%') => {
                            while self.peek_char().is_some_and(|c| c != '\n') {
                                self.bump();
                            }
                        }
                        Some(c) if c.is_whitespace() => self.bump(),
                        _ => break,
                    }
                }
                Tok::Space
            }
            Some('\\') => {
                self.bump();
                let rest = self.src.get(self.pos..).unwrap_or("");
                let n = rest.bytes().take_while(u8::is_ascii_alphabetic).count();
                if n > 0 {
                    self.pos += n;
                    Tok::Word(rest.get(..n).unwrap_or(""))
                } else if let Some(c) = self.peek_char() {
                    self.bump();
                    Tok::Sym(c)
                } else {
                    Tok::Bad
                }
            }
            Some(c) => {
                self.bump();
                Tok::Char(c)
            }
        };
        Token {
            tok,
            start,
            end: self.pos,
        }
    }
}
