extern crate alloc;

use core::fmt::Display;

use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use hashbrown::HashMap;
use kernel_userspace::{
    fs::{self, add_path, read_full_file, StatResponse},
    service::{generate_tracking_number, get_public_service_id, ServiceMessage},
    syscall::{send_and_get_response_service_message, CURRENT_PID},
};
use thiserror::Error;
use userspace::{print, println};

use super::parser::{Expr, Stmt};
use crate::error::Result;

pub fn execute<'a>(stmts: Vec<Stmt>, env: &mut Environment<'a>) -> Result<()> {
    for stmt in stmts {
        execute_stmt(stmt, env)?;
    }

    Ok(())
}

fn execute_stmt<'a>(stmt: Stmt, env: &mut Environment<'a>) -> Result<()> {
    match stmt {
        Stmt::Noop => Ok(()),
        Stmt::Execution(expr) => {
            execute_expr(&expr, env)?;
            Ok(())
        }
        Stmt::VarDec { id, expr } => {
            let val = execute_expr(&expr, env)?;
            env.set_var(id, val);
            Ok(())
        }
    }
}

fn execute_binary<'a>(path: String, pos_args: Vec<Expr>, env: &mut Environment<'a>) -> Result<()> {
    let fs_sid = get_public_service_id("FS").ok_or(ExecutionErrors::CouldNotFindFSSID)?;
    let elf_loader_sid =
        get_public_service_id("ELF_LOADER").ok_or(ExecutionErrors::CouldNotFindELFSID)?;

    let path = add_path(&env.cwd, &path);
    let stat = fs::stat(fs_sid, env.partition_id as usize, &path);

    let file = match stat {
        StatResponse::File(f) => f,
        StatResponse::Folder(_) => Err(ExecutionErrors::ExecNotAFile)?,
        StatResponse::NotFound => Err(ExecutionErrors::ExecCouldNotFind)?,
    };

    println!("READING...");
    let contents = read_full_file(fs_sid, env.partition_id as usize, file.node_id)
        .ok_or(ExecutionErrors::ReadError)?;

    println!("SPAWNING...");
    send_and_get_response_service_message(&ServiceMessage {
        service_id: elf_loader_sid,
        sender_pid: *CURRENT_PID,
        tracking_number: generate_tracking_number(),
        destination: kernel_userspace::service::SendServiceMessageDest::ToProvider,
        message: kernel_userspace::service::ServiceMessageType::ElfLoader(
            contents.get_data(),
            args_to_string(pos_args, env)?.as_bytes(),
        ),
    })?;

    Ok(())
}

pub fn args_to_string<'a>(pos_args: Vec<Expr>, env: &mut Environment<'a>) -> Result<String> {
    Ok(pos_args
        .iter()
        // TODO: Proper error handling / not unwrapping
        .map(|arg| execute_expr(arg, env).unwrap().to_string())
        .collect::<Vec<String>>()
        .join(" "))
}

pub fn execute_expr<'a>(expr: &Expr, env: &mut Environment<'a>) -> Result<Value> {
    Ok(match expr {
        Expr::String(str) => Value::String(str.clone()),
        Expr::Exec { path, pos_args } => {
            if path.starts_with("./") {
                execute_binary(path.to_string(), pos_args.clone(), env)?;
                return Ok(Value::Null);
            }

            if env.has_function(&path) {
                return env.call_function(&path, pos_args.clone());
            }

            Err(ExecutionErrors::UnresolvedCall(path.clone()))?
        }
        Expr::Var(key) => env.get_var(key),
    })
}

pub struct Environment<'a> {
    pub cwd: String,
    pub partition_id: u64,

    parent: Option<&'a mut Environment<'a>>,
    variables: HashMap<String, Value>,
    functions: HashMap<String, Value>,
}

impl<'a> Environment<'a> {
    pub fn new(cwd: String, partition_id: u64) -> Environment<'a> {
        Environment {
            cwd,
            partition_id,
            parent: None,
            variables: HashMap::new(),
            functions: HashMap::new(),
        }
    }

    pub fn with_parent(env: &'a mut Environment<'a>) -> Environment<'a> {
        Environment {
            cwd: env.cwd.clone(),
            partition_id: env.partition_id,
            parent: Some(env),
            variables: HashMap::new(),
            functions: HashMap::new(),
        }
    }

    pub fn has_function(&self, name: &str) -> bool {
        let mut found = self.functions.contains_key(name);

        if !found && let Some(parent) = &self.parent {
            found = parent.has_function(name);
        }

        found
    }

    pub fn get_function<'b>(&'b self, name: &str) -> Option<&'b Value> {
        let mut found = self.functions.get(name);

        if found.is_none() && let Some(parent) = &self.parent {
            found = parent.get_function(name);
        }

        found
    }

    pub fn call_function(&mut self, name: &str, args: Vec<Expr>) -> Result<Value> {
        let func = self
            .get_function(name)
            .ok_or(ExecutionErrors::CouldNotFindFunction(name.to_string()))?
            .clone();

        func.function_call(self, args)
    }

    pub fn add_internal_fn(&mut self, name: &str, func: InternalFunctionType) {
        self.functions
            .insert(name.to_string(), Value::InternalFunction(func));
    }

    pub fn get_var(&self, key: &str) -> Value {
        self.variables.get(key).map_or(Value::Null, |v| v.clone())
    }

    pub fn set_var(&mut self, key: String, val: Value) {
        self.variables.insert(key, val);
    }
}

type InternalFunctionType =
    &'static dyn for<'a> Fn(&mut Environment<'a>, Vec<Expr>) -> Result<Value>;

#[derive(Clone)]
pub enum Value {
    Null,
    String(String),
    InternalFunction(InternalFunctionType),
}

impl Value {
    pub fn function_call<'a>(&self, env: &mut Environment<'a>, args: Vec<Expr>) -> Result<Value> {
        if let Value::InternalFunction(func) = self {
            return func(env, args);
        }

        Err(ExecutionErrors::NotAFunction(self.to_string()))?
    }
}

impl Display for Value {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Value::Null => f.write_str("null"),
            Value::String(str) => f.write_str(str),
            Value::InternalFunction(_) => f.write_str("[InternalFunction]"),
        }
    }
}

#[derive(Debug, Error)]
pub enum ExecutionErrors {
    #[error("Could not resolve execution target '{0}'")]
    UnresolvedCall(String),

    #[error("Could not find fs service id")]
    CouldNotFindFSSID,

    #[error("Could not find elf service id")]
    CouldNotFindELFSID,

    #[error("Could not execute: found folder")]
    ExecNotAFile,

    #[error("Could not execute: file not found")]
    ExecCouldNotFind,

    #[error("Could not read file")]
    ReadError,

    #[error("Could not execute something that is not a function: {0}")]
    NotAFunction(String),

    #[error("Could not find function: {0}")]
    CouldNotFindFunction(String),
}
