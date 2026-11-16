//! Attribute macros that add rkyv archive support to qserde types.
//!
//! [`Archive`] points the derived rkyv impls at the rkyv re-export inside the
//! qserde crate, so downstream crates depend only on qserde.
#![warn(missing_docs)]

use proc_macro::TokenStream;

use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{parse_macro_input, Attribute, Error, Item, Path, Result, Token};

struct ArchiveArgs {
    crate_path: Option<Path>,
    rkyv_path: Option<Path>,
}

impl Parse for ArchiveArgs {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        if input.is_empty() {
            return Ok(Self {
                crate_path: None,
                rkyv_path: None,
            });
        }

        let fork = input.fork();
        if let Ok(_path) = fork.parse::<Path>() {
            if fork.is_empty() {
                return Ok(Self {
                    crate_path: Some(input.parse()?),
                    rkyv_path: None,
                });
            }
        }

        let mut crate_path = None;
        let mut rkyv_path = None;

        while !input.is_empty() {
            let ident: syn::Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            let path: Path = input.parse()?;

            match ident.to_string().as_str() {
                "crate" | "crate_path" => {
                    if crate_path.is_some() {
                        return Err(Error::new(ident.span(), "duplicate `crate` argument"));
                    }
                    crate_path = Some(path);
                }
                "rkyv" | "rkyv_path" => {
                    if rkyv_path.is_some() {
                        return Err(Error::new(ident.span(), "duplicate `rkyv` argument"));
                    }
                    rkyv_path = Some(path);
                }
                _ => {
                    return Err(Error::new(
                        ident.span(),
                        "unsupported argument, expected one of: `crate`, `crate_path`, `rkyv`, `rkyv_path`",
                    ));
                }
            }

            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            } else if !input.is_empty() {
                return Err(input.error("expected `,` between arguments"));
            }
        }

        Ok(Self {
            crate_path,
            rkyv_path,
        })
    }
}

#[allow(non_snake_case)]
#[proc_macro_attribute]
/// Derive rkyv `Archive`, `Serialize`, and `Deserialize` for a type.
///
/// Accepts `crate`/`crate_path` and `rkyv`/`rkyv_path` arguments.
pub fn Archive(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand(attr, item)
}

#[allow(non_snake_case)]
#[proc_macro_attribute]
/// Alias of [`Archive`], kept for rkyv-style naming.
pub fn Snapshot(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand(attr, item)
}

#[proc_macro_attribute]
/// Lowercase alias of [`Archive`].
pub fn archive(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand(attr, item)
}

#[proc_macro_attribute]
/// Lowercase alias of [`Snapshot`].
pub fn snapshot(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand(attr, item)
}

fn expand(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as ArchiveArgs);
    let item = parse_macro_input!(item as Item);

    if !matches!(item, Item::Struct(_) | Item::Enum(_) | Item::Union(_)) {
        return Error::new_spanned(item, "#[Archive] only supports struct/enum/union")
            .to_compile_error()
            .into();
    }

    let has_crate_attr = item_has_rkyv_crate_attr(&item);
    let crate_path = args
        .crate_path
        .unwrap_or_else(|| syn::parse_quote!(::qserde));
    let rkyv_path = args
        .rkyv_path
        .unwrap_or_else(|| syn::parse_quote!(#crate_path::rkyv));
    // Only suppress the injected path when the user already pinned one. Other
    // rkyv arguments (derive, compare, archive_bounds, ...) still need the
    // generated impls to resolve rkyv through qserde.
    let maybe_rkyv_attr = if has_crate_attr {
        quote! {}
    } else {
        quote! {
            #[rkyv(crate = #rkyv_path)]
        }
    };

    quote! {
        #[derive(
            #crate_path::rkyv::Archive,
            #crate_path::rkyv::Serialize,
            #crate_path::rkyv::Deserialize
        )]
        #maybe_rkyv_attr
        #item
    }
    .into()
}

/// Whether the item already pins rkyv's crate path in a crate argument.
///
/// Other rkyv arguments (derive, compare, archive_bounds, ...) must not
/// suppress the crate path injected by this macro, otherwise a downstream crate
/// that only depends on qserde fails to resolve the bare rkyv path.
fn item_has_rkyv_crate_attr(item: &Item) -> bool {
    let attrs: &[Attribute] = match item {
        Item::Struct(item) => &item.attrs,
        Item::Enum(item) => &item.attrs,
        Item::Union(item) => &item.attrs,
        _ => return false,
    };

    attrs
        .iter()
        .filter(|attr| attr.path().is_ident("rkyv"))
        .any(|attr| match &attr.meta {
            syn::Meta::List(list) => list
                .parse_args_with(
                    syn::punctuated::Punctuated::<syn::Meta, Token![,]>::parse_terminated,
                )
                .map(|metas| metas.iter().any(|meta| meta.path().is_ident("crate")))
                // Let rkyv's derive report a malformed attribute.
                .unwrap_or(true),
            _ => false,
        })
}
