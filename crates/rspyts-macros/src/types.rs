use super::*;
use crate::attributes::*;

pub(super) fn params_tokens(
    inputs: &mut Punctuated<FnArg, Comma>,
) -> syn::Result<Vec<TokenStream2>> {
    let mut params = Vec::new();
    for argument in inputs {
        let FnArg::Typed(argument) = argument else {
            continue;
        };
        let name = match argument.pat.as_ref() {
            Pat::Ident(ident) => ident.ident.to_string(),
            other => {
                return Err(syn::Error::new(
                    other.span(),
                    "exported parameters must be simple identifiers",
                ));
            }
        };
        let host_name = apply_case(&name, Some("camelCase"));
        let boundary = take_boundary_attr(&mut argument.attrs)?;
        let ty = type_ref_tokens(&argument.ty, boundary.as_deref())?;
        params.push(quote!(::rspyts::ir::ParamDef {
            rust_name: #name.to_owned(),
            host_name: #host_name.to_owned(),
            ty: #ty,
        }));
    }
    Ok(params)
}

pub(super) fn return_tokens(
    output: &ReturnType,
    return_boundary: Option<&str>,
    declared_error: Option<&SynType>,
) -> syn::Result<(TokenStream2, TokenStream2)> {
    let ReturnType::Type(_, ty) = output else {
        if declared_error.is_some() {
            return Err(syn::Error::new(
                Span::call_site(),
                "`error = ...` requires a Result<T> return type",
            ));
        }
        return Ok((quote!(::rspyts::ir::TypeRef::Unit), quote!(None)));
    };
    if let Some((ok, error)) = resolved_result_types(output, declared_error)? {
        let ok = type_ref_tokens(&ok, return_boundary)?;
        return Ok((
            ok,
            quote!(Some(<#error as ::rspyts::runtime::ContractError>::type_identity())),
        ));
    }
    Ok((type_ref_tokens(ty, return_boundary)?, quote!(None)))
}

pub(super) fn field_tokens(
    field: &syn::Field,
    rename_all: Option<SerdeRenameRule>,
) -> syn::Result<TokenStream2> {
    let ident = field.ident.as_ref().expect("named field");
    let rust_name = ident.unraw().to_string();
    let serde = serde_field(&field.attrs)?;
    let docs = docs_tokens(&field.attrs);
    let options = field_options(&field.attrs)?;
    validate_field_options(field, &options, &serde)?;
    let wire_name = serde
        .rename
        .unwrap_or_else(|| apply_serde_field_case(&rust_name, rename_all));
    let ty = type_ref_tokens(&field.ty, options.boundary.as_deref())?;
    let required =
        options.required || (!is_option(&field.ty) && !serde.default && options.default.is_none());
    let default = scalar_option_tokens(options.default.as_ref());
    let literal = scalar_option_tokens(options.literal.as_ref());
    let min_length = option_u64_tokens(options.min_length);
    let max_length = option_u64_tokens(options.max_length);
    let ge = option_i64_tokens(options.ge);
    let le = option_i64_tokens(options.le);
    Ok(quote!(::rspyts::ir::FieldDef {
        rust_name: #rust_name.to_owned(),
        wire_name: #wire_name.to_owned(),
        docs: #docs,
        ty: #ty,
        required: #required,
        default: #default,
        constraints: ::rspyts::ir::FieldConstraints {
            literal: #literal,
            min_length: #min_length,
            max_length: #max_length,
            ge: #ge,
            le: #le,
        },
    }))
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ScalarValue {
    Bool(bool),
    I64(i64),
    String(String),
}

#[derive(Clone, Debug)]
struct SpannedScalar {
    value: ScalarValue,
    span: Span,
}

#[derive(Default)]
pub(super) struct FieldOptions {
    pub(super) boundary: Option<String>,
    required: bool,
    literal: Option<SpannedScalar>,
    min_length: Option<u64>,
    max_length: Option<u64>,
    ge: Option<i64>,
    le: Option<i64>,
    default: Option<SpannedScalar>,
}

pub(super) fn field_options(attrs: &[Attribute]) -> syn::Result<FieldOptions> {
    let mut options = FieldOptions::default();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("rspyts")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("buffer") || meta.path.is_ident("bytes") {
                let boundary = meta.path.get_ident().expect("field boundary").to_string();
                if options.boundary.replace(boundary).is_some() {
                    return Err(meta.error("only one field boundary may be declared"));
                }
            } else if meta.path.is_ident("required") {
                if options.required {
                    return Err(meta.error("`required` may be declared only once"));
                }
                options.required = true;
            } else if meta.path.is_ident("literal") {
                let value = parse_scalar(meta.value()?.parse::<Expr>()?)?;
                if options.literal.replace(value).is_some() {
                    return Err(meta.error("`literal` may be declared only once"));
                }
            } else if meta.path.is_ident("min_length") {
                let value = parse_u64(meta.value()?.parse::<Expr>()?, "min_length")?;
                if options.min_length.replace(value).is_some() {
                    return Err(meta.error("`min_length` may be declared only once"));
                }
            } else if meta.path.is_ident("max_length") {
                let value = parse_u64(meta.value()?.parse::<Expr>()?, "max_length")?;
                if options.max_length.replace(value).is_some() {
                    return Err(meta.error("`max_length` may be declared only once"));
                }
            } else if meta.path.is_ident("ge") {
                let expression = meta.value()?.parse::<Expr>()?;
                let span = expression.span();
                let value = parse_i64(expression, "ge")?;
                if options.ge.replace(value).is_some() {
                    return Err(syn::Error::new(span, "`ge` may be declared only once"));
                }
            } else if meta.path.is_ident("le") {
                let expression = meta.value()?.parse::<Expr>()?;
                let span = expression.span();
                let value = parse_i64(expression, "le")?;
                if options.le.replace(value).is_some() {
                    return Err(syn::Error::new(span, "`le` may be declared only once"));
                }
            } else if meta.path.is_ident("default") {
                let value = parse_scalar(meta.value()?.parse::<Expr>()?)?;
                if options.default.replace(value).is_some() {
                    return Err(meta.error("`default` may be declared only once"));
                }
            } else {
                return Err(meta.error(
                    "supported field attributes are buffer, bytes, required, literal, min_length, max_length, ge, le, and default",
                ));
            }
            Ok(())
        })?;
    }
    Ok(options)
}

