//! Expansion of `#[bridge]` on data structs, data enums, string enums,
//! and `#[bridge(error)]` error enums (type-system §3, §4, §8).

use crate::attrs::{BridgeArgs, deny_bridge_attrs, take_field_required};
use crate::casing::{RenameRule, field_wire_name, variant_wire_name};
use crate::docs::extract_docs;
use crate::emit;
use crate::serde_reflect::{self, ContainerModel, MemberModel};
use crate::sig;
use proc_macro2::TokenStream;
use quote::quote;
use std::collections::BTreeSet;
use syn::parse_quote;

const RESERVED_WIRE_KEYS: [&str; 1] = ["__proto__"];

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum SerdeMode {
    Owning,
    Adopted,
}

impl SerdeMode {
    fn from_args(args: &BridgeArgs) -> Self {
        if args.serde.is_some() {
            Self::Adopted
        } else {
            Self::Owning
        }
    }
}

fn validate_mode(
    mode: SerdeMode,
    args: &BridgeArgs,
    attrs: &[syn::Attribute],
    ident: &syn::Ident,
) -> syn::Result<()> {
    match mode {
        SerdeMode::Owning => serde_reflect::reject_existing_serde_derives(attrs),
        SerdeMode::Adopted => {
            if let Some((_, span)) = args.rename_all {
                return Err(syn::Error::new(
                    span,
                    "#[bridge(rename_all = …)] competes with the adopted Serde contract; put `rename_all` in #[serde(...)] instead",
                ));
            }
            if let Some(tag) = &args.tag {
                return Err(syn::Error::new(
                    tag.span(),
                    "#[bridge(tag = …)] competes with the adopted Serde contract; put `tag` in #[serde(...)] instead",
                ));
            }
            serde_reflect::require_adoption_derives(attrs, ident)
        }
    }
}

fn reject_rule_authority_conflict(args: &BridgeArgs, model: &ContainerModel) -> syn::Result<()> {
    if let (Some(_), Some(serde_rule)) = (&args.rename_all, &model.rename_all) {
        return Err(syn::Error::new(
            serde_rule.span,
            "Serde `rename_all` and #[bridge(rename_all = …)] are competing authorities; keep only one",
        ));
    }
    Ok(())
}

fn reject_tag_authority_conflict(args: &BridgeArgs, model: &ContainerModel) -> syn::Result<()> {
    if let (Some(_), Some(serde_tag)) = (&args.tag, &model.tag) {
        return Err(syn::Error::new(
            serde_tag.span,
            "Serde `tag` and #[bridge(tag = …)] are competing authorities; keep only one",
        ));
    }
    Ok(())
}

fn member_wire(name: &str, model: &MemberModel, rule: RenameRule, variant: bool) -> String {
    model.rename.as_ref().map_or_else(
        || {
            if variant {
                variant_wire_name(name, rule)
            } else {
                field_wire_name(name, rule)
            }
        },
        |rename| rename.value.clone(),
    )
}

fn insert_owning_derives(attrs: &mut Vec<syn::Attribute>) {
    // A derive helper must appear before the first `#[serde(...)]` under
    // `legacy_derive_helpers`-denying builds. Preserve all user attribute
    // order while inserting the owned derives at that boundary.
    let index = attrs
        .iter()
        .position(|attr| attr.path().is_ident("serde"))
        .unwrap_or(attrs.len());
    attrs.insert(
        index,
        parse_quote! {
            #[derive(::rspyts::__private::serde::Serialize, ::rspyts::__private::serde::Deserialize)]
        },
    );
}

