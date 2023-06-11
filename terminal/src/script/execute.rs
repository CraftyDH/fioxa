extern crate alloc;

use alloc::vec::Vec;

use super::parser::Stmt;
use crate::error::Result;

pub fn execute<'a>(stmts: Vec<Stmt>, env: &mut Environment<'a>) -> Result<()> {
    for stmt in stmts {
        execute_single(stmt, env)?;
    }

    Ok(())
}

fn execute_single<'a>(stmt: Stmt, env: &mut Environment<'a>) -> Result<()> {
    match stmt {
        Stmt::Execution { path, pos_args } => todo!(),
    }

    Ok(())
}

pub struct Environment<'a> {
    parent: Option<&'a mut Environment<'a>>,
}

impl<'a> Environment<'a> {
    pub fn new() -> Environment<'a> {
        Environment { parent: None }
    }

    pub fn with_parent(env: &'a mut Environment<'a>) -> Environment<'a> {
        Environment { parent: Some(env) }
    }
}
