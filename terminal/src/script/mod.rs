use userspace::{print, println};

use self::parser::parse;
use self::tokenizer::tokenize;
use crate::error::Result;

mod execute;
mod parser;
mod tokenizer;

#[cfg(test)]
mod tests;

pub fn execute(line: &str) -> Result<()> {
    let tokens = tokenize(line)?;
    println!("{:?}", tokens);

    let stmts = parse(tokens)?;
    println!("{:?}", stmts);

    Ok(())
}