/// `#[bridge]` on a struct.
pub fn expand_struct(args: BridgeArgs, mut item: syn::ItemStruct) -> syn::Result<TokenStream> {
    args.deny_error("structs; `error` marks an enum")?;
    args.deny_tag("structs; `tag` sets the discriminator of a data enum")?;
    deny_fn_only_args(&args, "structs")?;
    sig::ensure_no_generics(&item.generics, "structs")?;

    let serde_model = serde_reflect::parse_container(&item.attrs)?;
    if let Some(tag) = &serde_model.tag {
        return Err(syn::Error::new(
            tag.span,
            "`serde(tag = …)` applies only to internally tagged enums, not structs",
        ));
    }
    let mode = SerdeMode::from_args(&args);
    validate_mode(mode, &args, &item.attrs, &item.ident)?;
    reject_rule_authority_conflict(&args, &serde_model)?;

    let tuple_newtype =
        matches!(&item.fields, syn::Fields::Unnamed(fields) if fields.unnamed.len() == 1);
    if tuple_newtype || serde_model.transparent.is_some() {
        return expand_newtype(args, item, serde_model, mode);
    }

    let name_str = item.ident.to_string();
    let fields = match &mut item.fields {
        syn::Fields::Named(fields) => &mut fields.named,
        syn::Fields::Unnamed(_) => {
            return Err(syn::Error::new_spanned(
                &item.ident,
                "bridged tuple structs must contain exactly one public field so they can project as a transparent newtype; use named fields for an object shape",
            ));
        }
        syn::Fields::Unit => {
            return Err(syn::Error::new_spanned(
                &item.ident,
                "unit structs are not bridgeable (docs/design/type-system.md §9)",
            ));
        }
    };

    let rule = serde_model
        .rename_all
        .as_ref()
        .map(|setting| setting.value)
        .or_else(|| args.rename_all.as_ref().map(|(rule, _)| *rule))
        .unwrap_or(match mode {
            SerdeMode::Owning => RenameRule::Camel,
            SerdeMode::Adopted => RenameRule::None,
        });
    let mut field_decls = Vec::new();
    let mut wire_names = BTreeSet::new();
    for field in fields {
        if !matches!(field.vis, syn::Visibility::Public(_)) {
            return Err(syn::Error::new_spanned(
                field,
                "every field of a bridged struct must be `pub` — the whole shape crosses the boundary (docs/design/type-system.md §3)",
            ));
        }
        let is_option = sig::is_option(&field.ty);
        let required = take_field_required(&mut field.attrs, is_option, "struct fields")?;
        let field_model = serde_reflect::parse_field(&field.attrs)?;
        let name = field.ident.as_ref().expect("named field").to_string();
        let wire = member_wire(&name, &field_model, rule, false);
        check_object_key(field, &wire, &mut wire_names, "struct field")?;
        let docs = extract_docs(&field.attrs);
        field_decls.push(emit::field_decl(&name, &wire, &docs, &field.ty, required));
    }
    if mode == SerdeMode::Adopted && serde_model.deny_unknown_fields.is_none() {
        return Err(syn::Error::new_spanned(
            &item.ident,
            "#[bridge(serde)] object structs must use #[serde(deny_unknown_fields)] so Rust, Python, and TypeScript reject the same unknown keys",
        ));
    }

    let docs = extract_docs(&item.attrs);
    let assertion = match mode {
        SerdeMode::Owning => {
            insert_owning_derives(&mut item.attrs);
            if serde_model.rename_all.is_none() {
                let serde_rule = rule.serde_value().expect("owning mode always has a rule");
                item.attrs
                    .push(parse_quote!(#[serde(rename_all = #serde_rule)]));
            }
            if serde_model.deny_unknown_fields.is_none() {
                item.attrs.push(parse_quote!(#[serde(deny_unknown_fields)]));
            }
            item.attrs.push(parse_quote! {
                #[serde(crate = "::rspyts::__private::serde")]
            });
            TokenStream::new()
        }
        SerdeMode::Adopted => serde_reflect::adoption_trait_assertion(&item.ident),
    };

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

    Ok(quote! { #item #assertion #bridged_impl #registration })
}

fn expand_newtype(
    args: BridgeArgs,
    mut item: syn::ItemStruct,
    serde_model: ContainerModel,
    mode: SerdeMode,
) -> syn::Result<TokenStream> {
    if let Some((_, span)) = args.rename_all {
        return Err(syn::Error::new(
            span,
            "`rename_all` does not apply to a transparent newtype because it has no wire object fields",
        ));
    }
    if let Some(rename_all) = &serde_model.rename_all {
        return Err(syn::Error::new(
            rename_all.span,
            "`serde(rename_all = …)` does not apply to a transparent newtype because it has no wire object fields",
        ));
    }

    let field = match &item.fields {
        syn::Fields::Unnamed(fields) if fields.unnamed.len() == 1 => &fields.unnamed[0],
        syn::Fields::Named(fields) if fields.named.len() == 1 => &fields.named[0],
        syn::Fields::Unnamed(fields) => {
            return Err(syn::Error::new_spanned(
                fields,
                "transparent newtypes must contain exactly one field; multifield tuple structs are not bridgeable",
            ));
        }
        syn::Fields::Named(fields) => {
            return Err(syn::Error::new_spanned(
                fields,
                "`serde(transparent)` bridged structs must contain exactly one field",
            ));
        }
        syn::Fields::Unit => {
            return Err(syn::Error::new_spanned(
                &item.ident,
                "transparent newtypes must contain exactly one field; unit structs have no inner wire type",
            ));
        }
    };
    if !matches!(field.vis, syn::Visibility::Public(_)) {
        return Err(syn::Error::new_spanned(
            field,
            "the field of a bridged newtype must be `pub`",
        ));
    }
    deny_bridge_attrs(&field.attrs, "newtype fields")?;
    let field_model = serde_reflect::parse_field(&field.attrs)?;
    if let Some(rename) = field_model.rename {
        return Err(syn::Error::new(
            rename.span,
            "`serde(rename = …)` does not apply to the inner value of a transparent newtype",
        ));
    }

    let inner = &field.ty;
    let name = item.ident.to_string();
    let docs = extract_docs(&item.attrs);
    let assertion = match mode {
        SerdeMode::Owning => {
            insert_owning_derives(&mut item.attrs);
            if serde_model.transparent.is_none() {
                item.attrs.push(parse_quote!(#[serde(transparent)]));
            }
            item.attrs.push(parse_quote! {
                #[serde(crate = "::rspyts::__private::serde")]
            });
            TokenStream::new()
        }
        SerdeMode::Adopted => serde_reflect::adoption_trait_assertion(&item.ident),
    };
    let origin = emit::origin_expr();
    let bridged_impl = emit::bridged_ref_impl(&item.ident, &name);
    let registration = emit::register_type(quote! {
        ::rspyts::__private::ir::TypeDecl::Newtype {
            name: ::std::string::String::from(#name),
            docs: ::std::string::String::from(#docs),
            origin: #origin,
            inner: <#inner as ::rspyts::__private::Bridged>::inventory_ty(),
        }
    });

    Ok(quote! { #item #assertion #bridged_impl #registration })
}

/// `#[bridge]` / `#[bridge(error)]` / `#[bridge(tag = …)]` on an enum.
pub fn expand_enum(args: BridgeArgs, item: syn::ItemEnum) -> syn::Result<TokenStream> {
    if args.error.is_some() {
        return expand_error_enum(args, item);
    }
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

fn expand_string_enum(args: BridgeArgs, mut item: syn::ItemEnum) -> syn::Result<TokenStream> {
    let serde_model = serde_reflect::parse_container(&item.attrs)?;
    if let Some(span) = serde_model.transparent {
        return Err(syn::Error::new(
            span,
            "`serde(transparent)` applies to single-field structs, not enums",
        ));
    }
    let mode = SerdeMode::from_args(&args);
    validate_mode(mode, &args, &item.attrs, &item.ident)?;
    args.deny_tag("string enums; a tag would change the string wire shape")?;
    if let Some(tag) = &serde_model.tag {
        return Err(syn::Error::new(
            tag.span,
            "`serde(tag = …)` changes a fieldless enum from a string into an object, which is not a string-enum contract",
        ));
    }
    if mode == SerdeMode::Owning {
        args.deny_rename_all(
            "enums; set Serde `rename_all` on the enum when custom variant casing is needed",
        )?;
    }

    let rule = serde_model
        .rename_all
        .as_ref()
        .map(|setting| setting.value)
        .unwrap_or(match mode {
            SerdeMode::Owning => RenameRule::Camel,
            SerdeMode::Adopted => RenameRule::None,
        });
    let mut variant_decls = Vec::new();
    let mut wire_names = BTreeSet::new();
    for variant in &item.variants {
        deny_bridge_attrs(&variant.attrs, "enum variants")?;
        let variant_model = serde_reflect::parse_variant(&variant.attrs)?;
        if let Some(rename_all) = variant_model.rename_all {
            return Err(syn::Error::new(
                rename_all.span,
                "`serde(rename_all = …)` on a fieldless variant has no fields to rename",
            ));
        }
        let name = variant.ident.to_string();
        let wire = member_wire(&name, &variant_model, rule, true);
        if !wire_names.insert(wire.clone()) {
            return Err(syn::Error::new_spanned(
                variant,
                format!("duplicate string-enum wire value `{wire}`"),
            ));
        }
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
    let assertion = match mode {
        SerdeMode::Owning => {
            insert_owning_derives(&mut item.attrs);
            if serde_model.rename_all.is_none() {
                let serde_rule = rule.serde_value().expect("owning mode always has a rule");
                item.attrs
                    .push(parse_quote!(#[serde(rename_all = #serde_rule)]));
            }
            item.attrs.push(parse_quote! {
                #[serde(crate = "::rspyts::__private::serde")]
            });
            TokenStream::new()
        }
        SerdeMode::Adopted => serde_reflect::adoption_trait_assertion(&item.ident),
    };
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

    Ok(quote! { #item #assertion #bridged_impl #registration })
}

fn expand_data_enum(args: BridgeArgs, mut item: syn::ItemEnum) -> syn::Result<TokenStream> {
    let serde_model = serde_reflect::parse_container(&item.attrs)?;
    if let Some(span) = serde_model.transparent {
        return Err(syn::Error::new(
            span,
            "`serde(transparent)` applies to single-field structs, not enums",
        ));
    }
    let mode = SerdeMode::from_args(&args);
    validate_mode(mode, &args, &item.attrs, &item.ident)?;
    reject_rule_authority_conflict(&args, &serde_model)?;
    reject_tag_authority_conflict(&args, &serde_model)?;
    if mode == SerdeMode::Owning {
        args.deny_rename_all(
            "enums; set Serde `rename_all` on the enum when custom variant casing is needed",
        )?;
    }

    let variant_rule = serde_model
        .rename_all
        .as_ref()
        .map(|setting| setting.value)
        .unwrap_or(match mode {
            SerdeMode::Owning => RenameRule::Camel,
            SerdeMode::Adopted => RenameRule::None,
        });
    let tag = serde_model
        .tag
        .as_ref()
        .map(|setting| setting.value.clone())
        .or_else(|| args.tag.as_ref().map(syn::LitStr::value))
        .or_else(|| (mode == SerdeMode::Owning).then(|| String::from("type")))
        .ok_or_else(|| {
            syn::Error::new_spanned(
                &item.ident,
                "#[bridge(serde)] data enums must use an existing `#[serde(tag = \"…\")]` internally tagged contract",
            )
        })?;
    if RESERVED_WIRE_KEYS.contains(&tag.as_str()) {
        return Err(syn::Error::new_spanned(
            &item.ident,
            format!("enum discriminator `{tag}` is unsafe for JavaScript object projection"),
        ));
    }

    let mut variant_decls = Vec::new();
    let mut variant_wires = BTreeSet::new();
    for variant in &mut item.variants {
        deny_bridge_attrs(&variant.attrs, "enum variants")?;
        let variant_model = serde_reflect::parse_variant(&variant.attrs)?;
        let fields = match &mut variant.fields {
            syn::Fields::Named(fields) => Some(&mut fields.named),
            syn::Fields::Unit => None,
            syn::Fields::Unnamed(_) => {
                return Err(syn::Error::new_spanned(
                    variant,
                    "tuple variants are not bridgeable; use named fields (`Variant { … }`) or a fieldless variant (docs/design/type-system.md §4)",
                ));
            }
        };
        let has_fields = fields.is_some();
        if !has_fields {
            if let Some(rename_all) = &variant_model.rename_all {
                return Err(syn::Error::new(
                    rename_all.span,
                    "`serde(rename_all = …)` on a fieldless variant has no fields to rename",
                ));
            }
        }
        let field_rule = variant_model
            .rename_all
            .as_ref()
            .map(|setting| setting.value)
            .unwrap_or(match mode {
                SerdeMode::Owning => RenameRule::Camel,
                SerdeMode::Adopted => RenameRule::None,
            });
        let mut field_decls = Vec::new();
        let mut field_wires = BTreeSet::new();
        for field in fields.into_iter().flatten() {
            let is_option = sig::is_option(&field.ty);
            let required = take_field_required(&mut field.attrs, is_option, "variant fields")?;
            let field_model = serde_reflect::parse_field(&field.attrs)?;
            let name = field.ident.as_ref().expect("named field").to_string();
            let wire = member_wire(&name, &field_model, field_rule, false);
            check_object_key(field, &wire, &mut field_wires, "enum field")?;
            if wire == tag {
                return Err(syn::Error::new_spanned(
                    field,
                    format!("enum field wire name `{wire}` collides with the discriminator"),
                ));
            }
            let docs = extract_docs(&field.attrs);
            field_decls.push(emit::field_decl(&name, &wire, &docs, &field.ty, required));
        }
        let name = variant.ident.to_string();
        let wire = member_wire(&name, &variant_model, variant_rule, true);
        if !variant_wires.insert(wire.clone()) {
            return Err(syn::Error::new_spanned(
                variant,
                format!("duplicate data-enum variant wire name `{wire}`"),
            ));
        }
        let docs = extract_docs(&variant.attrs);
        variant_decls.push(quote! {
            ::rspyts::__private::ir::VariantDecl {
                name: ::std::string::String::from(#name),
                wire_name: ::std::string::String::from(#wire),
                docs: ::std::string::String::from(#docs),
                fields: ::std::vec![#(#field_decls),*],
            }
        });
        if mode == SerdeMode::Owning && has_fields && variant_model.rename_all.is_none() {
            let serde_rule = field_rule
                .serde_value()
                .expect("owning mode always has a rule");
            variant
                .attrs
                .push(parse_quote!(#[serde(rename_all = #serde_rule)]));
        }
    }
    if mode == SerdeMode::Adopted && serde_model.deny_unknown_fields.is_none() {
        return Err(syn::Error::new_spanned(
            &item.ident,
            "#[bridge(serde)] data enums must use #[serde(deny_unknown_fields)] so Rust, Python, and TypeScript reject the same unknown keys",
        ));
    }

    let name_str = item.ident.to_string();
    let docs = extract_docs(&item.attrs);
    let assertion = match mode {
        SerdeMode::Owning => {
            insert_owning_derives(&mut item.attrs);
            if serde_model.tag.is_none() {
                item.attrs.push(parse_quote!(#[serde(tag = #tag)]));
            }
            if serde_model.rename_all.is_none() {
                let serde_rule = variant_rule
                    .serde_value()
                    .expect("owning mode always has a rule");
                item.attrs
                    .push(parse_quote!(#[serde(rename_all = #serde_rule)]));
            }
            if serde_model.deny_unknown_fields.is_none() {
                item.attrs.push(parse_quote!(#[serde(deny_unknown_fields)]));
            }
            item.attrs.push(parse_quote! {
                #[serde(crate = "::rspyts::__private::serde")]
            });
            TokenStream::new()
        }
        SerdeMode::Adopted => serde_reflect::adoption_trait_assertion(&item.ident),
    };
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

    Ok(quote! { #item #assertion #bridged_impl #registration })
}

fn expand_error_enum(args: BridgeArgs, mut item: syn::ItemEnum) -> syn::Result<TokenStream> {
    args.deny_serde("error enums; error envelopes are not Serde data contracts")?;
    args.deny_tag("error enums; errors carry their code in the envelope, not a tag")?;
    args.deny_rename_all("error enums; codes are always camelCase")?;
    deny_fn_only_args(&args, "error enums")?;
    sig::ensure_no_generics(&item.generics, "enums")?;
    ensure_variants(&item)?;

    let ident = item.ident.clone();
    let mut arms = Vec::new();
    let mut variant_decls = Vec::new();
    let mut error_codes = BTreeSet::new();
    for variant in &mut item.variants {
        deny_bridge_attrs(&variant.attrs, "enum variants")?;
        let variant_ident = variant.ident.clone();
        let name = variant_ident.to_string();
        let code = variant_wire_name(&name, RenameRule::Camel);
        if !error_codes.insert(code.clone()) {
            return Err(syn::Error::new_spanned(
                variant,
                format!("duplicate application-error wire code `{code}`"),
            ));
        }
        let docs = extract_docs(&variant.attrs);
        match &mut variant.fields {
            syn::Fields::Unit => {
                arms.push(
                    quote! { #ident::#variant_ident => (#code, ::std::option::Option::None), },
                );
                variant_decls.push(error_variant_decl(&name, &code, &docs, quote!()));
            }
            syn::Fields::Named(fields) => {
                let mut idents = Vec::new();
                let mut keys = Vec::new();
                let mut field_decls = Vec::new();
                let mut field_wires = BTreeSet::new();
                for field in &mut fields.named {
                    let is_option = sig::is_option(&field.ty);
                    let required =
                        take_field_required(&mut field.attrs, is_option, "variant fields")?;
                    sig::reject_attachment_type(&field.ty, "an application-error field")?;
                    let field_ident = field.ident.clone().expect("named field");
                    let field_name = field_ident.to_string();
                    let wire = field_wire_name(&field_name, RenameRule::Camel);
                    check_object_key(field, &wire, &mut field_wires, "application-error field")?;
                    let field_docs = extract_docs(&field.attrs);
                    field_decls.push(emit::field_decl(
                        &field_name,
                        &wire,
                        &field_docs,
                        &field.ty,
                        required,
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
                    "tuple variants are not bridgeable; use named fields (`Variant { … }`) or a fieldless variant (docs/design/type-system.md §4)",
                ));
            }
        }
    }

    let name_str = ident.to_string();
    let docs = extract_docs(&item.attrs);
    let bridge_err_impl = quote! {
        #[automatically_derived]
        impl ::rspyts::BridgeErr for #ident {
            fn into_bridge_error(self) -> ::rspyts::BridgeError {
                let __rspyts_message = ::std::string::ToString::to_string(&self);
                let (__rspyts_code, __rspyts_data): (
                    &'static str,
                    ::std::option::Option<::rspyts::__private::serde_json::Value>,
                ) = match self { #(#arms)* };
                let __rspyts_data = __rspyts_data.map(|data| {
                    ::rspyts::__private::wire::normalize_error_data(
                        data,
                        &::rspyts::__private::ir::Ty::qualified_ref_name(
                            ::core::env!("CARGO_PKG_NAME"),
                            #name_str,
                        ),
                        __rspyts_code,
                    )
                    .expect("rspyts: bridged error data failed wire normalization")
                });
                ::rspyts::BridgeError {
                    code: ::std::string::String::from(__rspyts_code),
                    message: __rspyts_message,
                    data: __rspyts_data,
                }
            }

            fn inventory_name() -> ::std::option::Option<::std::string::String> {
                ::std::option::Option::Some(
                    ::rspyts::__private::ir::Ty::qualified_ref_name(
                        ::core::env!("CARGO_PKG_NAME"),
                        #name_str,
                    ),
                )
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

    Ok(quote! { #item #bridge_err_impl #registration })
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

fn check_object_key<T: quote::ToTokens>(
    node: &T,
    wire: &str,
    seen: &mut BTreeSet<String>,
    kind: &str,
) -> syn::Result<()> {
    if RESERVED_WIRE_KEYS.contains(&wire) {
        return Err(syn::Error::new_spanned(
            node,
            format!("{kind} wire name `{wire}` is unsafe for JavaScript object projection"),
        ));
    }
    if !seen.insert(wire.to_string()) {
        return Err(syn::Error::new_spanned(
            node,
            format!("duplicate {kind} wire name `{wire}`"),
        ));
    }
    Ok(())
}

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
                "tuple variants are not bridgeable; use named fields (`Variant { … }`) (docs/design/type-system.md §4)",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::ToTokens;
    use syn::parse_quote;

    #[test]
    fn struct_fields_emit_required_polarity_and_strip_the_marker() {
        let item: syn::ItemStruct = parse_quote! {
            pub struct Presence {
                pub omittable: Option<String>,
                #[bridge(required)]
                pub nullable_but_required: Option<String>,
                pub always_required: String,
            }
        };
        let tokens = expand_struct(BridgeArgs::default(), item)
            .unwrap()
            .to_token_stream()
            .to_string()
            .replace(' ', "");

        assert_eq!(tokens.matches("required:false").count(), 1, "{tokens}");
        assert_eq!(tokens.matches("required:true").count(), 2, "{tokens}");
        assert!(!tokens.contains("bridge(required)"), "{tokens}");
    }
}
