extern crate alloc;

use alloc::{format, string::String, vec::Vec};
use thiserror::Error;
use userspace::{print, println};

use super::tokenizer::{
    Token,
    TokenKind::{self, *},
};
use crate::error::Result;

pub fn parse(tokens: Vec<Token>) -> Result<Vec<Stmt>> {
    let mut parser = Parser::new(tokens);
    let mut stmts = Vec::new();

    while !parser.is_at_end() {
        println!(
            "{} {} {:?} {}",
            parser.tokens.len(),
            parser.index,
            parser.tokens,
            parser.is_at_end()
        );

        stmts.push(parse_exec(&mut parser)?);
    }

    println!(
        "{} {} {:?} {}",
        parser.tokens.len(),
        parser.index,
        parser.tokens,
        parser.is_at_end()
    );

    Ok(stmts)
}

fn parse_exec(p: &mut Parser) -> Result<Stmt> {
    let path = parse_possible_strings(p)?;
    let pos_args = parse_positional_arguments(p)?;

    Ok(Stmt::Execution { path, pos_args })
}

fn parse_positional_arguments(p: &mut Parser) -> Result<Vec<Expr>> {
    let mut exprs = Vec::new();

    while !p.is_at_end() {
        let param = parse_possible_strings(p)?;
        exprs.push(Expr::String(param));
    }

    Ok(exprs)
}

fn parse_possible_strings(p: &mut Parser) -> Result<String> {
    Ok(match p.peek()? {
        Dot => {
            p.expect(Dot)?;
            format!(".{}", parse_possible_strings(p)?)
        }
        Slash => {
            p.expect(Slash)?;
            format!("/{}", parse_possible_strings(p)?)
        }
        Str(str) => {
            p.consume()?;
            str
        }
    })
}

pub struct Parser {
    pub index: usize,
    pub tokens: Vec<Token>,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Parser { index: 0, tokens }
    }

    pub fn is_at_end(&self) -> bool {
        self.index >= self.tokens.len()
    }

    fn ensure_current(&self) -> Result<()> {
        if self.is_at_end() {
            Err(ParserError::UnexpectedEOF)?
        }

        Ok(())
    }

    fn ensure_next(&self) -> Result<()> {
        if self.index + 1 > self.tokens.len() {
            Err(ParserError::UnexpectedEOF)?
        }

        Ok(())
    }

    pub fn peek(&self) -> Result<TokenKind> {
        self.ensure_current()?;
        Ok(self.tokens[self.index].kind.clone())
    }

    pub fn consume(&mut self) -> Result<TokenKind> {
        self.ensure_next()?;
        let current = self.peek()?;

        self.index += 1;
        Ok(current)
    }

    pub fn expect(&mut self, token: TokenKind) -> Result<TokenKind> {
        self.ensure_current()?;

        if self.tokens[self.index].kind != token {
            Err(ParserError::ExpectedToken(
                self.tokens[self.index].kind.clone(),
                token,
            ))?
        }

        let token = self.tokens[self.index].kind.clone();
        self.index += 1;

        Ok(token)
    }

    pub fn expect_id(&mut self) -> Result<String> {
        self.ensure_next()?;

        let id = match &self.tokens[self.index].kind {
            Str(val) => val.clone(),
            _ => Err(ParserError::ExpectedIdentifier(
                self.tokens[self.index].kind.clone(),
            ))?,
        };
        self.index += 1;

        Ok(id)
    }
}

#[derive(Debug)]
pub enum Stmt {
    Execution { path: String, pos_args: Vec<Expr> },
}

#[derive(Debug)]
pub enum Expr {
    String(String),
}

#[derive(Debug, Error)]
pub enum ParserError {
    #[error("Expected token '{0}', found '{0}'")]
    ExpectedToken(TokenKind, TokenKind),

    #[error("Expected identifier, found '{0}'")]
    ExpectedIdentifier(TokenKind),

    #[error("Unexpected end of input")]
    UnexpectedEOF,
}
