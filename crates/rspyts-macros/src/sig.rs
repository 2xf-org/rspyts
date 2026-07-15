//! Syntactic analysis of bridged signatures.
//!
//! Everything here is deliberately *syntactic* (ABI §3.1): a parameter is
//! a slice parameter iff its element is one of the ten ABI numeric dtypes;
//! a return type is fallible iff it is written
//! literally as `Result<T, E>`. Semantic membership in the portable type
//! system is enforced later by the `Bridged` trait bounds the emitted code
//! places on every plain type — a non-bridgeable type produces a
//! "trait bound not satisfied" error at the definition site.

use std::collections::BTreeSet;
use syn::parse_quote;

/// How a plain parameter is passed to the original function from the
/// owned value deserialized into the args struct.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Borrow {
    /// Written as `T` — moved out of the args struct.
    Owned,
    /// Written as `&T` — re-borrowed as `&args.field`.
    Ref,
    /// Written as `&str` — stored as `String`, passed as `.as_str()`.
    Str,
}

/// Classification of one bridged parameter (ABI §3.1).
pub enum ParamKind {
    /// A raw numeric slice, passed as a `(ptr, len)` C argument pair.
    Slice {
        /// The element type as written (`u8` … `f64`).
        elem: syn::Type,
        /// The matching `ir::Dtype` variant identifier (`U8` … `F64`).
        dtype: syn::Ident,
    },
    /// Everything else: carried in the JSON args object.
    Plain { owned: syn::Type, borrow: Borrow },
}

pub struct BridgedParam {
    pub ident: syn::Ident,
    pub kind: ParamKind,
}

/// Check the JSON keys produced from a function's plain parameters.
pub fn validate_param_wire_names(params: &[BridgedParam]) -> syn::Result<()> {
    let mut seen = BTreeSet::new();
    for param in params {
        if matches!(param.kind, ParamKind::Slice { .. }) {
            continue;
        }
        let wire = crate::casing::field_wire_name(
            &param.ident.to_string(),
            crate::casing::RenameRule::Camel,
        );
        if wire == "__proto__" {
            return Err(syn::Error::new_spanned(
                &param.ident,
                format!("parameter wire name `{wire}` is unsafe for JavaScript object projection"),
            ));
        }
        if !seen.insert(wire.clone()) {
            return Err(syn::Error::new_spanned(
                &param.ident,
                format!("duplicate parameter wire name `{wire}` after camelCase projection"),
            ));
        }
    }
    Ok(())
}

/// Classification of a bridged return type.
pub enum RetKind {
    /// No return type (or a literal `()`); serializes as `null`.
    Unit,
    /// An infallible return type.
    Plain(syn::Type),
    /// Written literally as `Result<T, E>`.
    Result {
        ok: syn::Type,
        /// Concrete error type. Its `BridgeErr::inventory_name()` carries
        /// exact origin identity for macro-generated error enums.
        err: Box<syn::Type>,
    },
}

impl RetKind {
    pub fn is_result(&self) -> bool {
        matches!(self, RetKind::Result { .. })
    }
}

/// Analyze the non-receiver inputs of a bridged function or method.
pub fn bridged_params<'a>(
    inputs: impl Iterator<Item = &'a syn::FnArg>,
) -> syn::Result<Vec<BridgedParam>> {
    let mut params = Vec::new();
    for input in inputs {
        let typed = match input {
            syn::FnArg::Receiver(receiver) => {
                return Err(syn::Error::new_spanned(
                    receiver,
                    "unexpected `self` parameter here",
                ));
            }
            syn::FnArg::Typed(typed) => typed,
        };
        let ident = match &*typed.pat {
            syn::Pat::Ident(pat) if pat.subpat.is_none() => pat.ident.clone(),
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "bridged parameters must be plain identifiers \
                     (the name becomes the wire key)",
                ));
            }
        };
        let kind = classify_type(&typed.ty)?;
        params.push(BridgedParam { ident, kind });
    }
    Ok(params)
}

