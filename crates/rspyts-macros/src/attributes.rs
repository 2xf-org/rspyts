use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SerdeRenameRule {
    Lower,
    Upper,
    Pascal,
    Camel,
    Snake,
    ScreamingSnake,
    Kebab,
    ScreamingKebab,
}

impl SerdeRenameRule {
    fn parse(value: &LitStr) -> syn::Result<Self> {
        match value.value().as_str() {
            "lowercase" => Ok(Self::Lower),
            "UPPERCASE" => Ok(Self::Upper),
            "PascalCase" => Ok(Self::Pascal),
            "camelCase" => Ok(Self::Camel),
            "snake_case" => Ok(Self::Snake),
            "SCREAMING_SNAKE_CASE" => Ok(Self::ScreamingSnake),
            "kebab-case" => Ok(Self::Kebab),
            "SCREAMING-KEBAB-CASE" => Ok(Self::ScreamingKebab),
            other => Err(syn::Error::new(
                value.span(),
                format!(
                    "unknown serde rename rule `{other}`; expected lowercase, UPPERCASE, PascalCase, camelCase, snake_case, SCREAMING_SNAKE_CASE, kebab-case, or SCREAMING-KEBAB-CASE"
                ),
            )),
        }
    }
}

pub(super) fn apply_serde_variant_case(value: &str, rule: Option<SerdeRenameRule>) -> String {
    let Some(rule) = rule else {
        return value.to_owned();
    };
    match rule {
        SerdeRenameRule::Lower => value.to_ascii_lowercase(),
        SerdeRenameRule::Upper => value.to_ascii_uppercase(),
        SerdeRenameRule::Pascal => value.to_owned(),
        SerdeRenameRule::Camel => value[..1].to_ascii_lowercase() + &value[1..],
        SerdeRenameRule::Snake => {
            let mut snake = String::new();
            for (index, character) in value.char_indices() {
                if index > 0 && character.is_uppercase() {
                    snake.push('_');
                }
                snake.push(character.to_ascii_lowercase());
            }
            snake
        }
        SerdeRenameRule::ScreamingSnake => {
            apply_serde_variant_case(value, Some(SerdeRenameRule::Snake)).to_ascii_uppercase()
        }
        SerdeRenameRule::Kebab => {
            apply_serde_variant_case(value, Some(SerdeRenameRule::Snake)).replace('_', "-")
        }
        SerdeRenameRule::ScreamingKebab => {
            apply_serde_variant_case(value, Some(SerdeRenameRule::ScreamingSnake)).replace('_', "-")
        }
    }
}

pub(super) fn apply_serde_field_case(value: &str, rule: Option<SerdeRenameRule>) -> String {
    let Some(rule) = rule else {
        return value.to_owned();
    };
    match rule {
        SerdeRenameRule::Lower | SerdeRenameRule::Snake => value.to_owned(),
        SerdeRenameRule::Upper => value.to_ascii_uppercase(),
        SerdeRenameRule::Pascal => {
            let mut pascal = String::new();
            let mut capitalize = true;
            for character in value.chars() {
                if character == '_' {
                    capitalize = true;
                } else if capitalize {
                    pascal.push(character.to_ascii_uppercase());
                    capitalize = false;
                } else {
                    pascal.push(character);
                }
            }
            pascal
        }
        SerdeRenameRule::Camel => {
            let pascal = apply_serde_field_case(value, Some(SerdeRenameRule::Pascal));
            pascal[..1].to_ascii_lowercase() + &pascal[1..]
        }
        SerdeRenameRule::ScreamingSnake => value.to_ascii_uppercase(),
        SerdeRenameRule::Kebab => value.replace('_', "-"),
        SerdeRenameRule::ScreamingKebab => value.to_ascii_uppercase().replace('_', "-"),
    }
}

#[derive(Default)]
pub(super) struct SerdeContainer {
    pub(super) rename_all: Option<SerdeRenameRule>,
    pub(super) rename_all_fields: Option<SerdeRenameRule>,
    pub(super) tag: Option<String>,
    pub(super) transparent: bool,
}

