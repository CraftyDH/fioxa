use core::{
    cfg,
    convert::Into,
    iter::{IntoIterator, Iterator},
};

use proc_macro::TokenStream;
use quote::quote;
use syn::{
    Attribute, Ident, Token, Type, parenthesized, parse::Parse, parse_macro_input,
    punctuated::Punctuated,
};

#[cfg(all(feature = "kernel", feature = "iret"))]
const _: () = compile_error!("kernel and iret syscall types are incompatable");

#[derive(Debug)]
struct Syscalls {
    syscalls: Punctuated<Syscall, Token![,]>,
}

impl Parse for Syscalls {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        Ok(Self {
            syscalls: input.parse_terminated(Syscall::parse, Token![,])?,
        })
    }
}

#[derive(Debug)]
struct Syscall {
    attrs: Vec<syn::Attribute>,
    name: Ident,
    args: Punctuated<Arg, Token![,]>,
    res: SyscallResult,
}

#[derive(Debug)]
enum SyscallResult {
    None,
    Never,
    One(Type),
}

impl Parse for SyscallResult {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        if input.parse::<Token![->]>().is_err() {
            return Ok(Self::None);
        }

        if input.parse::<Token![!]>().is_ok() {
            return Ok(Self::Never);
        }

        Ok(SyscallResult::One(input.parse()?))
    }
}

impl Parse for Syscall {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let attrs = input.call(Attribute::parse_outer)?;
        let name = input.parse()?;

        let args;
        parenthesized!(args in input);

        let args = args.parse_terminated(Arg::parse, Token![,])?;

        let res = input.parse()?;

        Ok(Self {
            attrs,
            name,
            args,
            res,
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

const PARAMETER_REGISTERS: [&str; 6] = if cfg!(feature = "kernel") || cfg!(feature = "iret") {
    ["rdi", "rsi", "rdx", "rcx", "r8", "r9"]
} else {
    ["rdi", "rsi", "rdx", "r10", "r8", "r9"]
};

const SCRATCH_REGISTERS: [&str; 8] = ["r11", "r10", "r9", "r8", "rdi", "rsi", "rdx", "rcx"];

#[proc_macro]
pub fn define_syscalls(tokens: TokenStream) -> TokenStream {
    let syscalls = parse_macro_input!(tokens as Syscalls);

    let mut result = Vec::new();

    let mut clobbers = quote!();
    let asm_call = if cfg!(feature = "kernel") {
        // ! Slightly sketch reading into CPU local storage
        quote! {"mov r11, gs:0x8; call r11"}
    } else if cfg!(feature = "iret") {
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
            .zip(PARAMETER_REGISTERS.iter())
            .map(|(arg, reg)| {
                let name = &arg.name;
                quote!(in(#reg) #name)
            });

        let scratch = SCRATCH_REGISTERS.iter().map(|r| quote!(lateout(#r) _));

        match syscall.res {
            SyscallResult::None => {
                result.push(quote!(
                    #(#attrs)*
                    #[inline]
                    pub unsafe fn #name(#(#args,)*) {
                        ::core::arch::asm!(
                            #asm_call,
                            in("rax") #num,
                            #(#asm_reg,)*
                            #(#scratch,)*
                            lateout("rax") _,
                            #clobbers
                            options(nostack, preserves_flags)
                        );
                    }
                ));
            }
            SyscallResult::Never => {
                result.push(quote!(
                    #(#attrs)*
                    #[inline]
                    pub unsafe fn #name(#(#args,)*) -> ! {
                        ::core::arch::asm!(
                            #asm_call,
                            "ud2",
                            in("rax") #num,
                            #(#asm_reg,)*
                            options(nostack, preserves_flags, noreturn)
                        );
                    }
                ));
            }
            SyscallResult::One(ret) => {
                result.push(quote!(
                    #(#attrs)*
                    #[inline]
                    pub unsafe fn #name(#(#args,)*) -> #ret {
                        let res: #ret;
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
        }
    }

    quote!(#(#result)*).into()
}
