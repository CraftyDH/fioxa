extern crate alloc;
use alloc::vec::Vec;
use userspace::print;

pub use self::execute::Environment;
use self::parser::parse;
use self::tokenizer::tiny_tokenizer;
use crate::error::Result;

pub mod execute;
pub mod parser;
mod tokenizer;

#[cfg(test)]
mod tests;

pub fn execute<'a>(line: &str, env: &mut Environment<'a>) -> Result<()> {
    let tokens = tiny_tokenizer(Vec::new(), line, 0)?;
    let stmts = parse(tokens)?;
    execute::execute(stmts, env)?;

    Ok(())
}