/// Classify one parameter type (ABI §3.1). Purely syntactic.
pub fn classify_type(ty: &syn::Type) -> syn::Result<ParamKind> {
    match ty {
        syn::Type::Reference(reference) => {
            if reference.mutability.is_some() {
                return Err(syn::Error::new_spanned(
                    ty,
                    "`&mut` parameters are not supported; \
                     parameters cross the boundary by value",
                ));
            }
            match &*reference.elem {
                syn::Type::Slice(slice) => match slice_dtype(&slice.elem) {
                    Some(dtype) => Ok(ParamKind::Slice {
                        elem: (*slice.elem).clone(),
                        dtype,
                    }),
                    None => Err(syn::Error::new_spanned(
                        ty,
                        "only slices of ABI numeric dtypes (`u8`, `i8`, `u16`, `i16`, \
                         `u32`, `i32`, `u64`, `i64`, `f32`, `f64`) are supported as \
                         slice parameters; use `Vec<T>` for other element \
                         types (docs/design/type-system.md §5)",
                    )),
                },
                syn::Type::Path(path) if path.qself.is_none() && path.path.is_ident("str") => {
                    Ok(ParamKind::Plain {
                        owned: parse_quote!(::std::string::String),
                        borrow: Borrow::Str,
                    })
                }
                inner => Ok(ParamKind::Plain {
                    owned: inner.clone(),
                    borrow: Borrow::Ref,
                }),
            }
        }
        syn::Type::Slice(_) => Err(syn::Error::new_spanned(
            ty,
            "bare slice types are not valid parameters; write `&[T]`",
        )),
        other => Ok(ParamKind::Plain {
            owned: other.clone(),
            borrow: Borrow::Owned,
        }),
    }
}

/// Classify a return type. Purely syntactic (`Result<T, E>` literal).
pub fn classify_ret(output: &syn::ReturnType) -> RetKind {
    let ty = match output {
        syn::ReturnType::Default => return RetKind::Unit,
        syn::ReturnType::Type(_, ty) => &**ty,
    };
    if let syn::Type::Tuple(tuple) = ty {
        if tuple.elems.is_empty() {
            return RetKind::Unit;
        }
    }
    if let Some((ok, err)) = result_parts(ty) {
        return RetKind::Result {
            ok: ok.clone(),
            err: Box::new(err.clone()),
        };
    }
    RetKind::Plain(ty.clone())
}

/// `Some((T, E))` when `ty` is written literally as `Result<T, E>`
/// (any path spelling ending in `Result` with exactly two type arguments).
pub fn result_parts(ty: &syn::Type) -> Option<(&syn::Type, &syn::Type)> {
    let syn::Type::Path(path) = ty else {
        return None;
    };
    if path.qself.is_some() {
        return None;
    }
    let last = path.path.segments.last()?;
    if last.ident != "Result" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &last.arguments else {
        return None;
    };
    let types: Vec<&syn::Type> = args
        .args
        .iter()
        .filter_map(|arg| match arg {
            syn::GenericArgument::Type(ty) => Some(ty),
            _ => None,
        })
        .collect();
    match (types.len(), args.args.len()) {
        (2, 2) => Some((types[0], types[1])),
        _ => None,
    }
}

/// True iff `ty` is written syntactically as a direct `Option<…>`.
/// Such object fields are omittable unless marked `#[bridge(required)]`.
pub fn is_option(ty: &syn::Type) -> bool {
    let syn::Type::Path(path) = ty else {
        return false;
    };
    if path.qself.is_some() {
        return false;
    }
    let Some(segment) = path.path.segments.last() else {
        return false;
    };
    let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
        return false;
    };
    segment.ident == "Option"
        && args.args.len() == 1
        && matches!(args.args.first(), Some(syn::GenericArgument::Type(_)))
}

