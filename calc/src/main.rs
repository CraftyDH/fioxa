#![no_std]
#![no_main]

use kernel_userspace::syscall::{exit, read_args};

extern crate alloc;
#[macro_use]
extern crate userspace;
extern crate userspace_bumpalloc;

#[export_name = "_start"]
pub extern "C" fn main() {
    let args = read_args();

    println!("WARN: Evaulating left to right, so no order of operations :(");

    println!("{args} =");

    let mut res: isize = 0;
    let mut op = Operators::Emit;

    let mut curr_val: isize = 0;

    for c in args.chars() {
        if c == ' ' {
            continue;
        }

        if c.is_alphanumeric() {
            curr_val = curr_val * 10 + c.to_digit(10).unwrap() as isize;
        } else {
            match op {
                Operators::Emit => res = curr_val,
                Operators::Add => res += curr_val,
                Operators::Sub => res -= curr_val,
                Operators::Mul => res *= curr_val,
                Operators::Div => res /= curr_val,
            }
            curr_val = 0;
            op = match c {
                '+' => Operators::Add,
                '-' => Operators::Sub,
                '*' => Operators::Mul,
                '/' => Operators::Div,
                _ => todo!(),
            }
        }
    }

    match op {
        Operators::Emit => res = curr_val,
        Operators::Add => res += curr_val,
        Operators::Sub => res -= curr_val,
        Operators::Mul => res *= curr_val,
        Operators::Div => res /= curr_val,
    }

    println!("{res}");
    //
    exit();
}

enum Operators {
    Emit,
    Add,
    Sub,
    Mul,
    Div,
}

#[panic_handler]
fn panic(i: &core::panic::PanicInfo) -> ! {
    println!("{}", i);
    exit()
}
