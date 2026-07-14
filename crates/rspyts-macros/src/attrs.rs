//! Parsing and validation of the `#[bridge(…)]` argument list.
//!
//! The attribute accepts a small closed set of arguments; which of them
//! apply depends on the annotated item, so every expander calls the
//! `deny_*` helpers for the arguments it does not support, producing a
//! diagnostic anchored on the offending argument rather than on the item.

use crate::casing::RenameRule;
use proc_macro2::{Span, TokenStream};
use syn::parse::{ParseStream, Parser};
use syn::spanned::Spanned;
use syn::{Meta, Token};

/// One code-generation target named by `target = "…"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetArg {
    Python,
    Typescript,
}

/// The parsed `#[bridge(…)]` argument list.
#[derive(Default)]
pub struct BridgeArgs {
    /// `error` — derive `BridgeErr` for an enum instead of treating it as data.
    pub error: Option<Span>,
    /// `constructor` — marks the constructor method inside a `#[bridge]` impl.
    pub constructor: Option<Span>,
    /// `static` — marks a handle-less static method inside a `#[bridge]` impl.
    pub statik: Option<Span>,
    /// `tag = "…"` — discriminator key override for data enums (default `"type"`).
    pub tag: Option<syn::LitStr>,
    /// `rename_all = "…"` — wire-name casing override for structs.
    pub rename_all: Option<(RenameRule, Span)>,
    /// `target = "…"` — restrict a function, method, or static to one projection.
    pub target: Option<(TargetArg, Span)>,
    /// `serde` — adopt derives and supported Serde metadata already on a data type.
    pub serde: Option<Span>,
}

impl BridgeArgs {
    pub fn parse(tokens: TokenStream) -> syn::Result<Self> {
        (|input: ParseStream| Self::parse_stream(input)).parse2(tokens)
    }

    // A custom loop rather than `Punctuated::<Meta, _>` because `static`
    // is a keyword and does not parse as a `Meta` path.
    fn parse_stream(input: ParseStream) -> syn::Result<Self> {
        let mut args = Self::default();
        while !input.is_empty() {
            if input.peek(Token![static]) {
                let kw: Token![static] = input.parse()?;
                require_unset(args.statik.is_none(), kw.span)?;
                args.statik = Some(kw.span);
            } else {
                let meta: Meta = input.parse()?;
                args.parse_meta(meta)?;
            }
            if input.is_empty() {
                break;
            }
            input.parse::<Token![,]>()?;
        }
        Ok(args)
    }

    fn parse_meta(&mut self, meta: Meta) -> syn::Result<()> {
        if meta.path().is_ident("error") {
            require_bare(&meta, "error")?;
            require_unset(self.error.is_none(), meta.span())?;
            self.error = Some(meta.span());
        } else if meta.path().is_ident("serde") {
            require_bare(&meta, "serde")?;
            require_unset(self.serde.is_none(), meta.span())?;
            self.serde = Some(meta.span());
        } else if meta.path().is_ident("constructor") {
            require_bare(&meta, "constructor")?;
            require_unset(self.constructor.is_none(), meta.span())?;
            self.constructor = Some(meta.span());
        } else if meta.path().is_ident("tag") {
            require_unset(self.tag.is_none(), meta.span())?;
            self.tag = Some(str_value(&meta)?);
        } else if meta.path().is_ident("rename_all") {
            require_unset(self.rename_all.is_none(), meta.span())?;
            let lit = str_value(&meta)?;
            let rule = RenameRule::parse(&lit.value()).ok_or_else(|| {
                syn::Error::new(
                    lit.span(),
                    format!(
                        "unsupported rename_all value; expected one of {}",
                        RenameRule::SUPPORTED
                    ),
                )
            })?;
            self.rename_all = Some((rule, lit.span()));
        } else if meta.path().is_ident("target") {
            require_unset(self.target.is_none(), meta.span())?;
            let lit = str_value(&meta)?;
            let target = match lit.value().as_str() {
                "python" => TargetArg::Python,
                "typescript" => TargetArg::Typescript,
                _ => {
                    return Err(syn::Error::new(
                        lit.span(),
                        r#"unsupported target; expected "python" or "typescript""#,
                    ));
                }
            };
            self.target = Some((target, lit.span()));
        } else {
            return Err(syn::Error::new_spanned(
                meta.path(),
                "unknown #[bridge] argument; expected `error`, `serde`, `constructor`, `static`, \
                 `tag = \"…\"`, `rename_all = \"…\"`, or `target = \"…\"`",
            ));
        }
        Ok(())
    }