pub(super) fn serde_container(attrs: &[Attribute]) -> syn::Result<SerdeContainer> {
    let mut result = SerdeContainer::default();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("serde")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename_all") {
                if result.rename_all.is_some() {
                    return Err(meta.error("`rename_all` may be declared only once"));
                }
                if !meta.input.peek(Token![=]) {
                    return Err(meta.error(
                        "directional `rename_all(serialize = ..., deserialize = ...)` is not supported because rspyts requires one host name",
                    ));
                }
                result.rename_all =
                    Some(SerdeRenameRule::parse(&meta.value()?.parse::<LitStr>()?)?);
            } else if meta.path.is_ident("rename_all_fields") {
                if result.rename_all_fields.is_some() {
                    return Err(meta.error("`rename_all_fields` may be declared only once"));
                }
                if !meta.input.peek(Token![=]) {
                    return Err(meta.error(
                        "directional `rename_all_fields(serialize = ..., deserialize = ...)` is not supported because rspyts requires one host name",
                    ));
                }
                result.rename_all_fields =
                    Some(SerdeRenameRule::parse(&meta.value()?.parse::<LitStr>()?)?);
            } else if meta.path.is_ident("tag") {
                result.tag = Some(meta.value()?.parse::<LitStr>()?.value());
            } else if meta.path.is_ident("transparent") {
                result.transparent = true;
            } else if meta.path.is_ident("deny_unknown_fields") {
                // The generated hosts are closed by default; this is accepted metadata.
            } else if meta.path.is_ident("rename") {
                if !meta.input.peek(Token![=]) {
                    return Err(meta.error(
                        "directional `rename(serialize = ..., deserialize = ...)` is not supported because rspyts requires one host name",
                    ));
                }
                let _ = meta.value()?.parse::<LitStr>()?;
            } else {
                return Err(
                    meta.error("unsupported serde container attribute in an rspyts contract")
                );
            }
            Ok(())
        })?;
    }
    Ok(result)
}

pub(super) fn serde_rename(attrs: &[Attribute]) -> syn::Result<Option<String>> {
    let mut value = None;
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("serde")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                if value.is_some() {
                    return Err(meta.error("`rename` may be declared only once"));
                }
                if !meta.input.peek(Token![=]) {
                    return Err(meta.error(
                        "directional `rename(serialize = ..., deserialize = ...)` is not supported because rspyts requires one host name",
                    ));
                }
                value = Some(meta.value()?.parse::<LitStr>()?.value());
            } else if meta.path.is_ident("rename_all") {
                return Err(meta.error(
                    "variant-level `rename_all` is not supported; use container `rename_all_fields`",
                ));
            } else if meta.path.is_ident("alias") {
                return Err(meta.error(
                    "`#[serde(alias = ...)]` is not supported because rspyts exposes one host name",
                ));
            } else {
                return Err(meta.error("unsupported serde field or variant attribute"));
            }
            Ok(())
        })?;
    }
    Ok(value)
}

#[derive(Default)]
pub(super) struct SerdeField {
    pub(super) rename: Option<String>,
    pub(super) default: bool,
    pub(super) skip_serializing_if: Option<LitStr>,
}

pub(super) fn serde_field(attrs: &[Attribute]) -> syn::Result<SerdeField> {
    let mut result = SerdeField::default();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("serde")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                if result.rename.is_some() {
                    return Err(meta.error("`rename` may be declared only once"));
                }
                if !meta.input.peek(Token![=]) {
                    return Err(meta.error(
                        "directional `rename(serialize = ..., deserialize = ...)` is not supported because rspyts requires one host name",
                    ));
                }
                result.rename = Some(meta.value()?.parse::<LitStr>()?.value());
            } else if meta.path.is_ident("default") {
                if result.default {
                    return Err(meta.error("`default` may be declared only once"));
                }
                if meta.input.peek(syn::Token![=]) {
                    let _ = meta.value()?.parse::<LitStr>()?;
                    return Err(meta.error(
                        "`#[serde(default = \"path\")]` is not supported because function-provided defaults cannot be represented by rspyts",
                    ));
                }
                result.default = true;
            } else if meta.path.is_ident("skip_serializing_if") {
                if result.skip_serializing_if.is_some() {
                    return Err(meta.error("`skip_serializing_if` may be declared only once"));
                }
                result.skip_serializing_if = Some(meta.value()?.parse::<LitStr>()?);
            } else if meta.path.is_ident("alias") {
                return Err(meta.error(
                    "`#[serde(alias = ...)]` is not supported because rspyts exposes one host name",
                ));
            } else {
                return Err(meta.error("unsupported serde field attribute"));
            }
            Ok(())
        })?;
    }
    Ok(result)
}

