//! Closed-subset reflection of Serde derive attributes.
//!
//! rspyts generates clients from a manifest, so accepting a Serde option
//! that the manifest cannot describe would create two competing wire
//! contracts. This module is intentionally allow-list based: new Serde
//! behavior is rejected until it is modeled end to end.

use crate::casing::RenameRule;
use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::meta::ParseNestedMeta;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{Attribute, LitStr, Path, Token};

#[derive(Clone, Debug)]
pub struct StringSetting {
    pub value: String,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct RuleSetting {
    pub value: RenameRule,
    pub span: Span,
}

/// Supported Serde settings on a struct or enum.
#[derive(Default, Debug)]
pub struct ContainerModel {
    pub rename: Option<StringSetting>,
    pub rename_all: Option<RuleSetting>,
    pub tag: Option<StringSetting>,
    pub transparent: Option<Span>,
    pub deny_unknown_fields: Option<Span>,
}

/// Supported Serde settings on a field or enum variant.
#[derive(Default, Debug)]
pub struct MemberModel {
    pub rename: Option<StringSetting>,
    /// Serde permits this on a variant to rename its named fields.
    pub rename_all: Option<RuleSetting>,
}

pub fn parse_container(attrs: &[Attribute]) -> syn::Result<ContainerModel> {
    let mut model = ContainerModel::default();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("serde")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                set_once(&mut model.rename, parse_string(&meta, "rename")?, "rename")
            } else if meta.path.is_ident("rename_all") {
                set_once(&mut model.rename_all, parse_rule(&meta)?, "rename_all")
            } else if meta.path.is_ident("tag") {
                set_once(&mut model.tag, parse_string(&meta, "tag")?, "tag")
            } else if meta.path.is_ident("transparent") {
                parse_flag(&meta, "transparent")?;
                set_span_once(&mut model.transparent, meta.path.span(), "transparent")
            } else if meta.path.is_ident("deny_unknown_fields") {
                parse_flag(&meta, "deny_unknown_fields")?;
                set_span_once(
                    &mut model.deny_unknown_fields,
                    meta.path.span(),
                    "deny_unknown_fields",
                )
            } else {
                Err(unsupported(&meta, Scope::Container))
            }
        })?;
    }
    Ok(model)
}

pub fn parse_field(attrs: &[Attribute]) -> syn::Result<MemberModel> {
    parse_member(attrs, Scope::Field)
}

pub fn parse_variant(attrs: &[Attribute]) -> syn::Result<MemberModel> {
    parse_member(attrs, Scope::Variant)
}

fn parse_member(attrs: &[Attribute], scope: Scope) -> syn::Result<MemberModel> {
    let mut model = MemberModel::default();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("serde")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                set_once(&mut model.rename, parse_string(&meta, "rename")?, "rename")
            } else if scope == Scope::Variant && meta.path.is_ident("rename_all") {
                set_once(&mut model.rename_all, parse_rule(&meta)?, "rename_all")
            } else {
                Err(unsupported(&meta, scope))
            }
        })?;
    }
    Ok(model)
}

fn parse_string(meta: &ParseNestedMeta<'_>, key: &str) -> syn::Result<StringSetting> {
    if meta.input.peek(syn::token::Paren) {
        return Err(meta.error(format!(
            "direction-specific `serde({key}(serialize = …, deserialize = …))` is not supported; one manifest wire name must apply in both directions"
        )));
    }
    let value = meta.value().map_err(|_| {
        meta.error(format!(
            "`serde({key})` requires a string literal: `{key} = \"…\"`"
        ))
    })?;
    let lit: LitStr = value.parse().map_err(|_| {
        meta.error(format!(
            "`serde({key})` requires a string literal: `{key} = \"…\"`"
        ))
    })?;
    Ok(StringSetting {
        value: lit.value(),
        span: lit.span(),
    })
}

fn parse_rule(meta: &ParseNestedMeta<'_>) -> syn::Result<RuleSetting> {
    let setting = parse_string(meta, "rename_all")?;
    let value = RenameRule::parse(&setting.value).ok_or_else(|| {
        syn::Error::new(
            setting.span,
            format!(
                "unsupported Serde rename rule; expected one of {}",
                RenameRule::SUPPORTED
            ),
        )
    })?;
    Ok(RuleSetting {
        value,
        span: setting.span,
    })
}

fn parse_flag(meta: &ParseNestedMeta<'_>, key: &str) -> syn::Result<()> {
    if meta.input.peek(Token![=]) || meta.input.peek(syn::token::Paren) {
        Err(meta.error(format!("`serde({key})` is a flag and takes no value")))
    } else {
        Ok(())
    }
}

fn set_once<T>(slot: &mut Option<T>, value: T, key: &str) -> syn::Result<()> {
    if slot.is_some() {
        Err(syn::Error::new(
            Span::call_site(),
            format!("duplicate `serde({key})` setting"),
        ))
    } else {
        *slot = Some(value);
        Ok(())
    }
}

