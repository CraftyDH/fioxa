#![no_std]
#![no_main]

use alloc::{format, string::String};
use kernel_userspace::sys::syscall::sys_read_args_string;
use userspace::print::STDIN_CHANNEL;

extern crate alloc;
#[macro_use]
extern crate userspace;
extern crate userspace_slaballoc;

init_userspace!(main);

fn solve(line: &str) -> Result<isize, String> {
    let mut res: isize = 0;
    let mut op = Operators::Emit;

    let mut curr_val: isize = 0;

    for c in line.chars() {
        if c == ' ' {
            continue;
        }

        if let Some(digit) = c.to_digit(10) {
            curr_val = curr_val.checked_mul(10).ok_or("overflow")? + digit as isize;
        } else {
            match op {
                Operators::Emit => res = curr_val,
                Operators::Add => res = res.checked_add(curr_val).ok_or("overflow")?,
                Operators::Sub => res = res.checked_sub(curr_val).ok_or("overflow")?,
                Operators::Mul => res = res.checked_mul(curr_val).ok_or("overflow")?,
                Operators::Div => res = res.checked_div(curr_val).ok_or("div by zero")?,
            }
            curr_val = 0;
            op = match c {
                '+' => Operators::Add,
                '-' => Operators::Sub,
                '*' => Operators::Mul,
                '/' => Operators::Div,
                _ => return Err(format!("unknown op {c:?}")),
            }
        }
    }

    match op {
        Operators::Emit => res = curr_val,
        Operators::Add => res += curr_val,
        Operators::Sub => res -= curr_val,
        Operators::Mul => res *= curr_val,
        Operators::Div => res = res.checked_div(curr_val).ok_or("div by zero")?,
    }

    Ok(res)
}

struct InputLines<'a> {
    read_buf: String,
    return_buf: &'a mut String,
}

impl<'a> Iterator for InputLines<'a> {
    type Item = ();

    fn next(&mut self) -> Option<Self::Item> {
        self.return_buf.clear();
        loop {
            let range = self
                .read_buf
                .find('\n')
                .map(|n| ..n + 1)
                .unwrap_or_else(|| ..self.read_buf.len());

            for c in self.read_buf.drain(range) {
                if c == '\n' {
                    return Some(());
                } else if c == '\x08' {
                    if self.return_buf.pop().is_some() {
                        print!("\x08")
                    }
                } else {
                    self.return_buf.push(c);
                    print!("{c}")
                }
            }

            unsafe {
                STDIN_CHANNEL
                    .read::<0>(self.read_buf.as_mut_vec(), true, true)
                    .unwrap()
            };
        }
    }
}

pub fn main() {
    let args = sys_read_args_string();

    eprintln!("WARN: Evaulating left to right, so no order of operations :(");

    if args.is_empty() {
        let mut return_buf = String::new();
        let mut input = InputLines {
            read_buf: String::new(),
            return_buf: &mut return_buf,
        };

        print!("> ");
        while let Some(()) = input.next() {
            if input.return_buf == "exit" {
                println!();
                return;
            }
            match solve(input.return_buf) {
                Ok(sol) => println!(" = {sol}"),
                Err(e) => println!(" {e}"),
            }

            print!("> ");
        }
    } else {
        print!("{args} ");
        match solve(&args) {
            Ok(sol) => println!(" = {sol}"),
            Err(e) => println!(" {e}"),
        }
    }
}

enum Operators {
    Emit,
    Add,
    Sub,
    Mul,
    Div,
}