pub(super) fn rspyts_host_override(attrs: &[Attribute]) -> syn::Result<Option<SynType>> {
    let mut result = None;
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("rspyts")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("host") {
                result = Some(meta.value()?.parse::<SynType>()?);
                Ok(())
            } else {
                Err(meta.error("unsupported type-level rspyts attribute"))
            }
        })?;
    }
    Ok(result)
}

pub(super) fn boundary_attr(attrs: &[Attribute]) -> syn::Result<Option<String>> {
    let mut result = None;
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("rspyts")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("buffer") || meta.path.is_ident("bytes") {
                let boundary = meta.path.get_ident().expect("ident").to_string();
                if result.replace(boundary).is_some() {
                    return Err(meta.error("only one parameter boundary may be declared"));
                }
            } else {
                return Err(meta.error("parameter attributes are buffer or bytes"));
            }
            Ok(())
        })?;
    }
    Ok(result)
}

pub(super) fn take_boundary_attr(attrs: &mut Vec<Attribute>) -> syn::Result<Option<String>> {
    let result = boundary_attr(attrs)?;
    attrs.retain(|attr| !attr.path().is_ident("rspyts"));
    Ok(result)
}

#[derive(Default)]
pub(super) struct FunctionOptions {
    pub(super) returns: Option<String>,
    pub(super) error: Option<SynType>,
}

pub(super) fn function_options(attrs: &[Attribute]) -> syn::Result<FunctionOptions> {
    let mut options = FunctionOptions::default();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("rspyts")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("returns") {
                meta.parse_nested_meta(|boundary| {
                    if boundary.path.is_ident("buffer") || boundary.path.is_ident("bytes") {
                        if options
                            .returns
                            .replace(
                                boundary
                                    .path
                                    .get_ident()
                                    .expect("return boundary identifier")
                                    .to_string(),
                            )
                            .is_some()
                        {
                            return Err(boundary.error("only one return boundary may be declared"));
                        }
                        Ok(())
                    } else {
                        Err(boundary.error("return boundary must be buffer or bytes"))
                    }
                })
            } else if meta.path.is_ident("error") {
                if options.error.is_some() {
                    return Err(meta.error("only one error type may be declared"));
                }
                options.error = Some(meta.value()?.parse::<SynType>()?);
                Ok(())
            } else {
                Err(meta
                    .error("function attributes are returns(buffer|bytes) and error = path::Error"))
            }
        })?;
    }
    Ok(options)
}

pub(super) fn take_function_options(attrs: &mut Vec<Attribute>) -> syn::Result<FunctionOptions> {
    let options = function_options(attrs)?;
    attrs.retain(|attr| !attr.path().is_ident("rspyts"));
    Ok(options)
}

#[derive(Default)]
pub(super) struct MethodOptions {
    pub(super) constructor: bool,
    pub(super) skip: bool,
    pub(super) returns: Option<String>,
    pub(super) error: Option<SynType>,
}

