extern crate alloc;

use core::fmt::{Display, Write};
use userspace::{print, println};

use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use thiserror::Error;

use crate::error::{Context, Result};

pub fn tiny_tokenizer(mut tokens: Vec<Token>, src: &str, start: usize) -> Result<Vec<Token>> {
    let next = src.chars().next();
    if let None = next {
        return Ok(tokens);
    }

    let next = next.ok_or(LexerError::UnexpectedEOF)?;
    if next == ' ' {
        return tiny_tokenizer(tokens, &src[1..], start + 1);
    }

    let (token, size) = match next {
        '.' => (TokenKind::Dot, 1),
        '/' => (TokenKind::Slash, 1),
        '=' => (TokenKind::Eq, 1),
        '\n' => (TokenKind::StmtEnd, 1),
        ';' => (TokenKind::StmtEnd, 1),
        '$' => {
            let (str, length) =
                tokenize_str(&src[1..]).with_context(|| LexerError::IdentifierFailed)?;
            (TokenKind::Var(str), length + 1)
        }
        c if c.is_alphanumeric() || c == '-' || c == '.' => {
            let (str, length) = tokenize_str(src).with_context(|| LexerError::IdentifierFailed)?;
            (TokenKind::Str(str), length)
        }
        _ => Err(LexerError::UnknownChar(start + 1, next))?,
    };

    let end = start + size;
    tokens.push(Token::new(token, start, end));

    tiny_tokenizer(tokens, &src[size..], end)
}

fn tokenize_str(data: &str) -> TokResult<(String, usize)> {
    // TODO: Reintroduce this once I have number types
    // match data.chars().next() {
    //     Some(ch) if ch.is_digit(10) => Err(LexerError::StartWithNum)?,
    //     None => Err(LexerError::UnexpectedEOF)?,
    //     _ => {}
    // }

    let (got, bytes_read) = take_while(data, |ch| ch == '-' || ch.is_alphanumeric() || ch == '.')?;
    Ok((got.to_string(), bytes_read))
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
    Eq,
    Dot,
    Slash,
    StmtEnd,
    Str(String),
    Var(String),
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
            Eq => f.write_char('='),
            Var(str) => {
                f.write_char('$')?;
                f.write_str(str)
            }
            StmtEnd => f.write_str("\\n"),
        }
    }
}

#[derive(Debug, Clone)]
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

    #[error("{0}: Unknown chars '{1}'")]
    UnknownChar(usize, char),
}
