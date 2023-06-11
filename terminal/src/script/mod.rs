pub use self::execute::Environment;
use self::parser::parse;
use self::tokenizer::tokenize;
use crate::error::Result;

pub mod execute;
pub mod parser;
mod tokenizer;

#[cfg(test)]
mod tests;

pub fn execute<'a>(line: &str, env: &mut Environment<'a>) -> Result<()> {
    let tokens = tokenize(line)?;

    let stmts = parse(tokens)?;

    execute::execute(stmts, env)?;

    Ok(())
}
