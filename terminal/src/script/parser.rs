extern crate alloc;

use alloc::{format, string::String, vec::Vec};
use thiserror::Error;

use super::tokenizer::{
    Token,
    TokenKind::{self, *},
};
use crate::error::{Context, Result};

pub fn parse(tokens: Vec<Token>) -> Result<Vec<Stmt>> {
    let mut parser = Parser::new(tokens);
    let mut stmts = Vec::new();

    while !parser.is_at_end() {
        let stmt = match parser.peek().context("Inside parse")? {
            Eq => Err(ParserError::UnexpectedToken(
                parser.peek().context("Inside parse Eq")?,
            ))?,
            Dot | Slash | Str(_) => parse_exec(&mut parser)?,
            Var(_) => parse_var(&mut parser)?,
            StmtEnd => {
                parser.consume()?;
                Stmt::Noop
            }
        };

        stmts.push(stmt);
    }

    Ok(stmts)
}

fn parse_var(p: &mut Parser) -> Result<Stmt> {
    let id = p.expect_var()?;
    p.expect(Eq)?;
    let expr = parse_expr(p)?;
    p.expect(StmtEnd)?;

    Ok(Stmt::VarDec { id, expr })
}

fn parse_exec(p: &mut Parser) -> Result<Stmt> {
    let path = parse_possible_strings(p)?;
    let mut pos_args = Vec::new();

    while !p.is_at_end()
        && match p.peek().context("Inside parse_exec")? {
            Eq | StmtEnd => false,
            _ => true,
        }
    {
        pos_args.push(parse_expr(p)?);
    }

    Ok(Stmt::Execution(Expr::Exec { path, pos_args }))
}

fn parse_expr(p: &mut Parser) -> Result<Expr> {
    Ok(match p.peek().context("Inside parse_expr")? {
        Dot | Slash | Str(_) => Expr::String(parse_possible_strings(p)?),
        Var(name) => {
            p.consume()?;
            Expr::Var(name)
        }
        Eq | StmtEnd => Err(ParserError::UnexpectedToken(
            p.peek().context("Inside parse_expr::Eq|StmtEnd")?,
        ))?,
    })
}

fn parse_possible_strings(p: &mut Parser) -> Result<String> {
    if p.is_at_end() {
        return Ok(String::new());
    }

    Ok(match p.peek().context("Inside parse_possible_strings")? {
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
        Eq | Var(_) | StmtEnd => Err(ParserError::UnexpectedToken(
            p.peek()
                .context("Inside parse_possible_strings::Eq|Var|StmtEnd")?,
        ))?,
    })
}

#[derive(Debug)]
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

    pub fn is(&self, tokens: &[TokenKind]) -> Result<bool> {
        let peek = self.peek().context("Inside is")?;

        for token in tokens {
            if peek == *token {
                return Ok(true);
            }
        }

        Ok(false)
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
        self.ensure_current().context("Inside peek")?;
        Ok(self.tokens[self.index].kind.clone())
    }

    pub fn consume(&mut self) -> Result<TokenKind> {
        self.ensure_next().context("Inside consume")?;
        let current = self.peek().context("Inside consume")?;

        self.index += 1;
        Ok(current)
    }

    pub fn expect(&mut self, token: TokenKind) -> Result<TokenKind> {
        self.ensure_current()
            .with_context(|| format!("Expected: {}", token))?;

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
        self.ensure_next().context("Inside expect_id")?;

        let id = match &self.tokens[self.index].kind {
            Str(val) => val.clone(),
            _ => Err(ParserError::ExpectedIdentifier(
                self.tokens[self.index].kind.clone(),
            ))?,
        };
        self.index += 1;

        Ok(id)
    }

    pub fn expect_var(&mut self) -> Result<String> {
        self.ensure_next().context("Inside expect_var")?;

        let id = match &self.tokens[self.index].kind {
            Var(val) => val.clone(),
            _ => Err(ParserError::ExpectedIdentifier(
                self.tokens[self.index].kind.clone(),
            ))?,
        };
        self.index += 1;

        Ok(id)
    }
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Noop,
    VarDec { id: String, expr: Expr },
    Execution(Expr),
}

#[derive(Debug, Clone)]
pub enum Expr {
    Exec { path: String, pos_args: Vec<Expr> },
    Var(String),
    String(String),
}

#[derive(Debug, Error)]
pub enum ParserError {
    #[error("Unexpected token '{0}'")]
    UnexpectedToken(TokenKind),

    #[error("Expected token '{0}', found '{1}'")]
    ExpectedToken(TokenKind, TokenKind),

    #[error("Expected identifier, found '{0}'")]
    ExpectedIdentifier(TokenKind),

    #[error("Unexpected end of input")]
    UnexpectedEOF,
}