/// Reject values that require an envelope attachment in positions whose
/// wire format has no raw tail (constants and application-error data).
pub fn reject_attachment_type(ty: &syn::Type, position: &str) -> syn::Result<()> {
    fn attachment(ty: &syn::Type) -> Option<&syn::Type> {
        match ty {
            syn::Type::Path(path) => {
                let last = path.path.segments.last()?;
                if last.ident == "Buf" || last.ident == "Bytes" {
                    return Some(ty);
                }
                let syn::PathArguments::AngleBracketed(args) = &last.arguments else {
                    return None;
                };
                args.args.iter().find_map(|arg| match arg {
                    syn::GenericArgument::Type(inner) => attachment(inner),
                    _ => None,
                })
            }
            syn::Type::Tuple(tuple) => tuple.elems.iter().find_map(attachment),
            syn::Type::Array(array) => attachment(&array.elem),
            syn::Type::Reference(reference) => attachment(&reference.elem),
            syn::Type::Paren(paren) => attachment(&paren.elem),
            syn::Type::Group(group) => attachment(&group.elem),
            _ => None,
        }
    }

    if let Some(found) = attachment(ty) {
        Err(syn::Error::new_spanned(
            found,
            format!(
                "`Buf` and `Bytes` cannot appear in {position} because that wire format has no attachment tail"
            ),
        ))
    } else {
        Ok(())
    }
}

/// True iff `ty` is `Self` or the bare class name — the literal forms a
/// constructor may return (type-system §7).
pub fn is_self_ty(ty: &syn::Type, self_ident: &syn::Ident) -> bool {
    let syn::Type::Path(path) = ty else {
        return false;
    };
    if path.qself.is_some() || path.path.segments.len() != 1 {
        return false;
    }
    let segment = &path.path.segments[0];
    segment.arguments.is_none() && (segment.ident == "Self" || &segment.ident == self_ident)
}

/// Reject generic parameters and where clauses on any bridged item.
pub fn ensure_no_generics(generics: &syn::Generics, what: &str) -> syn::Result<()> {
    if let Some(param) = generics.params.first() {
        return Err(syn::Error::new_spanned(
            param,
            format!(
                "bridged {what} cannot be generic — no type parameters, lifetimes, or \
                 const generics (docs/design/type-system.md §9)"
            ),
        ));
    }
    if let Some(clause) = &generics.where_clause {
        return Err(syn::Error::new_spanned(
            clause,
            format!("bridged {what} cannot have a where clause"),
        ));
    }
    Ok(())
}

/// Reject the common pre-shim mistakes shared by free functions and
/// methods: `async`, `unsafe`, generics, variadics.
pub fn ensure_plain_signature(sig: &syn::Signature, what: &str) -> syn::Result<()> {
    if let Some(token) = &sig.asyncness {
        return Err(syn::Error::new_spanned(
            token,
            "`async fn` cannot be bridged — ABI 3 has no async support \
             (docs/design/abi.md)",
        ));
    }
    if let Some(token) = &sig.unsafety {
        return Err(syn::Error::new_spanned(
            token,
            "`unsafe fn` cannot be bridged; wrap the unsafety in a safe function",
        ));
    }
    if let Some(variadic) = &sig.variadic {
        return Err(syn::Error::new_spanned(
            variadic,
            "variadic functions cannot be bridged",
        ));
    }
    ensure_no_generics(&sig.generics, what)
}

