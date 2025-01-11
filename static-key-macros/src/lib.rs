use std::cmp::Ordering;

use proc_macro2::{Literal, Span};
use syn::{parse::Parse, parse_macro_input, Arm, LitStr, Path, Token, Type};

struct StaticMatch {
    target_arch: LitStr,
    crate_path: Path,
    key: Path,
    ty: Type,
    semicolon: Token![;],
    arms: Vec<Arm>,
}

impl Parse for StaticMatch {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let target_arch = input.parse()?;
        let _: Token![,] = input.parse()?;
        let crate_path = input.parse()?;
        let _: Token![;] = input.parse()?;
        let key = input.parse()?;
        let _: Token![:] = input.parse()?;
        let ty = input.parse()?;
        let semicolon = input.parse()?;
        let mut arms = Vec::new();
        while !input.is_empty() {
            arms.push(input.parse()?);
        }
        Ok(Self {
            target_arch,
            crate_path,
            key,
            ty,
            semicolon,
            arms,
        })
    }
}

#[proc_macro]
pub fn parse_static_match(tokens: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let StaticMatch {
        target_arch,
        crate_path,
        key,
        ty,
        semicolon,
        mut arms,
    } = parse_macro_input!(tokens as StaticMatch);

    if arms.is_empty() {
        return syn::Error::new(semicolon.span, "static_match! cannot be used without arms")
            .into_compile_error()
            .into();
    }

    // Extract the most likely arm, which will be used as the fallthrough arm.
    let likely_arm = arms
        .iter_mut()
        .position(|x| {
            let mut found = false;
            // Find the `#[likely]` attribute and remove it.
            x.attrs.retain(|x| {
                if x.path().is_ident("likely") {
                    found = true;
                    return false;
                }
                true
            });
            found
        })
        .unwrap_or(arms.len() - 1);

    // Generate a matcher function that matches on a `&ty` and returns an index of the match arm.
    // For the `likely` arm, it needs to be encoded as `usize::MAX` instead.
    let matcher_arms: Vec<_> = arms
        .iter()
        .enumerate()
        .map(|(idx, x)| {
            let mapped_idx = match idx.cmp(&likely_arm) {
                Ordering::Less => idx as isize,
                Ordering::Equal => -1,
                Ordering::Greater => idx as isize - 1,
            };
            Arm {
                body: Box::new(
                    syn::ExprLit {
                        attrs: Vec::new(),
                        lit: syn::Lit::new(Literal::isize_unsuffixed(mapped_idx)),
                    }
                    .into(),
                ),
                comma: Some(Token![,](Span::mixed_site())),
                ..x.clone()
            }
        })
        .collect();
    let matcher_fn = quote::quote_spanned! { Span::mixed_site() =>
        fn matcher(value: &#ty) -> isize {
            match value {
                #(#matcher_arms)*
            }
        }
    };

    let fallback_body = arms.remove(likely_arm).body;
    let arms_len = arms.len();
    let label_bodies: Vec<_> = arms.into_iter().map(|arm| arm.body).collect();

    match target_arch.value().as_str() {
        "x86_64" => {
            let label_templates = (0..arms_len).map(|_| ".4byte {} - (2b + 5)");

            quote::quote_spanned! { Span::mixed_site() => 'label:{unsafe{
                #matcher_fn

                // Check the type of key and expected type matches.
                const _: *const #crate_path::StaticKey::<#ty> = &raw const #key;

                ::core::arch::asm!(
                    // Aligns the start to 8 byte boundary if doing so only require 1 bytes.
                    // This means that the start address % 8 will be 0~6 (but never 7).
                    // This ensures that we can at least atomically replace 2 bytes at a time.
                    // We pad using 0x2E which is the CS prefix, so we still have 1 single instruction.
                    ".p2align 3,0x2e,1",
                    "2:",
                    // 5 bytes
                    "ud2; ud2; nop",
                    r#".pushsection .data.static_match.jump_table"#,
                    ".p2align 3",
                    "3:",
                    ".8byte {key}",
                    ".8byte 2b",
                    ".8byte {matcher}",
                    ".8byte 0",
                    #(#label_templates,)*
                    ".popsection",
                    r#".pushsection .text.startup.static_match.init"#,
                    "4:",
                    "lea rdi, [rip + 3b]",
                    "jmp {register}",
                    ".popsection",
                    ".pushsection .init_array",
                    ".8byte 4b",
                    ".popsection",
                    #(
                        label { break 'label { match () { () => #label_bodies } }; },
                    )*
                    key = sym #key,
                    matcher = sym matcher,
                    register = sym #crate_path::CallSite::<#ty>::register,
                    options(nomem, nostack)
                );

                match () { () => #fallback_body }
            }}}
        }
        "riscv64" => {
            let label_templates = (0..arms_len).map(|_| ".4byte {} - 2b");

            quote::quote_spanned! { Span::mixed_site() => 'label:{unsafe{
                #matcher_fn

                // Check the type of key and expected type matches.
                const _: *const #crate_path::StaticKey::<#ty> = &raw const #key;

                ::core::arch::asm!(
                    // When RISC-V QEMU runs userspace level emulation, a thread enters a infinite loop and
                    // cannot leave it even if the instruction is modified and `sync_core()`.
                    // Align the instruction so instruction can be atomically replaced without need to add
                    // an infinite loop.
                    ".p2align 2",
                    "2:",
                    ".option push",
                    ".option norelax",
                    ".option norvc",
                    "unimp",
                    ".option pop",
                    r#".pushsection .data.static_match.jump_table"#,
                    ".p2align 3",
                    "3:",
                    ".8byte {key}",
                    ".8byte 2b",
                    ".8byte {matcher}",
                    ".8byte 0",
                    #(#label_templates,)*
                    ".popsection",
                    r#".pushsection .text.startup.static_match.init"#,
                    "4:",
                    "la a0, 3b",
                    "j {register}",
                    ".popsection",
                    ".pushsection .init_array",
                    ".8byte 4b",
                    ".popsection",
                    #(
                        label { break 'label { match () { () => #label_bodies } }; },
                    )*
                    key = sym #key,
                    matcher = sym matcher,
                    register = sym #crate_path::CallSite::<#ty>::register,
                    options(nomem, nostack)
                );

                match () { () => #fallback_body }
            }}}
        }
        _ => {
            unimplemented!();
        }
    }
    .into()
}
