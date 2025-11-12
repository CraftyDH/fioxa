use core::{
    convert::Into,
    iter::{IntoIterator, Iterator},
};

use proc_macro::TokenStream;
use proc_macro2::Literal;
use quote::quote;
use syn::{
    Attribute, Ident, Token, Type, parenthesized, parse::Parse, parse_macro_input,
    punctuated::Punctuated,
};

#[derive(Debug)]
struct Syscalls {
    abi: Literal,
    syscalls: Punctuated<Syscall, Token![,]>,
}

impl Parse for Syscalls {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let abi = input.parse()?;
        input.parse::<Token![,]>()?;
        Ok(Self {
            abi,
            syscalls: input.parse_terminated(Syscall::parse, Token![,])?,
        })
    }
}

#[derive(Debug)]
#[allow(dead_code)]
struct Syscall {
    struct_name: Ident,
    attrs: Vec<syn::Attribute>,
    name: Ident,
    args: Punctuated<Arg, Token![,]>,
}

impl Parse for Syscall {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let struct_name = input.parse()?;

        input.parse::<Token![@]>()?;

        let attrs = input.call(Attribute::parse_outer)?;
        let name = input.parse()?;

        let args;
        parenthesized!(args in input);

        let args = args.parse_terminated(Arg::parse, Token![,])?;

        Ok(Self {
            struct_name,
            attrs,
            name,
            args,
        })
    }
}

#[derive(Debug)]
struct Arg {
    name: Ident,
    ty: Type,
}

impl Parse for Arg {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let name = input.parse()?;
        input.parse::<Token![:]>()?;
        let ty = input.parse()?;

        Ok(Self { name, ty })
    }
}

const SCRATCH_REGISTERS: [&str; 8] = ["r11", "r10", "r9", "r8", "rdi", "rsi", "rdx", "rcx"];

#[proc_macro]
pub fn define_syscalls(tokens: TokenStream) -> TokenStream {
    let syscalls = parse_macro_input!(tokens as Syscalls);

    let abi = syscalls.abi.to_string();
    // trim quotes
    let abi = &abi[1..abi.len() - 1];

    let mut result = Vec::new();

    #[cfg(feature = "kernel")]
    {
        for syscall in &syscalls.syscalls {
            let args = syscall.args.iter().map(|arg| {
                let name = &arg.name;
                let ty = &arg.ty;
                quote!(pub #name: #ty)
            });

            let name = &syscall.struct_name;
            result.push(quote! {
                pub struct #name {
                    #(#args,)*
                }
            });
        }

        {
            let builder = syscalls.syscalls.iter().enumerate().map(|e| {
                let name = &e.1.struct_name;
                quote! { #name (#name) }
            });

            result.push(quote! {
                pub enum RawSyscalls {
                    #(#builder,)*
                }
            });
        }

        {
            let builder = syscalls.syscalls.iter().enumerate().map(|e| {
                let args = e.1.args.iter().enumerate().map(|(i, arg)| {
                    let name = &arg.name;
                    let ty = &arg.ty;

                    quote!(#name: args[#i] as #ty)
                });

                let num = e.0;
                let name = &e.1.struct_name;
                quote! { #num => Some(RawSyscalls::#name (#name { #(#args,)* })) }
            });

            result.push(quote! {
                #[inline]
                pub fn parse_syscalls(number: usize, args: &[usize; 6]) -> Option<RawSyscalls> {
                    match number {
                        #(#builder,)*
                        _ => None
                    }
                }
            });
        }

        {
            let builder1 = syscalls.syscalls.iter().map(|e| {
                let name = &e.name;
                let sname = &e.struct_name;
                quote!(fn #name(&mut self, req: &#sname) -> crate::types::SyscallResult {
                    crate::types::SyscallResult::Unimplemented
                })
            });

            let builder2 = syscalls.syscalls.iter().map(|e| {
                let name = &e.name;
                let sname = &e.struct_name;
                quote!(RawSyscalls::#sname(req) => self.#name(req))
            });

            result.push(quote! {
                pub trait DispatchSyscall {
                    #(#builder1)*

                    fn dispatch(&mut self, req: &RawSyscalls) -> crate::types::SyscallResult {
                        match req {
                            #(#builder2,)*
                        }
                    }
                }
            });
        }
    }

    if abi == "kernel" || abi == "driver" || abi == "userspace" {
        let parameter_registers: [&str; 6] = if abi == "kernel" || abi == "driver" {
            ["rdi", "rsi", "rdx", "rcx", "r8", "r9"]
        } else {
            ["rdi", "rsi", "rdx", "r10", "r8", "r9"]
        };

        let mut clobbers = quote!();
        let asm_call = if abi == "kernel" {
            // ! Slightly sketch reading into CPU local storage
            quote! {"mov r11, gs:0x8; call r11"}
        } else if abi == "driver" {
            quote! {"int 0x80"}
        } else {
            clobbers = quote!(lateout("r12") _,);
            quote! {"syscall" }
        };

        for (num, syscall) in syscalls.syscalls.into_iter().enumerate() {
            let attrs = syscall.attrs.iter();
            let name = syscall.name;

            let args = syscall.args.iter().map(|arg| {
                let name = &arg.name;
                let ty = &arg.ty;
                quote!(#name: #ty)
            });

            let asm_reg = syscall
                .args
                .iter()
                .zip(parameter_registers.iter())
                .map(|(arg, reg)| {
                    let name = &arg.name;
                    quote!(in(#reg) #name)
                });

            let scratch = SCRATCH_REGISTERS.iter().map(|r| quote!(lateout(#r) _));

            result.push(quote!(
                #(#attrs)*
                #[inline]
                pub unsafe fn #name(#(#args,)*) -> result_t {
                    let res: result_t;
                    ::core::arch::asm!(
                        #asm_call,
                        in("rax") #num,
                        #(#asm_reg,)*
                        #(#scratch,)*
                        lateout("rax") res,
                        #clobbers
                        options(nostack, preserves_flags)
                    );
                    res
                }
            ));
        }
    } else {
        // let lsp work by creating stubs
        for syscall in syscalls.syscalls.into_iter() {
            let attrs = syscall.attrs.iter();
            let name = syscall.name;

            let args = syscall.args.iter().map(|arg| {
                let name = &arg.name;
                let ty = &arg.ty;
                quote!(#name: #ty)
            });

            result.push(quote!(
                #(#attrs)*
                #[allow(unused_variables)]
                pub unsafe fn #name(#(#args,)*) -> result_t {
                    unimplemented!("fioxa syscall");
                }
            ));
        }
    }

    quote!(#(#result)*).into()
}
