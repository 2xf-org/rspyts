//! Expansion of `#[bridge]` on `const` items.
//!
//! The const is re-emitted untouched, followed by an inventory
//! registration whose builder schema-normalizes the value with
//! `wire::serialize(&NAME, &ty)` at manifest-build time — the manifest
//! carries the exact ABI wire value, and the emitters project it as a real
//! importable constant in each language.
//!
//! Because `&str` (and friends) are not `Bridged`, the IR type of the
//! const is resolved *syntactically* here: `&'static str` maps to
//! `Ty::String`, references to slices and plain arrays map to `Ty::List`
//! of the recursively mapped element, and every other path type falls back
//! to `<T as Bridged>::inventory_ty()` — so an owned bridged type works and a
//! non-bridgeable one fails with the usual trait-bound error.

use crate::attrs::BridgeArgs;
use crate::docs::extract_docs;
use crate::emit;
use crate::sig;
use proc_macro2::TokenStream;
use quote::quote;

pub fn expand_const(args: BridgeArgs, item: syn::ItemConst) -> syn::Result<TokenStream> {
    args.deny_error("constants")?;
    args.deny_constructor()?;
    args.deny_static("constants; `static` marks a method inside a #[bridge] impl block")?;
    args.deny_tag("constants")?;
    args.deny_rename_all("constants")?;
    args.deny_target("constants; constants are projected into every target")?;
    args.deny_serde("constants; it adopts Serde derives on data types")?;
    sig::ensure_no_generics(&item.generics, "constants")?;
    sig::reject_attachment_type(&item.ty, "a bridged constant")?;

    let ty_expr = const_ir_ty(&item.ty)?;
    let ident = &item.ident;
    let name_str = ident.to_string();
    let docs = extract_docs(&item.attrs);
    let origin = emit::origin_expr();

    let registration = emit::register_const(quote! {
        ::rspyts::__private::ir::ConstDecl {
            name: ::std::string::String::from(#name_str),
            docs: ::std::string::String::from(#docs),
            origin: #origin,
            ty: #ty_expr,
            value: ::rspyts::__private::wire::serialize(&#ident, &#ty_expr)
                .expect("rspyts: bridged const failed wire normalization"),
        }
    });

    Ok(quote! {
        #item
        #registration
    })
}

/// Map the written const type to an `ir::Ty` expression (see module docs).
fn const_ir_ty(ty: &syn::Type) -> syn::Result<TokenStream> {
    match ty {
        syn::Type::Reference(reference) => {
            if reference.mutability.is_some() {
                return Err(unsupported(ty));
            }
            match &*reference.elem {
                syn::Type::Path(path) if path.qself.is_none() && path.path.is_ident("str") => {
                    Ok(quote!(::rspyts::__private::ir::Ty::String))
                }
                syn::Type::Slice(slice) => Ok(list_of(const_ir_ty(&slice.elem)?)),
                _ => Err(unsupported(ty)),
            }
        }
        syn::Type::Array(array) => Ok(list_of(const_ir_ty(&array.elem)?)),
        syn::Type::Paren(paren) => const_ir_ty(&paren.elem),
        syn::Type::Group(group) => const_ir_ty(&group.elem),
        syn::Type::Path(_) | syn::Type::Tuple(_) => {
            // Owned types: membership is enforced semantically by `Bridged`
            // (`()` only ever satisfies it for the empty tuple).
            Ok(quote!(<#ty as ::rspyts::__private::Bridged>::inventory_ty()))
        }
        _ => Err(unsupported(ty)),
    }
}

fn list_of(inner: TokenStream) -> TokenStream {
    quote! {
        ::rspyts::__private::ir::Ty::List { inner: ::std::boxed::Box::new(#inner) }
    }
}

fn unsupported(ty: &syn::Type) -> syn::Error {
    syn::Error::new_spanned(
        ty,
        "this type cannot be a bridged const — supported: scalars, `&'static str`, \
         arrays and slices of supported types, and owned bridged data types \
         (see the `bridge` macro docs)",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::ToTokens;
    use syn::parse_quote;

    fn ty_tokens(ty: syn::Type) -> String {
        const_ir_ty(&ty)
            .unwrap()
            .to_token_stream()
            .to_string()
            .replace(' ', "")
    }

    #[test]
    fn str_refs_map_to_string() {
        assert_eq!(
            ty_tokens(parse_quote!(&'static str)),
            "::rspyts::__private::ir::Ty::String"
        );
        assert_eq!(
            ty_tokens(parse_quote!(&str)),
            "::rspyts::__private::ir::Ty::String"
        );
    }

    #[test]
    fn slices_and_arrays_map_to_lists() {
        assert_eq!(
            ty_tokens(parse_quote!(&'static [&'static str])),
            "::rspyts::__private::ir::Ty::List{inner:::std::boxed::Box::new\
             (::rspyts::__private::ir::Ty::String)}"
        );
        assert!(ty_tokens(parse_quote!([f64; 3])).starts_with("::rspyts::__private::ir::Ty::List"));
        assert!(ty_tokens(parse_quote!(&[u8])).contains("Bridged>::inventory_ty()"));
    }

    #[test]
    fn owned_path_types_fall_back_to_bridged() {
        assert_eq!(
            ty_tokens(parse_quote!(f64)),
            "<f64as::rspyts::__private::Bridged>::inventory_ty()"
        );
        assert_eq!(
            ty_tokens(parse_quote!(Option<u32>)),
            "<Option<u32>as::rspyts::__private::Bridged>::inventory_ty()"
        );
    }

    #[test]
    fn unsupported_shapes_are_rejected() {
        assert!(const_ir_ty(&parse_quote!(&'static f64)).is_err());
        assert!(const_ir_ty(&parse_quote!(&mut u32)).is_err());
        assert!(const_ir_ty(&parse_quote!(*const u8)).is_err());
        assert!(const_ir_ty(&parse_quote!(fn() -> u32)).is_err());
    }
}
