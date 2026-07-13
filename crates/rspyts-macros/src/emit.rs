//! Shared token-emission helpers.
//!
//! Every path in emitted code goes through `::rspyts::__private::…` (or
//! `::std`/`::core`), so expansions compile in any crate that depends on
//! the `rspyts` facade, regardless of what is in scope at the call site.

use crate::attrs::TargetArg;
use crate::casing::to_camel;
use crate::sig::{Borrow, BridgedParam, ParamKind, RetKind};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};

/// The `origin` expression for a type or const registration.
///
/// Emitted as `env!("CARGO_PKG_NAME")` so it expands during the *defining*
/// crate's build — macro output always compiles inside that crate, which is
/// exactly what cross-crate type identity needs (codegen.md §9).
pub fn origin_expr() -> TokenStream {
    quote!(::std::string::String::from(::core::env!("CARGO_PKG_NAME")))
}

/// The `targets` expression for a function, method, or static: the single
/// projection named by `#[bridge(target = "…")]`, or every target.
pub fn targets_expr(target: Option<TargetArg>) -> TokenStream {
    match target {
        None => quote!(::rspyts::__private::ir::Target::all()),
        Some(TargetArg::Python) => {
            quote!(::std::vec![::rspyts::__private::ir::Target::Python])
        }
        Some(TargetArg::Typescript) => {
            quote!(::std::vec![::rspyts::__private::ir::Target::Typescript])
        }
    }
}