    pub fn deny_error(&self, context: &str) -> syn::Result<()> {
        deny(self.error, "error", context)
    }

    pub fn deny_constructor(&self) -> syn::Result<()> {
        deny(
            self.constructor,
            "constructor",
            "this item; it marks the constructor method inside a #[bridge] impl block",
        )
    }

    pub fn deny_static(&self, context: &str) -> syn::Result<()> {
        deny(self.statik, "static", context)
    }

    pub fn deny_tag(&self, context: &str) -> syn::Result<()> {
        deny(self.tag.as_ref().map(|lit| lit.span()), "tag", context)
    }

    pub fn deny_rename_all(&self, context: &str) -> syn::Result<()> {
        deny(
            self.rename_all.as_ref().map(|(_, span)| *span),
            "rename_all",
            context,
        )
    }

    pub fn deny_target(&self, context: &str) -> syn::Result<()> {
        deny(
            self.target.as_ref().map(|(_, span)| *span),
            "target",
            context,
        )
    }

    pub fn deny_serde(&self, context: &str) -> syn::Result<()> {
        deny(self.serde, "serde", context)
    }
}

/// True for `#[bridge…]` and `#[rspyts::bridge…]` attributes.
pub fn is_bridge_attr(attr: &syn::Attribute) -> bool {
    let path = attr.path();
    if path.is_ident("bridge") {
        return true;
    }
    path.segments.len() == 2
        && path.segments[0].ident == "rspyts"
        && path.segments[1].ident == "bridge"
}

/// Reject any `#[bridge(…)]` attribute on a field or enum variant — no
/// argument applies in that position. The argument list is parsed first,
/// so an unknown key (e.g. the removed `rename`) reports the standard
/// unknown-key diagnostic instead of a bespoke one.
pub fn deny_bridge_attrs(attrs: &[syn::Attribute], what: &str) -> syn::Result<()> {
    for attr in attrs {
        if !is_bridge_attr(attr) {
            continue;
        }
        let nested = match &attr.meta {
            Meta::List(list) => list.tokens.clone(),
            _ => TokenStream::new(),
        };
        BridgeArgs::parse(nested)?;
        return Err(syn::Error::new_spanned(
            attr,
            format!("#[bridge] does not apply to {what}"),
        ));
    }
    Ok(())
}

fn deny(span: Option<Span>, name: &str, context: &str) -> syn::Result<()> {
    match span {
        Some(span) => Err(syn::Error::new(
            span,
            format!("`{name}` does not apply to {context}"),
        )),
        None => Ok(()),
    }
}

fn require_bare(meta: &Meta, name: &str) -> syn::Result<()> {
    match meta {
        Meta::Path(_) => Ok(()),
        _ => Err(syn::Error::new_spanned(
            meta,
            format!("`{name}` takes no value"),
        )),
    }
}

fn require_unset(unset: bool, span: Span) -> syn::Result<()> {
    if unset {
        Ok(())
    } else {
        Err(syn::Error::new(span, "duplicate #[bridge] argument"))
    }
}