fn parse_scalar(expression: Expr) -> syn::Result<SpannedScalar> {
    let span = expression.span();
    let value = match expression {
        Expr::Lit(literal) => match literal.lit {
            Lit::Bool(value) => ScalarValue::Bool(value.value),
            Lit::Int(value) => ScalarValue::I64(parse_positive_i64(&value, "scalar value")?),
            Lit::Str(value) => ScalarValue::String(value.value()),
            other => {
                return Err(syn::Error::new(
                    other.span(),
                    "rspyts scalar values must be a boolean, signed 64-bit integer, or string literal",
                ));
            }
        },
        Expr::Unary(unary) if matches!(unary.op, UnOp::Neg(_)) => {
            let Expr::Lit(literal) = *unary.expr else {
                return Err(syn::Error::new(
                    span,
                    "rspyts scalar values must be a boolean, signed 64-bit integer, or string literal",
                ));
            };
            let Lit::Int(value) = literal.lit else {
                return Err(syn::Error::new(
                    span,
                    "rspyts scalar values must be a boolean, signed 64-bit integer, or string literal",
                ));
            };
            let magnitude = value.base10_parse::<i128>()?;
            let signed = magnitude.checked_neg().ok_or_else(|| {
                syn::Error::new(value.span(), "scalar integer must fit in signed 64 bits")
            })?;
            ScalarValue::I64(i64::try_from(signed).map_err(|_| {
                syn::Error::new(value.span(), "scalar integer must fit in signed 64 bits")
            })?)
        }
        _ => {
            return Err(syn::Error::new(
                span,
                "rspyts scalar values must be a boolean, signed 64-bit integer, or string literal",
            ));
        }
    };
    Ok(SpannedScalar { value, span })
}