fn set_span_once(slot: &mut Option<Span>, value: Span, key: &str) -> syn::Result<()> {
    if slot.is_some() {
        Err(syn::Error::new(
            value,
            format!("duplicate `serde({key})` setting"),
        ))
    } else {
        *slot = Some(value);
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Container,
    Field,
    Variant,
}

fn unsupported(meta: &ParseNestedMeta<'_>, scope: Scope) -> syn::Error {
    let key = meta
        .path
        .segments
        .last()
        .map(|segment| segment.ident.to_string())
        .unwrap_or_else(|| String::from("unknown"));
    let supported = match scope {
        Scope::Container => {
            "`rename`, `rename_all`, `tag`, `transparent`, and `deny_unknown_fields`"
        }
        Scope::Field => "`rename`",
        Scope::Variant => "`rename` and `rename_all`",
    };
    meta.error(format!(
        "unsupported Serde attribute `{key}` on this {}; rspyts can reflect only {supported}. Use `rspyts::Json` for a deliberately schemaless custom wire contract",
        match scope {
            Scope::Container => "type",
            Scope::Field => "field",
            Scope::Variant => "variant",
        }
    ))
}

/// Require syntactic Serde derives in adoption mode.
///
/// Trait assertions are emitted separately so aliases or unusual derive
/// paths cannot accidentally satisfy only this syntax check.
pub fn require_adoption_derives(attrs: &[Attribute], ident: &syn::Ident) -> syn::Result<()> {
    let derives = derive_set(attrs)?;
    if derives.serialize && derives.deserialize {
        Ok(())
    } else {
        let missing = match (derives.serialize, derives.deserialize) {
            (false, false) => "Serialize and Deserialize",
            (false, true) => "Serialize",
            (true, false) => "Deserialize",
            (true, true) => unreachable!(),
        };
        Err(syn::Error::new_spanned(
            ident,
            format!(
                "#[bridge(serde)] adopts an existing derived contract and requires syntactic Serde derives for both Serialize and Deserialize; missing {missing}"
            ),
        ))
    }
}

/// Owning mode must be the only source of Serde implementations.
pub fn reject_existing_serde_derives(attrs: &[Attribute]) -> syn::Result<()> {
    let derives = derive_set(attrs)?;
    if let Some(span) = derives.first_serde_span {
        Err(syn::Error::new(
            span,
            "default #[bridge] owns the Serde derives; remove the existing Serialize/Deserialize derives or use #[bridge(serde)] to adopt them",
        ))
    } else {
        Ok(())
    }
}

pub fn adoption_trait_assertion(ident: &syn::Ident) -> TokenStream {
    quote! {
        const _: () = {
            fn __rspyts_assert_serde<T>()
            where
                T: ::rspyts::__private::serde::Serialize,
                for<'__rspyts_de> T: ::rspyts::__private::serde::Deserialize<'__rspyts_de>,
            {}
            let _ = __rspyts_assert_serde::<#ident>;
        };
    }
}

#[derive(Default)]
struct DeriveSet {
    serialize: bool,
    deserialize: bool,
    first_serde_span: Option<Span>,
}

fn derive_set(attrs: &[Attribute]) -> syn::Result<DeriveSet> {
    let mut found = DeriveSet::default();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("derive")) {
        let paths = attr.parse_args_with(Punctuated::<Path, Token![,]>::parse_terminated)?;
        for path in paths {
            let Some(last) = path.segments.last() else {
                continue;
            };
            if last.ident == "Serialize" {
                found.serialize = true;
                found.first_serde_span.get_or_insert(last.ident.span());
            } else if last.ident == "Deserialize" {
                found.deserialize = true;
                found.first_serde_span.get_or_insert(last.ident.span());
            }
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_closed_container_subset() {
        let item: syn::ItemStruct = syn::parse_quote! {
            #[serde(rename = "Wire", rename_all = "kebab-case", tag = "kind", transparent, deny_unknown_fields)]
            struct Value { field: u32 }
        };
        let model = parse_container(&item.attrs).unwrap();
        assert_eq!(model.rename.unwrap().value, "Wire");
        assert_eq!(model.rename_all.unwrap().value, RenameRule::Kebab);
        assert_eq!(model.tag.unwrap().value, "kind");
        assert!(model.transparent.is_some());
        assert!(model.deny_unknown_fields.is_some());
    }

    #[test]
    fn rejects_shape_changing_keys_at_the_key() {
        let item: syn::ItemStruct = syn::parse_quote! {
            struct Value { #[serde(flatten)] field: u32 }
        };
        let field = match item.fields {
            syn::Fields::Named(fields) => fields.named.into_iter().next().unwrap(),
            _ => unreachable!(),
        };
        let error = parse_field(&field.attrs).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unsupported Serde attribute `flatten`")
        );
    }

    #[test]
    fn directional_renames_are_rejected() {
        let item: syn::ItemStruct = syn::parse_quote! {
            struct Value { #[serde(rename(serialize = "out", deserialize = "in"))] field: u32 }
        };
        let field = match item.fields {
            syn::Fields::Named(fields) => fields.named.into_iter().next().unwrap(),
            _ => unreachable!(),
        };
        let error = parse_field(&field.attrs).unwrap_err();
        assert!(error.to_string().contains("direction-specific"));
    }
}