fn str_value(meta: &Meta) -> syn::Result<syn::LitStr> {
    if let Meta::NameValue(nv) = meta {
        if let syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(lit),
            ..
        }) = &nv.value
        {
            return Ok(lit.clone());
        }
    }
    Err(syn::Error::new_spanned(
        meta,
        "expected a string literal, e.g. `tag = \"kind\"`",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    #[test]
    fn empty_arguments_parse_to_defaults() {
        let args = BridgeArgs::parse(TokenStream::new()).unwrap();
        assert!(args.error.is_none());
        assert!(args.constructor.is_none());
        assert!(args.statik.is_none());
        assert!(args.tag.is_none());
        assert!(args.rename_all.is_none());
        assert!(args.target.is_none());
        assert!(args.serde.is_none());
    }

    #[test]
    fn flags_and_values_parse() {
        let args = BridgeArgs::parse(quote!(error)).unwrap();
        assert!(args.error.is_some());

        let args = BridgeArgs::parse(quote!(constructor)).unwrap();
        assert!(args.constructor.is_some());

        let args = BridgeArgs::parse(quote!(static)).unwrap();
        assert!(args.statik.is_some());

        let args = BridgeArgs::parse(quote!(tag = "kind")).unwrap();
        assert_eq!(args.tag.unwrap().value(), "kind");

        let args = BridgeArgs::parse(quote!(rename_all = "snake_case")).unwrap();
        assert_eq!(args.rename_all.unwrap().0, RenameRule::Snake);

        let args = BridgeArgs::parse(quote!(target = "python")).unwrap();
        assert_eq!(args.target.unwrap().0, TargetArg::Python);
        let args = BridgeArgs::parse(quote!(target = "typescript")).unwrap();
        assert_eq!(args.target.unwrap().0, TargetArg::Typescript);

        let args = BridgeArgs::parse(quote!(serde)).unwrap();
        assert!(args.serde.is_some());
    }

    #[test]
    fn arguments_combine_and_allow_trailing_comma() {
        let args = BridgeArgs::parse(quote!(static, target = "python",)).unwrap();
        assert!(args.statik.is_some());
        assert_eq!(args.target.unwrap().0, TargetArg::Python);
    }

    #[test]
    fn unknown_and_malformed_arguments_are_rejected() {
        assert!(BridgeArgs::parse(quote!(frobnicate)).is_err());
        assert!(BridgeArgs::parse(quote!(error = "yes")).is_err());
        assert!(BridgeArgs::parse(quote!(tag)).is_err());
        assert!(BridgeArgs::parse(quote!(tag = 3)).is_err());
        assert_eq!(
            BridgeArgs::parse(quote!(rename_all = "PascalCase"))
                .unwrap()
                .rename_all
                .unwrap()
                .0,
            RenameRule::Pascal
        );
        assert!(BridgeArgs::parse(quote!(error, error)).is_err());
        assert!(BridgeArgs::parse(quote!(static, static)).is_err());
        // `rename` ceased to exist: it is an ordinary unknown key now.
        assert!(BridgeArgs::parse(quote!(rename = "x")).is_err());
        assert!(BridgeArgs::parse(quote!(target = "rust")).is_err());
        assert!(BridgeArgs::parse(quote!(target)).is_err());
    }

    #[test]
    fn deny_helpers_fire_only_when_set() {
        let args = BridgeArgs::parse(quote!(tag = "kind")).unwrap();
        assert!(args.deny_error("x").is_ok());
        assert!(args.deny_tag("x").is_err());
        assert!(args.deny_static("x").is_ok());
        assert!(args.deny_target("x").is_ok());

        let args = BridgeArgs::parse(quote!(static, target = "python")).unwrap();
        assert!(args.deny_static("x").is_err());
        assert!(args.deny_target("x").is_err());
    }

    #[test]
    fn bridge_attrs_on_fields_are_rejected() {
        let field: syn::Field = syn::parse_quote! {
            /// Docs are fine.
            pub chin: f64
        };
        assert!(deny_bridge_attrs(&field.attrs, "struct fields").is_ok());

        // A known-elsewhere argument is rejected as inapplicable...
        let field: syn::Field = syn::parse_quote! {
            #[bridge(error)]
            pub chin: f64
        };
        let msg = deny_bridge_attrs(&field.attrs, "struct fields")
            .unwrap_err()
            .to_string();
        assert!(msg.contains("does not apply to struct fields"), "{msg}");

        // ...while the removed `rename` reports the standard unknown key.
        let field: syn::Field = syn::parse_quote! {
            #[bridge(rename = "chin_emg")]
            pub chin: f64
        };
        let msg = deny_bridge_attrs(&field.attrs, "struct fields")
            .unwrap_err()
            .to_string();
        assert!(msg.contains("unknown #[bridge] argument"), "{msg}");
    }
}
