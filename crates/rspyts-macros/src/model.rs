use heck::ToSnakeCase;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{
    Attribute, Data, DataEnum, DataStruct, DeriveInput, Fields, Ident, ext::IdentExt,
    spanned::Spanned,
};

use crate::attributes::{
    apply_serde_variant_case, docs_tokens, rspyts_host_override, serde_container, serde_rename,
};
use crate::types::{field_options, field_tokens, reject_generics, type_ref_tokens};

pub(super) fn expand_type(input: DeriveInput) -> syn::Result<TokenStream2> {
    reject_generics(&input.generics, input.ident.span())?;
    let ident = input.ident;
    let docs = docs_tokens(&input.attrs);
    let id = quote!(concat!(module_path!(), "::", stringify!(#ident)).to_owned());
    let host = rspyts_host_override(&input.attrs)?;
    let shape = if let Some(host) = host {
        let target = type_ref_tokens(&host, None)?;
        quote!(::rspyts::ir::TypeShape::Alias {
            target: #target,
        })
    } else {
        match input.data {
            Data::Struct(data) => struct_shape(&input.attrs, &ident, data)?,
            Data::Enum(data) => enum_shape(&input.attrs, &data)?,
            Data::Union(data) => {
                return Err(syn::Error::new(
                    data.union_token.span,
                    "unions cannot cross an rspyts boundary",
                ));
            }
        }
    };

    Ok(quote! {
        impl ::rspyts::ContractType for #ident {
            fn type_ref() -> ::rspyts::ir::TypeRef {
                ::rspyts::ir::TypeRef::Named {
                    identity: ::rspyts::ir::DefinitionId::new(
                        env!("CARGO_PKG_NAME"),
                        #id,
                    ),
                }
            }
        }

        const _: () = {
            fn __rspyts_type_registration() -> ::rspyts::ir::TypeDef {
                ::rspyts::ir::TypeDef {
                    owner: ::rspyts::ir::CargoPackageId::new(env!("CARGO_PKG_NAME")),
                    id: #id,
                    name: stringify!(#ident).to_owned(),
                    docs: #docs,
                    shape: #shape,
                }
            }
            ::rspyts::__private::inventory::submit! {
                ::rspyts::registry::TypeRegistration(__rspyts_type_registration)
            }
        };
    })
}

fn struct_shape(attrs: &[Attribute], ident: &Ident, data: DataStruct) -> syn::Result<TokenStream2> {
    let serde = serde_container(attrs)?;
    if serde.rename_all_fields.is_some() {
        return Err(syn::Error::new(
            ident.span(),
            "`#[serde(rename_all_fields = ...)]` is supported only on enums",
        ));
    }
    match data.fields {
        Fields::Named(fields) => {
            if serde.transparent {
                return Err(syn::Error::new(
                    ident.span(),
                    "a transparent struct must have exactly one tuple field",
                ));
            }
            let fields = fields
                .named
                .iter()
                .map(|field| field_tokens(field, serde.rename_all))
                .collect::<syn::Result<Vec<_>>>()?;
            Ok(quote!(::rspyts::ir::TypeShape::Struct {
                fields: vec![#(#fields),*],
            }))
        }
        Fields::Unnamed(fields) if fields.unnamed.len() == 1 && serde.transparent => {
            let field = fields.unnamed.first().expect("one field");
            let options = field_options(&field.attrs)?;
            let target = type_ref_tokens(&field.ty, options.boundary.as_deref())?;
            Ok(quote!(::rspyts::ir::TypeShape::Alias {
                target: #target,
            }))
        }
        Fields::Unnamed(fields) => Err(syn::Error::new(
            fields.span(),
            "tuple structs require `#[serde(transparent)]` and exactly one field",
        )),
        Fields::Unit => Err(syn::Error::new(
            ident.span(),
            "unit structs are not rspyts contract types",
        )),
    }
}

fn enum_shape(attrs: &[Attribute], data: &DataEnum) -> syn::Result<TokenStream2> {
    let serde = serde_container(attrs)?;
    let variants = data
        .variants
        .iter()
        .map(|variant| {
            let rust_name = variant.ident.unraw().to_string();
            let wire_name = serde_rename(&variant.attrs)?
                .unwrap_or_else(|| apply_serde_variant_case(&rust_name, serde.rename_all));
            let docs = docs_tokens(&variant.attrs);
            let fields = match &variant.fields {
                Fields::Unit => Vec::new(),
                Fields::Named(fields) if serde.tag.is_some() => fields
                    .named
                    .iter()
                    .map(|field| field_tokens(field, serde.rename_all_fields))
                    .collect::<syn::Result<Vec<_>>>()?,
                other => {
                    return Err(syn::Error::new(
                        other.span(),
                        "rspyts enums support unit variants or named variants on an internally tagged enum",
                    ));
                }
            };
            Ok(quote!(::rspyts::ir::EnumVariantDef {
                rust_name: #rust_name.to_owned(),
                wire_name: #wire_name.to_owned(),
                docs: #docs,
                fields: vec![#(#fields),*],
            }))
        })
        .collect::<syn::Result<Vec<_>>>()?;
    if let Some(tag) = serde.tag {
        Ok(quote!(::rspyts::ir::TypeShape::TaggedEnum {
            tag: #tag.to_owned(),
            variants: vec![#(#variants),*],
        }))
    } else {
        Ok(quote!(::rspyts::ir::TypeShape::StringEnum {
            variants: vec![#(#variants),*],
        }))
    }
}

pub(super) fn expand_error(input: DeriveInput) -> syn::Result<TokenStream2> {
    reject_generics(&input.generics, input.ident.span())?;
    let ident = input.ident;
    let docs = docs_tokens(&input.attrs);
    let id = quote!(concat!(module_path!(), "::", stringify!(#ident)));
    let mut arms = Vec::new();
    let code_body = match input.data {
        Data::Enum(data) => {
            for variant in data.variants {
                let variant_ident = variant.ident;
                let rust_name = variant_ident.to_string();
                let code =
                    serde_rename(&variant.attrs)?.unwrap_or_else(|| rust_name.to_snake_case());
                let pattern = match &variant.fields {
                    Fields::Unit => quote!(Self::#variant_ident),
                    Fields::Named(_) => quote!(Self::#variant_ident { .. }),
                    Fields::Unnamed(_) => quote!(Self::#variant_ident ( .. )),
                };
                arms.push(quote!(#pattern => #code.to_owned()));
            }
            quote!(match self { #(#arms),* })
        }
        Data::Struct(_) => {
            let rust_name = ident.to_string();
            let code = serde_rename(&input.attrs)?.unwrap_or_else(|| rust_name.to_snake_case());
            quote!(#code.to_owned())
        }
        Data::Union(data) => {
            return Err(syn::Error::new(
                data.union_token.span,
                "unions cannot be rspyts errors",
            ));
        }
    };

    Ok(quote! {
        impl ::rspyts::runtime::ContractError for #ident {
            fn type_identity() -> ::rspyts::ir::DefinitionId {
                ::rspyts::ir::DefinitionId::new(env!("CARGO_PKG_NAME"), #id)
            }

            fn code(&self) -> String {
                #code_body
            }
        }

        const _: () = {
            fn __rspyts_error_registration() -> ::rspyts::ir::ErrorDef {
                ::rspyts::ir::ErrorDef {
                    owner: ::rspyts::ir::CargoPackageId::new(env!("CARGO_PKG_NAME")),
                    id: #id.to_owned(),
                    name: stringify!(#ident).to_owned(),
                    docs: #docs,
                }
            }
            ::rspyts::__private::inventory::submit! {
                ::rspyts::registry::ErrorRegistration(__rspyts_error_registration)
            }
        };
    })
}

#[cfg(test)]
mod tests {
    use super::expand_type;
    use syn::DeriveInput;

    #[test]
    fn expands_a_named_model() {
        let input: DeriveInput = syn::parse_quote! {
            pub struct Point {
                pub x: i32,
                pub y: i32,
            }
        };
        assert!(expand_type(input).is_ok());
    }
}