fn slice_dtype(elem: &syn::Type) -> Option<syn::Ident> {
    let syn::Type::Path(path) = elem else {
        return None;
    };
    if path.qself.is_some() {
        return None;
    }
    let ident = path.path.get_ident()?;
    let variant = match ident.to_string().as_str() {
        "u8" => "U8",
        "i8" => "I8",
        "u16" => "U16",
        "i16" => "I16",
        "u32" => "U32",
        "i32" => "I32",
        "u64" => "U64",
        "i64" => "I64",
        "f32" => "F32",
        "f64" => "F64",
        _ => return None,
    };
    Some(syn::Ident::new(variant, proc_macro2::Span::call_site()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::ToTokens;

    fn classify(ty: syn::Type) -> syn::Result<ParamKind> {
        classify_type(&ty)
    }

    #[test]
    fn dtype_slices_classify_as_slice_params() {
        for (ty, variant) in [
            (parse_quote!(&[u8]), "U8"),
            (parse_quote!(&[i8]), "I8"),
            (parse_quote!(&[u16]), "U16"),
            (parse_quote!(&[i16]), "I16"),
            (parse_quote!(&[u32]), "U32"),
            (parse_quote!(&[i32]), "I32"),
            (parse_quote!(&[u64]), "U64"),
            (parse_quote!(&[i64]), "I64"),
            (parse_quote!(&[f32]), "F32"),
            (parse_quote!(&[f64]), "F64"),
        ] {
            match classify(ty).unwrap() {
                ParamKind::Slice { dtype, .. } => assert_eq!(dtype.to_string(), variant),
                ParamKind::Plain { .. } => panic!("expected slice"),
            }
        }
    }

    #[test]
    fn non_dtype_slices_are_rejected() {
        assert!(classify(parse_quote!(&[String])).is_err());
        assert!(classify(parse_quote!(&[bool])).is_err());
    }

    #[test]
    fn str_ref_becomes_owned_string() {
        match classify(parse_quote!(&str)).unwrap() {
            ParamKind::Plain { owned, borrow } => {
                assert_eq!(borrow, Borrow::Str);
                assert_eq!(
                    owned.to_token_stream().to_string().replace(' ', ""),
                    "::std::string::String"
                );
            }
            ParamKind::Slice { .. } => panic!("expected plain"),
        }
    }

    #[test]
    fn shared_refs_strip_one_level_and_owned_stay_owned() {
        match classify(parse_quote!(&QueryOptions)).unwrap() {
            ParamKind::Plain { owned, borrow } => {
                assert_eq!(borrow, Borrow::Ref);
                assert_eq!(owned.to_token_stream().to_string(), "QueryOptions");
            }
            ParamKind::Slice { .. } => panic!("expected plain"),
        }
        match classify(parse_quote!(Vec<f64>)).unwrap() {
            ParamKind::Plain { borrow, .. } => assert_eq!(borrow, Borrow::Owned),
            ParamKind::Slice { .. } => panic!("expected plain"),
        }
    }

    #[test]
    fn mut_refs_are_rejected_and_buf_params_are_plain_owned_values() {
        assert!(classify(parse_quote!(&mut Foo)).is_err());
        assert!(matches!(
            classify(parse_quote!(Buf<f64>)).unwrap(),
            ParamKind::Plain {
                borrow: Borrow::Owned,
                ..
            }
        ));
        assert!(matches!(
            classify(parse_quote!(&Buf<f64>)).unwrap(),
            ParamKind::Plain {
                borrow: Borrow::Ref,
                ..
            }
        ));
        assert!(classify(parse_quote!(rspyts::Buf<f64>)).is_ok());
    }

    #[test]
    fn return_classification_is_literal() {
        assert!(matches!(
            classify_ret(&syn::ReturnType::Default),
            RetKind::Unit
        ));
        assert!(matches!(classify_ret(&parse_quote!(-> ())), RetKind::Unit));
        assert!(matches!(
            classify_ret(&parse_quote!(-> ProcessingReport)),
            RetKind::Plain(_)
        ));

        match classify_ret(&parse_quote!(-> Result<ProcessingReport, QueryError>)) {
            RetKind::Result { err, .. } => assert_eq!(err.as_ref(), &parse_quote!(QueryError)),
            _ => panic!("expected result"),
        }
        match classify_ret(&parse_quote!(-> std::result::Result<u32, errors::MyError>)) {
            RetKind::Result { err, .. } => {
                assert_eq!(err.as_ref(), &parse_quote!(errors::MyError))
            }
            _ => panic!("expected result"),
        }
        // A one-argument `Result` alias is not the literal form.
        assert!(matches!(
            classify_ret(&parse_quote!(-> Result<u32>)),
            RetKind::Plain(_)
        ));
    }

    #[test]
    fn option_detection_is_syntactic() {
        assert!(is_option(&parse_quote!(Option<f64>)));
        assert!(is_option(&parse_quote!(std::option::Option<f64>)));
        assert!(!is_option(&parse_quote!(Option<f64, f32>)));
        assert!(!is_option(&parse_quote!(Vec<Option<f64>>)));
        assert!(!is_option(&parse_quote!(f64)));
    }

    #[test]
    fn self_ty_detection() {
        let ident: syn::Ident = parse_quote!(RunningStats);
        assert!(is_self_ty(&parse_quote!(Self), &ident));
        assert!(is_self_ty(&parse_quote!(RunningStats), &ident));
        assert!(!is_self_ty(&parse_quote!(Other), &ident));
        assert!(!is_self_ty(&parse_quote!(Box<Self>), &ident));
    }
}
