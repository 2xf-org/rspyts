//! Expansion of `#[bridge]` on data structs, data enums, string enums,
//! and `#[bridge(error)]` error enums (type-system §3, §4, §8).

use crate::attrs::{BridgeArgs, deny_bridge_attrs};
use crate::casing::{RenameRule, wire_name};
use crate::docs::extract_docs;
use crate::emit;
use crate::sig;
use proc_macro2::TokenStream;
use quote::quote;
use syn::parse_quote;

/// Denials shared by every data-type expansion (the arguments below only
/// make sense on functions, methods, or statics).
fn deny_fn_only_args(args: &BridgeArgs, what: &str) -> syn::Result<()> {
    args.deny_constructor()?;
    args.deny_static(&format!(
        "{what}; `static` marks a method inside a #[bridge] impl block"
    ))?;
    args.deny_target(&format!(
        "{what}; it scopes free functions, methods, and statics"
    ))?;
    Ok(())
}

/// `#[bridge]` on a struct: serde derives with camelCase wire names and
/// `deny_unknown_fields`, a `Bridged` impl, and a `TypeDecl::Struct`
/// registration.
pub fn expand_struct(args: BridgeArgs, mut item: syn::ItemStruct) -> syn::Result<TokenStream> {
    args.deny_error("structs; `error` marks an enum")?;
    args.deny_tag("structs; `tag` sets the discriminator of a data enum")?;
    deny_fn_only_args(&args, "structs")?;
    sig::ensure_no_generics(&item.generics, "structs")?;

    let name_str = item.ident.to_string();
    let fields = match &item.fields {
        syn::Fields::Named(fields) => &fields.named,
        syn::Fields::Unnamed(_) => {
            return Err(syn::Error::new_spanned(
                &item.ident,
                "bridged structs must have named fields; tuple structs are not \
                 bridgeable — name the fields (docs/design/type-system.md §3)",
            ));
        }
        syn::Fields::Unit => {
            return Err(syn::Error::new_spanned(
                &item.ident,
                "unit structs are not bridgeable (docs/design/type-system.md §9)",
            ));
        }
    };

    let rule = args
        .rename_all
        .map(|(rule, _)| rule)
        .unwrap_or(RenameRule::Camel);
    let mut field_decls = Vec::new();
    for field in fields.iter() {
        if !matches!(field.vis, syn::Visibility::Public(_)) {
            return Err(syn::Error::new_spanned(
                field,
                "every field of a bridged struct must be `pub` — the whole shape \
                 crosses the boundary (docs/design/type-system.md §3)",
            ));
        }
        deny_bridge_attrs(&field.attrs, "struct fields")?;
        let name = field.ident.as_ref().expect("named field").to_string();
        let wire = wire_name(&name, rule);
        let docs = extract_docs(&field.attrs);
        let optional = sig::is_option(&field.ty);
        field_decls.push(emit::field_decl(&name, &wire, &docs, &field.ty, optional));
    }

    let docs = extract_docs(&item.attrs);
    let serde_rule = rule.serde_value();
    item.attrs.push(parse_quote! {
        #[derive(::rspyts::__private::serde::Serialize, ::rspyts::__private::serde::Deserialize)]
    });
    item.attrs.push(parse_quote! {
        #[serde(rename_all = #serde_rule, deny_unknown_fields, crate = "::rspyts::__private::serde")]
    });

    let origin = emit::origin_expr();
    let bridged_impl = emit::bridged_ref_impl(&item.ident, &name_str);
    let registration = emit::register_type(quote! {
        ::rspyts::__private::ir::TypeDecl::Struct {
            name: ::std::string::String::from(#name_str),
            docs: ::std::string::String::from(#docs),
            origin: #origin,
            fields: ::std::vec![#(#field_decls),*],
        }
    });

    Ok(quote! {
        #item
        #bridged_impl
        #registration
    })
}

/// `#[bridge]` / `#[bridge(error)]` / `#[bridge(tag = …)]` on an enum.
pub fn expand_enum(args: BridgeArgs, item: syn::ItemEnum) -> syn::Result<TokenStream> {
    if args.error.is_some() {
        return expand_error_enum(args, item);
    }
    args.deny_rename_all("enums; variant wire names are always camelCase")?;
    deny_fn_only_args(&args, "enums")?;
    sig::ensure_no_generics(&item.generics, "enums")?;
    ensure_variants(&item)?;

    let any_named = item
        .variants
        .iter()
        .any(|variant| matches!(variant.fields, syn::Fields::Named(_)));
    if any_named {
        expand_data_enum(args, item)
    } else {
        expand_string_enum(args, item)
    }
}

/// All variants fieldless → a string on the wire (type-system §4).
fn expand_string_enum(args: BridgeArgs, mut item: syn::ItemEnum) -> syn::Result<TokenStream> {
    args.deny_tag("string enums; `tag` sets the discriminator of a data enum")?;

    let mut variant_decls = Vec::new();
    for variant in &item.variants {
        deny_bridge_attrs(&variant.attrs, "enum variants")?;
        let name = variant.ident.to_string();
        let wire = wire_name(&name, RenameRule::Camel);
        let docs = extract_docs(&variant.attrs);
        variant_decls.push(quote! {
            ::rspyts::__private::ir::StringVariantDecl {
                name: ::std::string::String::from(#name),
                wire_name: ::std::string::String::from(#wire),
                docs: ::std::string::String::from(#docs),
            }
        });
    }

    let name_str = item.ident.to_string();
    let docs = extract_docs(&item.attrs);
    item.attrs.push(parse_quote! {
        #[derive(::rspyts::__private::serde::Serialize, ::rspyts::__private::serde::Deserialize)]
    });
    item.attrs.push(parse_quote! {
        #[serde(rename_all = "camelCase", crate = "::rspyts::__private::serde")]
    });

    let origin = emit::origin_expr();
    let bridged_impl = emit::bridged_ref_impl(&item.ident, &name_str);
    let registration = emit::register_type(quote! {
        ::rspyts::__private::ir::TypeDecl::StringEnum {
            name: ::std::string::String::from(#name_str),
            docs: ::std::string::String::from(#docs),
            origin: #origin,
            variants: ::std::vec![#(#variant_decls),*],
        }
    });

    Ok(quote! {
        #item
        #bridged_impl
        #registration
    })
}

/// At least one variant with named fields → internally tagged union.
/// Every variant must then use named fields (type-system §4).
fn expand_data_enum(args: BridgeArgs, mut item: syn::ItemEnum) -> syn::Result<TokenStream> {
    let mut variant_decls = Vec::new();
    for variant in &mut item.variants {
        deny_bridge_attrs(&variant.attrs, "enum variants")?;
        let fields = match &variant.fields {
            syn::Fields::Named(fields) => &fields.named,
            // Tuple variants were rejected in `expand_enum`.
            syn::Fields::Unnamed(_) | syn::Fields::Unit => {
                return Err(syn::Error::new_spanned(
                    variant,
                    "mixing fieldless and data variants is not supported in v0.1 — \
                     give every variant named fields, or make all variants fieldless \
                     (docs/design/type-system.md §4)",
                ));
            }
        };
        let mut field_decls = Vec::new();
        for field in fields.iter() {
            deny_bridge_attrs(&field.attrs, "variant fields")?;
            let name = field.ident.as_ref().expect("named field").to_string();
            let wire = wire_name(&name, RenameRule::Camel);
            let docs = extract_docs(&field.attrs);
            let optional = sig::is_option(&field.ty);
            field_decls.push(emit::field_decl(&name, &wire, &docs, &field.ty, optional));
        }
        let name = variant.ident.to_string();
        let wire = wire_name(&name, RenameRule::Camel);
        let docs = extract_docs(&variant.attrs);
        variant_decls.push(quote! {
            ::rspyts::__private::ir::VariantDecl {
                name: ::std::string::String::from(#name),
                wire_name: ::std::string::String::from(#wire),
                docs: ::std::string::String::from(#docs),
                fields: ::std::vec![#(#field_decls),*],
            }
        });
        variant
            .attrs
            .push(parse_quote!(#[serde(rename_all = "camelCase")]));
    }

    let tag = args
        .tag
        .as_ref()
        .map(|lit| lit.value())
        .unwrap_or_else(|| "type".to_string());
    let name_str = item.ident.to_string();
    let docs = extract_docs(&item.attrs);
    item.attrs.push(parse_quote! {
        #[derive(::rspyts::__private::serde::Serialize, ::rspyts::__private::serde::Deserialize)]
    });
    item.attrs.push(parse_quote! {
        #[serde(tag = #tag, rename_all = "camelCase", crate = "::rspyts::__private::serde")]
    });

    let origin = emit::origin_expr();
    let bridged_impl = emit::bridged_ref_impl(&item.ident, &name_str);
    let registration = emit::register_type(quote! {
        ::rspyts::__private::ir::TypeDecl::Enum {
            name: ::std::string::String::from(#name_str),
            docs: ::std::string::String::from(#docs),
            origin: #origin,
            tag: ::std::string::String::from(#tag),
            variants: ::std::vec![#(#variant_decls),*],
        }
    });

    Ok(quote! {
        #item
        #bridged_impl
        #registration
    })
}

/// `#[bridge(error)]`: derive `BridgeErr` — camelCase variant name as
/// `code`, `Display` as `message`, named fields as `data` — and register
/// a `TypeDecl::ErrorEnum`. No serde derives: error enums never cross the
/// boundary as data (ABI §5).
fn expand_error_enum(args: BridgeArgs, item: syn::ItemEnum) -> syn::Result<TokenStream> {
    args.deny_tag("error enums; errors carry their code in the envelope, not a tag")?;
    args.deny_rename_all("error enums; codes are always camelCase")?;
    deny_fn_only_args(&args, "error enums")?;
    sig::ensure_no_generics(&item.generics, "enums")?;
    ensure_variants(&item)?;

    let ident = item.ident.clone();
    let mut arms = Vec::new();
    let mut variant_decls = Vec::new();
    for variant in &item.variants {
        deny_bridge_attrs(&variant.attrs, "enum variants")?;
        let variant_ident = variant.ident.clone();
        let name = variant_ident.to_string();
        let code = wire_name(&name, RenameRule::Camel);
        let docs = extract_docs(&variant.attrs);
        match &variant.fields {
            syn::Fields::Unit => {
                arms.push(quote! {
                    #ident::#variant_ident => (#code, ::std::option::Option::None),
                });
                variant_decls.push(error_variant_decl(&name, &code, &docs, quote!()));
            }
            syn::Fields::Named(fields) => {
                let mut idents = Vec::new();
                let mut keys = Vec::new();
                let mut field_decls = Vec::new();
                for field in fields.named.iter() {
                    deny_bridge_attrs(&field.attrs, "variant fields")?;
                    let field_ident = field.ident.clone().expect("named field");
                    let field_name = field_ident.to_string();
                    let wire = wire_name(&field_name, RenameRule::Camel);
                    let field_docs = extract_docs(&field.attrs);
                    let optional = sig::is_option(&field.ty);
                    field_decls.push(emit::field_decl(
                        &field_name,
                        &wire,
                        &field_docs,
                        &field.ty,
                        optional,
                    ));
                    idents.push(field_ident);
                    keys.push(wire);
                }
                arms.push(quote! {
                    #ident::#variant_ident { #(#idents),* } => (
                        #code,
                        ::std::option::Option::Some(
                            ::rspyts::__private::serde_json::json!({ #(#keys: #idents),* }),
                        ),
                    ),
                });
                variant_decls.push(error_variant_decl(
                    &name,
                    &code,
                    &docs,
                    quote!(#(#field_decls),*),
                ));
            }
            syn::Fields::Unnamed(_) => {
                return Err(syn::Error::new_spanned(
                    variant,
                    "tuple variants are not bridgeable; use named fields \
                     (`Variant { … }`) or a fieldless variant \
                     (docs/design/type-system.md §4)",
                ));
            }
        }
    }

    let name_str = ident.to_string();
    let docs = extract_docs(&item.attrs);

    // `Display` supplies `message`; the enum's author implements it. If
    // they forget, the missing-trait error below points at this impl.
    let bridge_err_impl = quote! {
        #[automatically_derived]
        impl ::rspyts::BridgeErr for #ident {
            fn into_bridge_error(self) -> ::rspyts::BridgeError {
                let __rspyts_message = ::std::string::ToString::to_string(&self);
                let (__rspyts_code, __rspyts_data): (
                    &'static str,
                    ::std::option::Option<::rspyts::__private::serde_json::Value>,
                ) = match self {
                    #(#arms)*
                };
                ::rspyts::BridgeError {
                    code: ::std::string::String::from(__rspyts_code),
                    message: __rspyts_message,
                    data: __rspyts_data,
                }
            }
        }
    };
    let origin = emit::origin_expr();
    let registration = emit::register_type(quote! {
        ::rspyts::__private::ir::TypeDecl::ErrorEnum {
            name: ::std::string::String::from(#name_str),
            docs: ::std::string::String::from(#docs),
            origin: #origin,
            variants: ::std::vec![#(#variant_decls),*],
        }
    });

    Ok(quote! {
        #item
        #bridge_err_impl
        #registration
    })
}

fn error_variant_decl(name: &str, code: &str, docs: &str, fields: TokenStream) -> TokenStream {
    quote! {
        ::rspyts::__private::ir::ErrorVariantDecl {
            name: ::std::string::String::from(#name),
            wire_code: ::std::string::String::from(#code),
            docs: ::std::string::String::from(#docs),
            fields: ::std::vec![#fields],
        }
    }
}

/// Common enum checks: at least one variant, no tuple variants anywhere.
fn ensure_variants(item: &syn::ItemEnum) -> syn::Result<()> {
    if item.variants.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.ident,
            "bridged enums must have at least one variant",
        ));
    }
    for variant in &item.variants {
        if matches!(variant.fields, syn::Fields::Unnamed(_)) {
            return Err(syn::Error::new_spanned(
                variant,
                "tuple variants are not bridgeable; use named fields (`Variant { … }`) \
                 (docs/design/type-system.md §4)",
            ));
        }
    }
    Ok(())
}
