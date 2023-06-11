extern crate alloc;

use core::fmt::{Display, Write};

use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use thiserror::Error;

use crate::error::{Context, Result};

struct Tokenizer<'a> {
    current_index: usize,
    remaining_text: &'a str,
}

impl<'a> Tokenizer<'a> {
    fn new(src: &str) -> Tokenizer {
        Tokenizer {
            current_index: 0,
            remaining_text: src,
        }
    }

    fn next_token(&mut self) -> Result<Option<Token>> {
        self.skip_whitespace();

        if self.remaining_text.is_empty() {
            Ok(None)
        } else {
            let start = self.current_index;
            let tok = self
                ._next_token()
                .with_context(|| LexerError::ReadFailed(self.current_index))?;
            let end = self.current_index;
            Ok(Some(Token::new(tok, start, end)))
        }
    }

    fn skip_whitespace(&mut self) {
        let skipped = skip(self.remaining_text);
        self.chomp(skipped);
    }

    fn _next_token(&mut self) -> Result<TokenKind> {
        let (tok, bytes_read) = tokenize_single_token(self.remaining_text)?;
        self.chomp(bytes_read);

        Ok(tok)
    }

    fn chomp(&mut self, num_bytes: usize) {
        self.remaining_text = &self.remaining_text[num_bytes..];
        self.current_index += num_bytes;
    }
}

pub fn tokenize(src: &str) -> Result<Vec<Token>> {
    let mut tokenizer = Tokenizer::new(src);
    let mut tokens = Vec::new();

    while let Some(tok) = tokenizer.next_token()? {
        tokens.push(tok);
    }

    Ok(tokens)
}

fn skip_whitespace(data: &str) -> usize {
    match take_while(data, |ch| ch.is_whitespace()) {
        Ok((_, bytes_skipped)) => bytes_skipped,
        _ => 0,
    }
}

/// Skip past any whitespace characters or comments.
fn skip(src: &str) -> usize {
    let mut remaining = src;

    loop {
        let ws = skip_whitespace(remaining);
        remaining = &remaining[ws..];
        // let comments = skip_comments(remaining);
        // remaining = &remaining[comments..];

        if ws == 0 {
            return src.len() - remaining.len();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    Dot,
    Slash,
    Str(String),
}

impl Display for TokenKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        use TokenKind::*;

        match self {
            Dot => f.write_char('.'),
            Slash => f.write_char('/'),
            Str(str) => {
                f.write_char('"')?;
                f.write_str(str)?;
                f.write_char('"')
            }
        }
    }
}

impl From<String> for TokenKind {
    fn from(value: String) -> Self {
        TokenKind::Str(value)
    }
}

impl<'a> From<&'a str> for TokenKind {
    fn from(value: &'a str) -> Self {
        TokenKind::Str(value.to_string())
    }
}

#[derive(Debug)]
pub struct Token {
    pub kind: TokenKind,
    /// The location inside of the initial tokenizing string
    pub start: usize,
    pub end: usize,
}

impl Token {
    pub fn new(kind: TokenKind, start: usize, end: usize) -> Self {
        Token { kind, start, end }
    }
}

type TokResult<T> = Result<T>;

pub fn tokenize_single_token(data: &str) -> TokResult<(TokenKind, usize)> {
    let next = match data.chars().next() {
        Some(c) => c,
        None => Err(LexerError::UnexpectedEOF)?,
    };

    Ok(match next {
        '.' => (TokenKind::Dot, 1),
        '/' => (TokenKind::Slash, 1),
        c if c.is_alphanumeric() || c == '-' || c == '.' => {
            tokenize_ident(data).with_context(|| LexerError::IdentifierFailed)?
        }
        _ => Err(LexerError::UnknownChar(next))?,
    })
}

fn tokenize_ident(data: &str) -> TokResult<(TokenKind, usize)> {
    // TODO: Reintroduce this once I have number types
    // match data.chars().next() {
    //     Some(ch) if ch.is_digit(10) => Err(LexerError::StartWithNum)?,
    //     None => Err(LexerError::UnexpectedEOF)?,
    //     _ => {}
    // }

    let (got, bytes_read) = take_while(data, |ch| ch == '-' || ch.is_alphanumeric() || ch == '.')?;

    let tok = TokenKind::Str(got.to_string());
    Ok((tok, bytes_read))
}

fn take_while<F>(data: &str, mut pred: F) -> TokResult<(&str, usize)>
where
    F: FnMut(char) -> bool,
{
    let mut current_index = 0;

    for ch in data.chars() {
        let should_continue = pred(ch);

        if !should_continue {
            break;
        }

        current_index += ch.len_utf8();
    }

    if current_index == 0 {
        Err(LexerError::NoMatches)?
    } else {
        Ok((&data[..current_index], current_index))
    }
}

#[derive(Error, Debug)]
pub enum LexerError {
    #[error("Identifiers can't start with a number")]
    StartWithNum,

    #[error("Unexpected end of file")]
    UnexpectedEOF,

    #[error("Failed to parse identifier")]
    IdentifierFailed,

    #[error("No matches")]
    NoMatches,

    #[error("Unknown chars '{0}'")]
    UnknownChar(char),

    #[error("{0}: Could not read the next token")]
    ReadFailed(usize),
}
