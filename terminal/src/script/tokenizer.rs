extern crate alloc;

use core::fmt::{Display, Write};

use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use thiserror::Error;

use crate::error::{Context, Result};

pub fn tiny_tokenizer(mut tokens: Vec<Token>, src: &str, start: usize) -> Result<Vec<Token>> {
    if src.is_empty() {
        return Ok(tokens);
    }

    let next = src.chars().next().ok_or(LexerError::UnexpectedEOF)?;

    let (token, size) = match next {
        '.' => (TokenKind::Dot, 1),
        '/' => (TokenKind::Slash, 1),
        c if c.is_alphanumeric() || c == '-' || c == '.' => {
            tokenize_ident(src).with_context(|| LexerError::IdentifierFailed)?
        }
        _ => Err(LexerError::UnknownChar(next))?,
    };

    let end = start + size;
    tokens.push(Token::new(token, start, end));

    Ok(tiny_tokenizer(tokens, &src[size..], end)?)
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

#[derive(Error, Debug)]
pub enum LexerError {
    #[error("Unexpected end of file")]
    UnexpectedEOF,

    #[error("Failed to parse identifier")]
    IdentifierFailed,

    #[error("No matches")]
    NoMatches,

    #[error("Unknown chars '{0}'")]
    UnknownChar(char),
}
