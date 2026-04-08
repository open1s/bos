use proc_macro::TokenStream;

use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{parse_macro_input, Item, Path, Result, Token};

struct ArchiveArgs {
    crate_path: Option<Path>,
}

impl Parse for ArchiveArgs {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        if input.is_empty() {
            return Ok(Self { crate_path: None });
        }

        let ident: syn::Ident = input.parse()?;
        if ident != "crate_path" {
            return Err(syn::Error::new(
                ident.span(),
                "expected `crate_path = <path>`",
            ));
        }
        input.parse::<Token![=]>()?;
        let crate_path: Path = input.parse()?;

        if !input.is_empty() {
            return Err(input.error("unexpected extra tokens"));
        }

        Ok(Self {
            crate_path: Some(crate_path),
        })
    }
}

#[allow(non_snake_case)]
#[proc_macro_attribute]
pub fn Archive(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand(attr, item)
}

#[allow(non_snake_case)]
#[proc_macro_attribute]
pub fn Snapshot(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand(attr, item)
}

#[proc_macro_attribute]
pub fn archive(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand(attr, item)
}

#[proc_macro_attribute]
pub fn snapshot(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand(attr, item)
}

fn expand(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as ArchiveArgs);
    let item = parse_macro_input!(item as Item);
    let crate_path = args
        .crate_path
        .unwrap_or_else(|| syn::parse_quote!(::qserde));

    quote! {
        #[derive(
            #crate_path::rkyv::Archive,
            #crate_path::rkyv::Serialize,
            #crate_path::rkyv::Deserialize
        )]
        #[rkyv(crate = #crate_path::rkyv)]
        #item
    }
    .into()
}