pub(super) fn method_options(attrs: &[Attribute]) -> syn::Result<MethodOptions> {
    let mut options = MethodOptions::default();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("rspyts")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("constructor") {
                options.constructor = true;
            } else if meta.path.is_ident("skip") {
                options.skip = true;
            } else if meta.path.is_ident("returns") {
                meta.parse_nested_meta(|boundary| {
                    if boundary.path.is_ident("buffer") || boundary.path.is_ident("bytes") {
                        if options
                            .returns
                            .replace(
                                boundary
                                    .path
                                    .get_ident()
                                    .expect("return boundary identifier")
                                    .to_string(),
                            )
                            .is_some()
                        {
                            return Err(
                                boundary.error("only one return boundary may be declared")
                            );
                        }
                        Ok(())
                    } else {
                        Err(boundary.error("return boundary must be buffer or bytes"))
                    }
                })?;
            } else if meta.path.is_ident("error") {
                if options.error.is_some() {
                    return Err(meta.error("only one error type may be declared"));
                }
                options.error = Some(meta.value()?.parse::<SynType>()?);
            } else {
                return Err(meta.error(
                    "method attributes are constructor, skip, returns(buffer|bytes), or error = path::Error",
                ));
            }
            Ok(())
        })?;
    }
    Ok(options)
}

pub(super) fn take_method_options(attrs: &mut Vec<Attribute>) -> syn::Result<MethodOptions> {
    let options = method_options(attrs)?;
    attrs.retain(|attr| !attr.path().is_ident("rspyts"));
    Ok(options)
}

pub(super) fn method_exported(method: &ImplItemFn, constructor: bool) -> syn::Result<bool> {
    let options = method_options(&method.attrs)?;
    if options.skip || options.constructor != constructor {
        return Ok(false);
    }
    Ok(true)
}

pub(super) fn docs_tokens(attrs: &[Attribute]) -> TokenStream2 {
    let lines = attrs
        .iter()
        .filter(|attr| attr.path().is_ident("doc"))
        .filter_map(|attr| match &attr.meta {
            Meta::NameValue(value) => match &value.value {
                Expr::Lit(literal) => match &literal.lit {
                    syn::Lit::Str(value) => Some(value.value().trim().to_owned()),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if lines.is_empty() {
        quote!(None)
    } else {
        let docs = lines.join("\n");
        quote!(Some(#docs.to_owned()))
    }
}

pub(super) fn apply_case(value: &str, rule: Option<&str>) -> String {
    match rule {
        Some("camelCase") => value.to_lower_camel_case(),
        Some("snake_case") => value.to_snake_case(),
        Some("kebab-case") => value.to_kebab_case(),
        Some("SCREAMING_SNAKE_CASE") => value.to_shouty_snake_case(),
        Some("PascalCase") => value.to_upper_camel_case(),
        Some("lowercase") => value.to_ascii_lowercase(),
        Some("UPPERCASE") => value.to_ascii_uppercase(),
        _ => value.to_owned(),
    }
}

pub(super) fn type_last_ident(ty: &SynType) -> syn::Result<&Ident> {
    let SynType::Path(path) = ty else {
        return Err(syn::Error::new(ty.span(), "expected a named Rust type"));
    };
    path.path
        .segments
        .last()
        .map(|segment| &segment.ident)
        .ok_or_else(|| syn::Error::new(ty.span(), "expected a named Rust type"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_supported_serde_container_options() {
        let input: DeriveInput = syn::parse_quote! {
            #[serde(rename_all = "camelCase", tag = "kind")]
            enum Event {}
        };
        let options = serde_container(&input.attrs).unwrap();
        assert_eq!(options.rename_all, Some(SerdeRenameRule::Camel));
        assert_eq!(options.tag.as_deref(), Some("kind"));
    }

    #[test]
    fn rejects_serde_aliases() {
        let variant: syn::Variant = syn::parse_quote! {
            #[serde(alias = "old")]
            Current
        };
        let error = serde_rename(&variant.attrs).unwrap_err();
        assert!(error.to_string().contains("alias"));
    }

    #[test]
    fn applies_host_naming_rules() {
        assert_eq!(apply_case("roll_dice", Some("camelCase")), "rollDice");
        assert_eq!(
            apply_serde_variant_case("InvalidRequest", Some(SerdeRenameRule::Snake)),
            "invalid_request"
        );
    }
}
