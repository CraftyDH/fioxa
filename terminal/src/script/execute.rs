extern crate alloc;

use core::fmt::Display;

use alloc::{
    borrow::ToOwned,
    string::{String, ToString},
    vec::Vec,
};
use hashbrown::HashMap;
use kernel_userspace::{
    elf::LoadElfError,
    fs::{self, add_path, read_full_file, StatResponse},
    ids::{ProcessID, ServiceID},
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
    let fs_sid = env.services.ok_or(ExecutionErrors::UninitedService)?.fs;
    let elf_loader_sid = env
        .services
        .ok_or(ExecutionErrors::UninitedService)?
        .elf_loader;

    let path = add_path(&env.cwd, &path);
    let stat = fs::stat(
        fs_sid,
        env.partition_id as usize,
        &path,
        env.services_buffer()?,
    )?;

    let file = match stat {
        StatResponse::File(ref f) => f.clone(),
        StatResponse::Folder(_) => Err(ExecutionErrors::ExecNotAFile)?,
    };

    drop(stat);

    println!("READING...");
    let contents = read_full_file(
        fs_sid,
        env.partition_id as usize,
        file.node_id,
        env.services_buffer()?,
    )?
    .ok_or(ExecutionErrors::ReadError)?
    .to_owned();

    println!("SPAWNING...");
    let args = args_to_string(pos_args, env)?;

    let _: ServiceMessage<Result<ProcessID, LoadElfError<'_>>> =
        send_and_get_response_service_message(
            &ServiceMessage {
                service_id: elf_loader_sid,
                sender_pid: *CURRENT_PID,
                tracking_number: generate_tracking_number(),
                destination: kernel_userspace::service::SendServiceMessageDest::ToProvider,
                message: (contents, args.as_bytes()),
            },
            env.services_buffer()?,
        )?;

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

    services_buffer_internal: Option<Vec<u8>>,
    pub services: Option<Services>,

    parent: Option<&'a mut Environment<'a>>,
    variables: HashMap<String, Value>,
    functions: HashMap<String, Value>,
}

impl<'a> Environment<'a> {
    pub fn new(cwd: String, partition_id: u64) -> Environment<'a> {
        Environment {
            cwd,
            partition_id,
            services_buffer_internal: Some(Vec::new()),
            services: None,
            parent: None,
            variables: HashMap::new(),
            functions: HashMap::new(),
        }
    }

    pub fn with_parent(env: &'a mut Environment<'a>) -> Environment<'a> {
        Environment {
            cwd: env.cwd.clone(),
            partition_id: env.partition_id,
            services_buffer_internal: None,
            services: env.services.clone(),
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

    pub fn add_service(&mut self, name: &str) -> Result<ServiceID> {
        Ok(get_public_service_id(name, self.services_buffer()?)
            .ok_or_else(|| ExecutionErrors::NoService(name.to_string()))?)
    }

    pub fn services_buffer<'b>(&'b mut self) -> Result<&'b mut Vec<u8>> {
        if let Some(services_buff) = &mut self.services_buffer_internal {
            Ok(services_buff)
        } else if let Some(parent) = &mut self.parent {
            parent.services_buffer()
        } else {
            Err(ExecutionErrors::NoParentServiceBuffer)?
        }
    }
}

#[derive(Clone, Copy)]
pub struct Services {
    pub fs: ServiceID,
    pub keyboard: ServiceID,
    pub elf_loader: ServiceID,
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

    #[error("Could obtain service '{0}'")]
    NoService(String),

    #[error("Service was not initialized")]
    UninitedService,

    #[error("The parent does not have a services buffer")]
    NoParentServiceBuffer,

    #[error("Could not find function: {0}")]
    CouldNotFindFunction(String),
}
