use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kernel_userspace::fs::{self, add_path, get_disks, read_file_sector, StatResponse};
use terminal::error::Result;
use terminal::script::execute::{args_to_string, execute_expr};
use terminal::script::{execute::Value, parser::Expr, Environment};
use thiserror::Error;
use userspace::{print, println};

pub fn pwd<'a>(env: &mut Environment<'a>, _args: Vec<Expr>) -> Result<Value> {
    println!("{}", env.cwd);
    Ok(Value::Null)
}

pub fn echo<'a>(env: &mut Environment<'a>, args: Vec<Expr>) -> Result<Value> {
    if args.len() == 0 {
        println!("ECHO!");
    } else {
        println!("{}", execute_expr(&args[0], env)?);
    }

    Ok(Value::Null)
}

pub fn disk<'a>(env: &mut Environment<'a>, args: Vec<Expr>) -> Result<Value> {
    if args.len() == 1 {
        if let Some(new_id) = execute_expr(&args[0], env)?
            .to_string()
            .chars()
            .next()
            .and_then(|c| c.to_digit(10))
        {
            env.partition_id = new_id as u64;
            return Ok(Value::Null);
        }
    }

    let fs_sid = env.services.ok_or(FuncExecError::UninitedService)?.fs;

    println!("Drives:");
    for part in get_disks(fs_sid, env.services_buffer()?)?.iter() {
        println!("{}:", part)
    }

    Ok(Value::Null)
}

pub fn ls<'a>(env: &mut Environment<'a>, args: Vec<Expr>) -> Result<Value> {
    let fs_sid = env.services.ok_or(FuncExecError::UninitedService)?.fs;

    let path = add_path(&env.cwd.clone(), &args_to_string(args, env)?);

    let stat = fs::stat(
        fs_sid,
        env.partition_id as usize,
        path.as_str(),
        env.services_buffer()?,
    )?;

    match stat {
        StatResponse::File(_) => println!("This is a file"),
        StatResponse::Folder(c) => {
            for child in c.children {
                println!("{child}")
            }
        }
    };

    Ok(Value::Null)
}

pub fn cd<'a>(env: &mut Environment<'a>, args: Vec<Expr>) -> Result<Value> {
    env.cwd = add_path(&env.cwd.clone(), &args_to_string(args, env)?);
    Ok(Value::Null)
}

pub fn cat<'a>(env: &mut Environment<'a>, args: Vec<Expr>) -> Result<Value> {
    let fs_sid = env.services.ok_or(FuncExecError::UninitedService)?.fs;

    for file in args {
        let file = execute_expr(&file, env)?.to_string();

        let path = add_path(&env.cwd, &file);

        let stat = fs::stat(
            fs_sid,
            env.partition_id as usize,
            path.as_str(),
            env.services_buffer()?,
        )?;

        let file = match stat {
            StatResponse::File(f) => f,
            StatResponse::Folder(_) => {
                println!("Not a file");
                continue;
            }
        };

        for i in 0..file.file_size / 512 {
            let sect = read_file_sector(
                fs_sid,
                env.partition_id as usize,
                file.node_id,
                i as u32,
                env.services_buffer()?,
            )?;
            if let Some(data) = sect {
                print!("{}", String::from_utf8_lossy(data))
            } else {
                print!("Error reading");
                break;
            }
        }
    }

    Ok(Value::Null)
}

#[derive(Debug, Clone, Error)]
pub enum FuncExecError {
    #[error("Service was not initialized")]
    UninitedService,
}