/// The generated `#[derive(Deserialize)]` struct holding a shim's plain
/// (JSON-carried) parameters, keyed camelCase on the wire (ABI §3.1).
pub fn args_struct(ident: &syn::Ident, params: &[BridgedParam]) -> TokenStream {
    let fields = params.iter().filter_map(|param| match &param.kind {
        ParamKind::Plain { owned, .. } => {
            let name = &param.ident;
            Some(quote!(#name: #owned))
        }
        ParamKind::Slice { .. } => None,
    });
    quote! {
        #[derive(::rspyts::__private::serde::Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields, crate = "::rspyts::__private::serde")]
        #[doc(hidden)]
        #[allow(non_camel_case_types)]
        struct #ident { #(#fields,)* }
    }
}

/// The pieces of a shim derived from its parameter list: the C parameter
/// declarations (`args_ptr, args_len[, s0_ptr, s0_len, …]` — the caller
/// prepends the handle for methods), the decode-and-bind prelude, and the
/// argument expressions for the call to the original function.
pub struct ShimBindings {
    pub c_params: Vec<TokenStream>,
    pub prelude: TokenStream,
    pub call_args: Vec<TokenStream>,
}

pub fn shim_bindings(args_struct: &syn::Ident, params: &[BridgedParam]) -> ShimBindings {
    let args_ptr = format_ident!("__rspyts_args_ptr");
    let args_len = format_ident!("__rspyts_args_len");
    let mut c_params = vec![quote!(#args_ptr: *const u8), quote!(#args_len: usize)];
    let mut slice_lets = Vec::new();
    let mut call_args = Vec::new();
    let mut slice_index = 0usize;

    for param in params {
        let ident = &param.ident;
        match &param.kind {
            ParamKind::Slice { elem, .. } => {
                let ptr = format_ident!("__rspyts_s{}_ptr", slice_index);
                let len = format_ident!("__rspyts_s{}_len", slice_index);
                slice_index += 1;
                c_params.push(quote!(#ptr: *const #elem));
                c_params.push(quote!(#len: usize));
                slice_lets.push(quote! {
                    let #ident: &[#elem] =
                        unsafe { ::rspyts::__private::shim::slice_arg(#ptr, #len) };
                });
                call_args.push(quote!(#ident));
            }
            ParamKind::Plain { borrow, .. } => {
                call_args.push(match borrow {
                    Borrow::Owned => quote!(__rspyts_args.#ident),
                    Borrow::Ref => quote!(&__rspyts_args.#ident),
                    Borrow::Str => quote!(__rspyts_args.#ident.as_str()),
                });
            }
        }
    }

    let prelude = quote! {
        let __rspyts_args: #args_struct = unsafe {
            ::rspyts::__private::shim::decode_args(#args_ptr, #args_len)
        }?;
        #(#slice_lets)*
    };
    ShimBindings {
        c_params,
        prelude,
        call_args,
    }
}

/// An `ir::ParamDecl` expression for the registration builder.
pub fn param_decl(param: &BridgedParam) -> TokenStream {
    let name = param.ident.to_string();
    let wire = to_camel(&name);
    let ty = match &param.kind {
        ParamKind::Slice { dtype, .. } => quote! {
            ::rspyts::__private::ir::Ty::Slice { dt: ::rspyts::__private::ir::Dtype::#dtype }
        },
        ParamKind::Plain { owned, .. } => quote!(<#owned as ::rspyts::__private::Bridged>::ty()),
    };
    quote! {
        ::rspyts::__private::ir::ParamDecl {
            name: ::std::string::String::from(#name),
            wire_name: ::std::string::String::from(#wire),
            ty: #ty,
        }
    }
}

/// An `ir::Ty` expression for a return type.
pub fn ret_ty(ret: &RetKind) -> TokenStream {
    match ret {
        RetKind::Unit => quote!(<() as ::rspyts::__private::Bridged>::ty()),
        RetKind::Plain(ty) => quote!(<#ty as ::rspyts::__private::Bridged>::ty()),
        RetKind::Result { ok, .. } => quote!(<#ok as ::rspyts::__private::Bridged>::ty()),
    }
}

/// The `Option<String>` error-enum name expression for a return type.
pub fn err_name(ret: &RetKind) -> TokenStream {
    match ret {
        RetKind::Result {
            err_name: Some(name),
            ..
        } => {
            quote!(::std::option::Option::Some(::std::string::String::from(#name)))
        }
        _ => quote!(::std::option::Option::None),
    }
}

/// An `ir::FieldDecl` expression.
pub fn field_decl(
    name: &str,
    wire_name: &str,
    docs: &str,
    ty: &syn::Type,
    optional: bool,
) -> TokenStream {
    quote! {
        ::rspyts::__private::ir::FieldDecl {
            name: ::std::string::String::from(#name),
            wire_name: ::std::string::String::from(#wire_name),
            docs: ::std::string::String::from(#docs),
            ty: <#ty as ::rspyts::__private::Bridged>::ty(),
            optional: #optional,
        }
    }
}

/// `impl Bridged` returning `Ty::Ref { name }` for a named data type.
pub fn bridged_ref_impl(ident: &syn::Ident, name: &str) -> TokenStream {
    quote! {
        #[automatically_derived]
        impl ::rspyts::__private::Bridged for #ident {
            fn ty() -> ::rspyts::__private::ir::Ty {
                ::rspyts::__private::ir::Ty::Ref { name: ::std::string::String::from(#name) }
            }
        }
    }
}

/// Inventory registration of a `TypeDecl` built at manifest time.
///
/// The builder is a named `fn` item so it coerces to the plain
/// `fn() -> TypeDecl` pointer `registry::RegisteredType` expects.
pub fn register_type(decl: TokenStream) -> TokenStream {
    quote! {
        const _: () = {
            fn __rspyts_type_decl() -> ::rspyts::__private::ir::TypeDecl { #decl }
            ::rspyts::__private::inventory::submit! {
                ::rspyts::__private::registry::RegisteredType { build: __rspyts_type_decl }
            }
        };
    }
}

/// Inventory registration of a `ConstDecl` built at manifest time.
pub fn register_const(decl: TokenStream) -> TokenStream {
    quote! {
        const _: () = {
            fn __rspyts_const_decl() -> ::rspyts::__private::ir::ConstDecl { #decl }
            ::rspyts::__private::inventory::submit! {
                ::rspyts::__private::registry::RegisteredConst { build: __rspyts_const_decl }
            }
        };
    }
}

/// Inventory registration of an `FnDecl`.
pub fn register_fn(decl: TokenStream) -> TokenStream {
    quote! {
        const _: () = {
            fn __rspyts_fn_decl() -> ::rspyts::__private::ir::FnDecl { #decl }
            ::rspyts::__private::inventory::submit! {
                ::rspyts::__private::registry::RegisteredFn { build: __rspyts_fn_decl }
            }
        };
    }
}

/// Inventory registration of a `ClassDecl`.
pub fn register_class(decl: TokenStream) -> TokenStream {
    quote! {
        const _: () = {
            fn __rspyts_class_decl() -> ::rspyts::__private::ir::ClassDecl { #decl }
            ::rspyts::__private::inventory::submit! {
                ::rspyts::__private::registry::RegisteredClass { build: __rspyts_class_decl }
            }
        };
    }
}