fn parse_positive_i64(value: &syn::LitInt, label: &str) -> syn::Result<i64> {
    let parsed = value.base10_parse::<u128>()?;
    i64::try_from(parsed)
        .map_err(|_| syn::Error::new(value.span(), format!("{label} must fit in signed 64 bits")))
}

fn parse_i64(expression: Expr, label: &str) -> syn::Result<i64> {
    let scalar = parse_scalar(expression)?;
    match scalar.value {
        ScalarValue::I64(value) => Ok(value),
        _ => Err(syn::Error::new(
            scalar.span,
            format!("`{label}` must be a signed 64-bit integer literal"),
        )),
    }
}

fn parse_u64(expression: Expr, label: &str) -> syn::Result<u64> {
    let span = expression.span();
    let Expr::Lit(literal) = expression else {
        return Err(syn::Error::new(
            span,
            format!("`{label}` must be a non-negative integer literal"),
        ));
    };
    let Lit::Int(value) = literal.lit else {
        return Err(syn::Error::new(
            span,
            format!("`{label}` must be a non-negative integer literal"),
        ));
    };
    value.base10_parse::<u64>().map_err(|_| {
        syn::Error::new(
            value.span(),
            format!("`{label}` must fit in an unsigned 64-bit integer"),
        )
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FieldKind {
    Bool,
    Integer,
    String,
    List,
    Bytes,
    Buffer,
    Unknown,
}

fn field_kind(ty: &SynType, boundary: Option<&str>) -> FieldKind {
    match boundary {
        Some("bytes") => return FieldKind::Bytes,
        Some("buffer") => return FieldKind::Buffer,
        _ => {}
    }
    let SynType::Path(path) = ty else {
        return FieldKind::Unknown;
    };
    let Some(segment) = path.path.segments.last() else {
        return FieldKind::Unknown;
    };
    if segment.ident == "Option" {
        let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
            return FieldKind::Unknown;
        };
        return arguments
            .args
            .iter()
            .find_map(|argument| match argument {
                GenericArgument::Type(ty) => Some(field_kind(ty, None)),
                _ => None,
            })
            .unwrap_or(FieldKind::Unknown);
    }
    match segment.ident.to_string().as_str() {
        "bool" => FieldKind::Bool,
        "u8" | "i8" | "u16" | "i16" | "u32" | "i32" | "u64" | "i64" => FieldKind::Integer,
        "String" | "str" => FieldKind::String,
        "Vec" => FieldKind::List,
        _ => FieldKind::Unknown,
    }
}

fn rust_default_scalar(ty: &SynType) -> Option<ScalarValue> {
    let SynType::Path(path) = ty else {
        return None;
    };
    if path.qself.is_some()
        || path
            .path
            .segments
            .iter()
            .any(|segment| !matches!(segment.arguments, PathArguments::None))
    {
        return None;
    }
    let segments = path
        .path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>();
    let primitive = match segments.as_slice() {
        [primitive] => Some(primitive.as_str()),
        [root, module, primitive]
            if matches!(root.as_str(), "core" | "std") && module == "primitive" =>
        {
            Some(primitive.as_str())
        }
        _ => None,
    };
    match primitive {
        Some("bool") => return Some(ScalarValue::Bool(false)),
        Some("u8" | "i8" | "u16" | "i16" | "u32" | "i32" | "u64" | "i64") => {
            return Some(ScalarValue::I64(0));
        }
        _ => {}
    }
    if matches!(segments.as_slice(), [name] if name == "String")
        || matches!(
            segments.as_slice(),
            [root, module, name]
                if matches!(root.as_str(), "alloc" | "std")
                    && module == "string"
                    && name == "String"
        )
    {
        return Some(ScalarValue::String(String::new()));
    }
    None
}

fn validate_field_options(
    field: &syn::Field,
    options: &FieldOptions,
    serde: &SerdeField,
) -> syn::Result<()> {
    let required =
        options.required || (!is_option(&field.ty) && !serde.default && options.default.is_none());
    if let Some(skip) = serde.skip_serializing_if.as_ref() {
        if required {
            return Err(syn::Error::new(
                skip.span(),
                "`#[serde(skip_serializing_if = ...)]` cannot be used on a required rspyts field",
            ));
        }
        if !is_option(&field.ty) {
            return Err(syn::Error::new(
                skip.span(),
                "`skip_serializing_if` is supported only on `Option<T>` rspyts fields",
            ));
        }
        if !matches!(
            skip.value().as_str(),
            "Option::is_none"
                | "std::option::Option::is_none"
                | "core::option::Option::is_none"
                | "::std::option::Option::is_none"
                | "::core::option::Option::is_none"
        ) {
            return Err(syn::Error::new(
                skip.span(),
                "rspyts supports only `Option::is_none` for `skip_serializing_if`",
            ));
        }
    }
    if let (true, Some(default)) = (options.required, options.default.as_ref()) {
        return Err(syn::Error::new(
            default.span,
            "`required` and an explicit rspyts default cannot be combined",
        ));
    }
    if serde.default {
        let Some(default) = options.default.as_ref() else {
            return Err(syn::Error::new(
                field.span(),
                "`#[serde(default)]` requires an explicit scalar `#[rspyts(default = ...)]` that exactly matches the Rust `Default` value",
            ));
        };
        let Some(rust_default) = rust_default_scalar(&field.ty) else {
            return Err(syn::Error::new(
                field.ty.span(),
                "`#[serde(default)]` is supported only for direct bool, integer, and String carriers; defaults such as `Option::None` cannot be represented",
            ));
        };
        if default.value != rust_default {
            return Err(syn::Error::new(
                default.span,
                "`#[rspyts(default = ...)]` must exactly match the carrier's Rust `Default` value when `#[serde(default)]` is present",
            ));
        }
    }
    if let (Some(minimum), Some(maximum)) = (options.min_length, options.max_length)
        && minimum > maximum
    {
        return Err(syn::Error::new(
            field.span(),
            "`min_length` cannot exceed `max_length`",
        ));
    }
    if let (Some(minimum), Some(maximum)) = (options.ge, options.le)
        && minimum > maximum
    {
        return Err(syn::Error::new(field.span(), "`ge` cannot exceed `le`"));
    }
    let kind = field_kind(&field.ty, options.boundary.as_deref());
    if (options.min_length.is_some() || options.max_length.is_some())
        && !matches!(
            kind,
            FieldKind::String | FieldKind::List | FieldKind::Unknown
        )
    {
        return Err(syn::Error::new(
            field.ty.span(),
            "`min_length` and `max_length` apply only to string or list fields",
        ));
    }
    if (options.ge.is_some() || options.le.is_some())
        && !matches!(kind, FieldKind::Integer | FieldKind::Unknown)
    {
        return Err(syn::Error::new(
            field.ty.span(),
            "`ge` and `le` apply only to integer fields",
        ));
    }
    if let Some(literal) = options.literal.as_ref() {
        validate_scalar_kind(literal, kind, "literal")?;
    }
    if let Some(default) = options.default.as_ref() {
        validate_scalar_kind(default, kind, "default")?;
    }
    if let (Some(literal), Some(default)) = (options.literal.as_ref(), options.default.as_ref())
        && literal.value != default.value
    {
        return Err(syn::Error::new(
            default.span,
            "an explicit default must equal the field's `literal` constraint",
        ));
    }
    if let Some(minimum) = options.ge {
        for (label, scalar) in [
            ("literal", options.literal.as_ref()),
            ("default", options.default.as_ref()),
        ] {
            if let Some(SpannedScalar {
                value: ScalarValue::I64(value),
                span,
            }) = scalar
                && *value < minimum
            {
                return Err(syn::Error::new(
                    *span,
                    format!("the field's `{label}` value is below its `ge` constraint"),
                ));
            }
        }
    }
    if let Some(maximum) = options.le {
        for (label, scalar) in [
            ("literal", options.literal.as_ref()),
            ("default", options.default.as_ref()),
        ] {
            if let Some(SpannedScalar {
                value: ScalarValue::I64(value),
                span,
            }) = scalar
                && *value > maximum
            {
                return Err(syn::Error::new(
                    *span,
                    format!("the field's `{label}` value is above its `le` constraint"),
                ));
            }
        }
    }
    Ok(())
}

fn validate_scalar_kind(value: &SpannedScalar, kind: FieldKind, label: &str) -> syn::Result<()> {
    let valid = matches!(kind, FieldKind::Unknown)
        || matches!(
            (&value.value, kind),
            (ScalarValue::Bool(_), FieldKind::Bool)
                | (ScalarValue::I64(_), FieldKind::Integer)
                | (ScalarValue::String(_), FieldKind::String)
        );
    if valid {
        Ok(())
    } else {
        Err(syn::Error::new(
            value.span,
            format!("the `{label}` scalar does not match this field's Rust type"),
        ))
    }
}

fn scalar_option_tokens(value: Option<&SpannedScalar>) -> TokenStream2 {
    match value.map(|value| &value.value) {
        Some(ScalarValue::Bool(value)) => {
            quote!(Some(::rspyts::ir::ScalarValue::Bool(#value)))
        }
        Some(ScalarValue::I64(value)) => {
            quote!(Some(::rspyts::ir::ScalarValue::I64(#value)))
        }
        Some(ScalarValue::String(value)) => {
            quote!(Some(::rspyts::ir::ScalarValue::String(#value.to_owned())))
        }
        None => quote!(None),
    }
}

fn option_u64_tokens(value: Option<u64>) -> TokenStream2 {
    value.map_or_else(|| quote!(None), |value| quote!(Some(#value)))
}

fn option_i64_tokens(value: Option<i64>) -> TokenStream2 {
    value.map_or_else(|| quote!(None), |value| quote!(Some(#value)))
}

pub(super) fn type_ref_tokens(ty: &SynType, boundary: Option<&str>) -> syn::Result<TokenStream2> {
    let fixed_bytes = fixed_byte_array_length(ty)?;
    match boundary {
        Some("bytes") if fixed_bytes.is_some() => {
            let length = fixed_bytes.expect("checked fixed byte array");
            Ok(quote!(::rspyts::ir::TypeRef::FixedBytes {
                length: <::core::primitive::u64 as ::core::convert::TryFrom<
                    ::core::primitive::usize
                >>::try_from(::core::mem::size_of::<[::core::primitive::u8; #length]>())
                    .expect("fixed byte length must fit in the rspyts IR"),
            }))
        }
        Some("bytes") => {
            validate_bytes_type(ty)?;
            Ok(quote!(::rspyts::ir::TypeRef::Bytes))
        }
        Some("buffer") => {
            let scalar = sequence_scalar(ty).ok_or_else(|| {
                syn::Error::new(
                    ty.span(),
                    "`buffer` requires Vec<T> or &[T] with a numeric scalar",
                )
            })?;
            let element = match type_last_ident(scalar)?.to_string().as_str() {
                "u8" => quote!(U8),
                "i8" => quote!(I8),
                "u16" => quote!(U16),
                "i16" => quote!(I16),
                "u32" => quote!(U32),
                "i32" => quote!(I32),
                "u64" => quote!(U64),
                "i64" => quote!(I64),
                "f32" => quote!(F32),
                "f64" => quote!(F64),
                _ => {
                    return Err(syn::Error::new(
                        scalar.span(),
                        "unsupported numeric buffer scalar",
                    ));
                }
            };
            Ok(quote!(::rspyts::ir::TypeRef::Buffer {
                element: ::rspyts::ir::BufferElement::#element,
            }))
        }
        Some(other) => Err(syn::Error::new(
            ty.span(),
            format!("unknown boundary attribute `{other}`"),
        )),
        None if fixed_bytes.is_some() => Err(syn::Error::new(
            ty.span(),
            "fixed byte arrays require `#[rspyts(bytes)]` or `#[rspyts(returns(bytes))]`",
        )),
        None => Ok(quote!(<#ty as ::rspyts::ContractType>::type_ref())),
    }
}

fn validate_bytes_type(ty: &SynType) -> syn::Result<()> {
    if fixed_byte_array_length(ty)?.is_some() || is_owned_bytes(ty) || is_borrowed_byte_slice(ty) {
        Ok(())
    } else {
        Err(syn::Error::new(
            ty.span(),
            "`bytes` requires exactly `Vec<u8>`, `[u8; N]`, `&[u8]`, or `&[u8; N]`",
        ))
    }
}

fn is_owned_bytes(ty: &SynType) -> bool {
    let SynType::Path(path) = ty else {
        return false;
    };
    if path.qself.is_some() || !is_vec_path(&path.path) {
        return false;
    }
    let Some(segment) = path.path.segments.last() else {
        return false;
    };
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return false;
    };
    let mut arguments = arguments.args.iter();
    matches!(arguments.next(), Some(GenericArgument::Type(ty)) if is_u8(ty))
        && arguments.next().is_none()
}

fn is_borrowed_byte_slice(ty: &SynType) -> bool {
    let SynType::Reference(reference) = ty else {
        return false;
    };
    if reference.mutability.is_some() {
        return false;
    }
    matches!(reference.elem.as_ref(), SynType::Slice(slice) if is_u8(&slice.elem))
}

fn fixed_byte_array_length(ty: &SynType) -> syn::Result<Option<&Expr>> {
    let ty = match ty {
        SynType::Reference(reference) if reference.mutability.is_none() => reference.elem.as_ref(),
        ty => ty,
    };
    let SynType::Array(array) = ty else {
        return Ok(None);
    };
    if !is_u8(&array.elem) {
        return Err(syn::Error::new(
            array.elem.span(),
            "fixed rspyts arrays support only the byte element type `u8`",
        ));
    }
    Ok(Some(&array.len))
}

fn is_vec_path(path: &syn::Path) -> bool {
    let segments = path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>();
    match segments.as_slice() {
        [name] => name == "Vec",
        [root, module, name] => {
            (root == "std" || root == "alloc") && module == "vec" && name == "Vec"
        }
        _ => false,
    }
}

fn is_u8(ty: &SynType) -> bool {
    let SynType::Path(path) = ty else {
        return false;
    };
    if path.qself.is_some() {
        return false;
    }
    let segments = path
        .path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>();
    let is_primitive = match segments.as_slice() {
        [name] => name == "u8",
        [root, module, name] => {
            (root == "std" || root == "core") && module == "primitive" && name == "u8"
        }
        _ => false,
    };
    is_primitive
        && path
            .path
            .segments
            .iter()
            .all(|segment| matches!(segment.arguments, PathArguments::None))
}

fn sequence_scalar(ty: &SynType) -> Option<&SynType> {
    match ty {
        SynType::Reference(reference) => sequence_scalar(&reference.elem),
        SynType::Slice(slice) => Some(&slice.elem),
        SynType::Path(path) => {
            let segment = path.path.segments.last()?;
            if segment.ident != "Vec" {
                return None;
            }
            let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
                return None;
            };
            arguments.args.iter().find_map(|argument| match argument {
                GenericArgument::Type(ty) => Some(ty),
                _ => None,
            })
        }
        _ => None,
    }
}

pub(super) fn resolved_result_types(
    output: &ReturnType,
    declared_error: Option<&SynType>,
) -> syn::Result<Option<(SynType, SynType)>> {
    let ReturnType::Type(_, ty) = output else {
        return Ok(None);
    };
    let SynType::Path(TypePath { path, .. }) = ty.as_ref() else {
        if declared_error.is_some() {
            return Err(syn::Error::new(
                ty.span(),
                "`error = ...` requires a Result<T> return type",
            ));
        }
        return Ok(None);
    };
    let Some(segment) = path.segments.last() else {
        return Ok(None);
    };
    if segment.ident != "Result" {
        if declared_error.is_some() {
            return Err(syn::Error::new(
                ty.span(),
                "`error = ...` requires a Result<T> return type",
            ));
        }
        return Ok(None);
    }
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return Err(syn::Error::new(
            segment.span(),
            "Result must declare its success type",
        ));
    };
    let types = arguments
        .args
        .iter()
        .filter_map(|argument| match argument {
            GenericArgument::Type(ty) => Some(ty.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    match (types.as_slice(), declared_error) {
        ([ok, error], None) => Ok(Some((ok.clone(), error.clone()))),
        ([ok], Some(error)) => Ok(Some((ok.clone(), error.clone()))),
        ([_, _], Some(_)) => Err(syn::Error::new(
            segment.span(),
            "`error = ...` is only valid for a one-parameter Result<T> alias",
        )),
        ([..], None) => Err(syn::Error::new(
            segment.span(),
            "a one-parameter Result<T> alias requires `#[rspyts(error = crate::Error)]`",
        )),
        _ => Err(syn::Error::new(
            segment.span(),
            "Result must contain one success type and either an inline or declared error type",
        )),
    }
}

fn is_option(ty: &SynType) -> bool {
    matches!(
        ty,
        SynType::Path(path) if path.path.segments.last().is_some_and(|segment| segment.ident == "Option")
    )
}

pub(super) fn ensure_public(visibility: &syn::Visibility, span: Span) -> syn::Result<()> {
    if matches!(visibility, syn::Visibility::Public(_)) {
        Ok(())
    } else {
        Err(syn::Error::new(
            span,
            "exported rspyts items must be public",
        ))
    }
}

pub(super) fn reject_generics(generics: &syn::Generics, span: Span) -> syn::Result<()> {
    if generics.params.is_empty() && generics.where_clause.is_none() {
        Ok(())
    } else {
        Err(syn::Error::new(
            span,
            "generic rspyts contracts are not supported",
        ))
    }
}

pub(super) fn reject_signature(signature: &syn::Signature) -> syn::Result<()> {
    reject_generics(&signature.generics, signature.ident.span())?;
    for argument in &signature.inputs {
        let FnArg::Typed(argument) = argument else {
            continue;
        };
        let Pat::Ident(pattern) = argument.pat.as_ref() else {
            continue;
        };
        let name = pattern.ident.unraw().to_string();
        if name.starts_with("__rspyts_") {
            return Err(syn::Error::new(
                pattern.ident.span(),
                format!(
                    "parameter `{name}` uses the reserved `__rspyts_` prefix; rename it because that namespace belongs to generated rspyts wrapper bindings"
                ),
            ));
        }
    }
    if signature.asyncness.is_some() {
        return Err(syn::Error::new(
            signature.span(),
            "async exports are not supported in v1.0",
        ));
    }
    if signature.unsafety.is_some() || signature.variadic.is_some() {
        return Err(syn::Error::new(
            signature.span(),
            "unsafe and variadic exports are not supported",
        ));
    }
    Ok(())
}

pub(super) fn reject_reserved_resource_method(method: &ImplItemFn) -> syn::Result<()> {
    let name = method.sig.ident.to_string();
    if matches!(name.as_str(), "close" | "free") {
        return Err(syn::Error::new(
            method.sig.ident.span(),
            format!("`{name}` is reserved for generated resource lifecycle behavior"),
        ));
    }
    Ok(())
}

pub(super) fn wasm_native_host_name(host_name: &str) -> String {
    format!("__rspyts_export_{host_name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_field_constraints() {
        let field: syn::Field = syn::parse_quote! {
            #[rspyts(ge = 1, le = 100)]
            count: u32
        };
        let options = field_options(&field.attrs).unwrap();
        assert_eq!(options.ge, Some(1));
        assert_eq!(options.le, Some(100));
    }

    #[test]
    fn rejects_async_exports() {
        let function: ItemFn = syn::parse_quote! {
            pub async fn fetch() {}
        };
        let error = reject_signature(&function.sig).unwrap_err();
        assert!(error.to_string().contains("async exports"));
    }
}
