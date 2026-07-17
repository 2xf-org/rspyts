//! Proc macros for the deliberately small rspyts 0.4 contract language.

use heck::{ToKebabCase, ToLowerCamelCase, ToShoutySnakeCase, ToSnakeCase, ToUpperCamelCase};
use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::{
    Attribute, Data, DataEnum, DataStruct, DeriveInput, Expr, Fields, FnArg, GenericArgument,
    Ident, ImplItem, ImplItemFn, Item, ItemConst, ItemFn, ItemImpl, ItemStatic, Lit, LitStr, Meta,
    Pat, PathArguments, ReturnType, Token, Type as SynType, TypePath, UnOp,
    parse::{Parse, ParseStream},
    parse_macro_input,
    punctuated::Punctuated,
    spanned::Spanned,
    token::Comma,
};

#[proc_macro_derive(Type, attributes(rspyts, serde))]
pub fn derive_type(input: TokenStream) -> TokenStream {
    expand_type(parse_macro_input!(input as DeriveInput))
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

#[proc_macro_derive(Error, attributes(rspyts, serde, error, source, from, backtrace))]
pub fn derive_error(input: TokenStream) -> TokenStream {
    expand_error(parse_macro_input!(input as DeriveInput))
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

#[proc_macro_attribute]
pub fn export(args: TokenStream, input: TokenStream) -> TokenStream {
    let target = match parse_target(args.into()) {
        Ok(target) => target,
        Err(error) => return error.into_compile_error().into(),
    };
    let item = parse_macro_input!(input as Item);
    expand_export(target, item)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

#[proc_macro]
pub fn module(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as ModuleInput);
    expand_module(input).into()
}

#[derive(Clone, Copy)]
enum ModuleTarget {
    Both,
    Python,
    Typescript,
}

struct ModuleInput {
    module: Ident,
    target: ModuleTarget,
}

impl Parse for ModuleInput {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let module = input.parse::<Ident>()?;
        let target = if input.is_empty() {
            ModuleTarget::Both
        } else {
            input.parse::<Token![,]>()?;
            let target = input.parse::<Ident>()?;
            match target.to_string().as_str() {
                "python" => ModuleTarget::Python,
                "typescript" => ModuleTarget::Typescript,
                _ => {
                    return Err(syn::Error::new(
                        target.span(),
                        "module target must be `python` or `typescript`",
                    ));
                }
            }
        };
        if !input.is_empty() {
            return Err(input.error("unexpected tokens after rspyts module target"));
        }
        Ok(Self { module, target })
    }
}

#[derive(Clone, Copy)]
enum ExportTarget {
    Both,
    Python,
    Typescript,
    Static,
}

impl ExportTarget {
    fn tokens(self) -> TokenStream2 {
        match self {
            Self::Both => quote!(::rspyts::ir::Target::Both),
            Self::Python => quote!(::rspyts::ir::Target::Python),
            Self::Typescript => quote!(::rspyts::ir::Target::Typescript),
            Self::Static => quote!(::rspyts::ir::Target::Static),
        }
    }

    fn includes_python(self) -> bool {
        matches!(self, Self::Both | Self::Python)
    }

    fn includes_wasm(self) -> bool {
        matches!(self, Self::Both | Self::Typescript)
    }
}

fn parse_target(args: TokenStream2) -> syn::Result<ExportTarget> {
    if args.is_empty() {
        return Ok(ExportTarget::Both);
    }
    let target = args.to_string();
    match target.as_str() {
        "python" => Ok(ExportTarget::Python),
        "typescript" | "wasm" => Ok(ExportTarget::Typescript),
        "static" => Ok(ExportTarget::Static),
        _ => Err(syn::Error::new(
            Span::call_site(),
            "expected `python`, `typescript`, or `static`",
        )),
    }
}

fn expand_type(input: DeriveInput) -> syn::Result<TokenStream2> {
    reject_generics(&input.generics, input.ident.span())?;
    let ident = input.ident;
    let docs = docs_tokens(&input.attrs);
    let id = quote!(concat!(module_path!(), "::", stringify!(#ident)).to_owned());
    let wire = rspyts_type_override(&input.attrs)?;
    let shape = if let Some(wire) = wire {
        quote!(::rspyts::ir::TypeShape::Alias {
            target: <#wire as ::rspyts::ContractType>::type_ref(),
        })
    } else {
        match input.data {
            Data::Struct(data) => struct_shape(&input.attrs, &ident, data)?,
            Data::Enum(data) => enum_shape(&input.attrs, data)?,
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
                .map(|field| field_tokens(field, serde.rename_all.as_deref()))
                .collect::<syn::Result<Vec<_>>>()?;
            Ok(quote!(::rspyts::ir::TypeShape::Struct {
                fields: vec![#(#fields),*],
            }))
        }
        Fields::Unnamed(fields) if fields.unnamed.len() == 1 && serde.transparent => {
            let ty = &fields.unnamed.first().expect("one field").ty;
            Ok(quote!(::rspyts::ir::TypeShape::Alias {
                target: <#ty as ::rspyts::ContractType>::type_ref(),
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

fn enum_shape(attrs: &[Attribute], data: DataEnum) -> syn::Result<TokenStream2> {
    let serde = serde_container(attrs)?;
    let variants = data
        .variants
        .iter()
        .map(|variant| {
            let rust_name = variant.ident.to_string();
            let wire_name = serde_rename(&variant.attrs)?
                .unwrap_or_else(|| apply_case(&rust_name, serde.rename_all.as_deref()));
            let docs = docs_tokens(&variant.attrs);
            let fields = match &variant.fields {
                Fields::Unit => Vec::new(),
                Fields::Named(fields) if serde.tag.is_some() => fields
                    .named
                    .iter()
                    .map(|field| field_tokens(field, serde.rename_all_fields.as_deref()))
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

fn expand_error(input: DeriveInput) -> syn::Result<TokenStream2> {
    reject_generics(&input.generics, input.ident.span())?;
    let ident = input.ident;
    let docs = docs_tokens(&input.attrs);
    let id = quote!(concat!(module_path!(), "::", stringify!(#ident)));
    let mut arms = Vec::new();
    let mut variants = Vec::new();
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
                let variant_docs = docs_tokens(&variant.attrs);
                variants.push(quote!(::rspyts::ir::ErrorVariantDef {
                    rust_name: #rust_name.to_owned(),
                    code: #code.to_owned(),
                    docs: #variant_docs,
                    fields: Vec::new(),
                }));
            }
            quote!(match self { #(#arms),* })
        }
        Data::Struct(_) => {
            let rust_name = ident.to_string();
            let code = serde_rename(&input.attrs)?.unwrap_or_else(|| rust_name.to_snake_case());
            variants.push(quote!(::rspyts::ir::ErrorVariantDef {
                rust_name: #rust_name.to_owned(),
                code: #code.to_owned(),
                docs: #docs,
                fields: Vec::new(),
            }));
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
                    variants: vec![#(#variants),*],
                }
            }
            ::rspyts::__private::inventory::submit! {
                ::rspyts::registry::ErrorRegistration(__rspyts_error_registration)
            }
        };
    })
}

fn expand_export(target: ExportTarget, item: Item) -> syn::Result<TokenStream2> {
    match item {
        Item::Fn(function) => expand_function(target, function),
        Item::Impl(item_impl) => expand_resource(target, item_impl),
        Item::Const(item_const) => expand_const(target, item_const),
        Item::Static(item_static) => expand_static(target, item_static),
        other => Err(syn::Error::new(
            other.span(),
            "`#[rspyts::export]` supports public functions, inherent impl blocks, consts, and statics",
        )),
    }
}

fn expand_function(target: ExportTarget, mut function: ItemFn) -> syn::Result<TokenStream2> {
    if matches!(target, ExportTarget::Static) {
        return Err(syn::Error::new(
            function.sig.ident.span(),
            "functions cannot target static TypeScript output",
        ));
    }
    ensure_public(&function.vis, function.sig.ident.span())?;
    reject_signature(&function.sig)?;
    let options = take_function_options(&mut function.attrs)?;
    let ident = &function.sig.ident;
    let rust_name = ident.to_string();
    let host_name = apply_case(&rust_name, Some("camelCase"));
    let docs = docs_tokens(&function.attrs);
    let python_wrapper = if target.includes_python() {
        python_function_wrapper(
            &function,
            options.returns.as_deref(),
            options.error.as_ref(),
        )?
    } else {
        TokenStream2::new()
    };
    let wasm_wrapper = if target.includes_wasm() {
        wasm_function_wrapper(
            &function,
            options.returns.as_deref(),
            options.error.as_ref(),
        )?
    } else {
        TokenStream2::new()
    };
    let params = params_tokens(&mut function.sig.inputs)?;
    let (returns, error) = return_tokens(
        &function.sig.output,
        options.returns.as_deref(),
        options.error.as_ref(),
    )?;
    let target = target.tokens();

    Ok(quote! {
        #function

        const _: () = {
            fn __rspyts_function_registration() -> ::rspyts::ir::FunctionDef {
                ::rspyts::ir::FunctionDef {
                    owner: ::rspyts::ir::CargoPackageId::new(env!("CARGO_PKG_NAME")),
                    rust_name: #rust_name.to_owned(),
                    host_name: #host_name.to_owned(),
                    docs: #docs,
                    target: #target,
                    params: vec![#(#params),*],
                    returns: #returns,
                    error: #error,
                }
            }
            ::rspyts::__private::inventory::submit! {
                ::rspyts::registry::FunctionRegistration(__rspyts_function_registration)
            }
        };

        #python_wrapper
        #wasm_wrapper
    })
}

fn python_function_wrapper(
    function: &ItemFn,
    return_boundary: Option<&str>,
    declared_error: Option<&SynType>,
) -> syn::Result<TokenStream2> {
    let function_ident = &function.sig.ident;
    let wrapper_ident = format_ident!("__rspyts_python_{}", function_ident);
    let register_ident = format_ident!("__rspyts_register_python_{}", function_ident);
    let host_name = apply_case(&function_ident.to_string(), Some("camelCase"));
    let params = wrapper_params(&function.sig.inputs, HostBackend::Python)?;
    let declarations = params.iter().map(|param| &param.declaration);
    let decodes = params.iter().map(|param| &param.decode);
    let calls = params.iter().map(|param| &param.call);
    let invocation = quote!(#function_ident(#(#calls),*));
    let body = host_return_body(
        &function.sig.output,
        invocation,
        HostBackend::Python,
        return_boundary,
        declared_error,
    )?;
    Ok(quote! {
        #[cfg(all(feature = "python", not(target_arch = "wasm32")))]
        #[::rspyts::__private::pyo3::pyfunction(name = #host_name)]
        #[pyo3(crate = "::rspyts::__private::pyo3")]
        fn #wrapper_ident<'py>(
            py: ::rspyts::__private::pyo3::Python<'py>,
            #(#declarations),*
        ) -> ::rspyts::__private::pyo3::PyResult<
            ::rspyts::__private::pyo3::Py<::rspyts::__private::pyo3::PyAny>
        > {
            let __rspyts_types = ::rspyts::registry::type_definitions().map_err(|error| {
                ::rspyts::__private::pyo3::exceptions::PyRuntimeError::new_err(error.to_string())
            })?;
            #(#decodes)*
            #body
        }

        #[cfg(all(feature = "python", not(target_arch = "wasm32")))]
        fn #register_ident(
            module: &::rspyts::__private::pyo3::Bound<'_, ::rspyts::__private::pyo3::types::PyModule>,
        ) -> ::rspyts::__private::pyo3::PyResult<()> {
            ::rspyts::__private::pyo3::types::PyModuleMethods::add_function(
                module,
                ::rspyts::__private::pyo3::wrap_pyfunction!(#wrapper_ident, module)?,
            )?;
            Ok(())
        }

        #[cfg(all(feature = "python", not(target_arch = "wasm32")))]
        ::rspyts::__private::inventory::submit! {
            ::rspyts::runtime::python::Registration {
                owner: env!("CARGO_PKG_NAME"),
                register: #register_ident,
            }
        }
    })
}

fn wasm_function_wrapper(
    function: &ItemFn,
    return_boundary: Option<&str>,
    declared_error: Option<&SynType>,
) -> syn::Result<TokenStream2> {
    let function_ident = &function.sig.ident;
    let wrapper_ident = format_ident!("__rspyts_wasm_{}", function_ident);
    let host_name = apply_case(&function_ident.to_string(), Some("camelCase"));
    let params = wrapper_params(&function.sig.inputs, HostBackend::Wasm)?;
    let declarations = params.iter().map(|param| &param.declaration);
    let decodes = params.iter().map(|param| &param.decode);
    let calls = params.iter().map(|param| &param.call);
    let invocation = quote!(#function_ident(#(#calls),*));
    let body = host_return_body(
        &function.sig.output,
        invocation,
        HostBackend::Wasm,
        return_boundary,
        declared_error,
    )?;
    Ok(quote! {
        #[cfg(all(feature = "wasm", target_arch = "wasm32"))]
        #[doc(hidden)]
        #[allow(missing_docs)]
        #[wasm_bindgen::prelude::wasm_bindgen(js_name = #host_name)]
        pub fn #wrapper_ident(
            #(#declarations),*
        ) -> ::std::result::Result<
            ::rspyts::__private::wasm_bindgen::JsValue,
            ::rspyts::__private::wasm_bindgen::JsValue
        > {
            let __rspyts_types = ::rspyts::registry::type_definitions().map_err(|error| {
                ::rspyts::__private::wasm_bindgen::JsValue::from_str(&error.to_string())
            })?;
            #(#decodes)*
            #body
        }
    })
}

#[derive(Clone, Copy)]
enum HostBackend {
    Python,
    Wasm,
}

struct WrapperParam {
    declaration: TokenStream2,
    decode: TokenStream2,
    call: TokenStream2,
}

fn wrapper_params(
    inputs: &Punctuated<FnArg, Comma>,
    backend: HostBackend,
) -> syn::Result<Vec<WrapperParam>> {
    inputs
        .iter()
        .filter_map(|argument| match argument {
            FnArg::Receiver(_) => None,
            FnArg::Typed(argument) => Some(argument),
        })
        .map(|argument| {
            let Pat::Ident(pattern) = argument.pat.as_ref() else {
                return Err(syn::Error::new(
                    argument.pat.span(),
                    "exported parameters must be simple identifiers",
                ));
            };
            let ident = &pattern.ident;
            let (owned, call) = owned_boundary_type(&argument.ty, ident)?;
            let declaration = match backend {
                HostBackend::Python => quote!(
                    #ident: &::rspyts::__private::pyo3::Bound<'py, ::rspyts::__private::pyo3::PyAny>
                ),
                HostBackend::Wasm => quote!(
                    #ident: ::rspyts::__private::wasm_bindgen::JsValue
                ),
            };
            let boundary = boundary_attr(&argument.attrs)?;
            let ty_ref = type_ref_tokens(&argument.ty, boundary.as_deref())?;
            let decode = match backend {
                HostBackend::Python => quote! {
                    let __rspyts_type = #ty_ref;
                    let __rspyts_wire = ::rspyts::backend::python::decode_typed(
                        #ident,
                        &__rspyts_type,
                        &__rspyts_types,
                    ).map_err(::rspyts::__private::pyo3::PyErr::from)?;
                    let #ident: #owned = ::rspyts::codec::decode(
                        __rspyts_wire,
                        &__rspyts_type,
                        &__rspyts_types,
                    ).map_err(|error| {
                        ::rspyts::__private::pyo3::exceptions::PyValueError::new_err(error.to_string())
                    })?;
                },
                HostBackend::Wasm => quote! {
                    let __rspyts_type = #ty_ref;
                    let __rspyts_wire = ::rspyts::backend::typescript::decode_typed(
                        &#ident,
                        &__rspyts_type,
                        &__rspyts_types,
                    ).map_err(::rspyts::__private::wasm_bindgen::JsValue::from)?;
                    let #ident: #owned = ::rspyts::codec::decode(
                        __rspyts_wire,
                        &__rspyts_type,
                        &__rspyts_types,
                    ).map_err(|error| {
                        ::rspyts::__private::wasm_bindgen::JsValue::from_str(&error.to_string())
                    })?;
                },
            };
            Ok(WrapperParam {
                declaration,
                decode,
                call,
            })
        })
        .collect()
}

fn owned_boundary_type(ty: &SynType, ident: &Ident) -> syn::Result<(SynType, TokenStream2)> {
    let SynType::Reference(reference) = ty else {
        return Ok((ty.clone(), quote!(#ident)));
    };
    if reference.mutability.is_some() {
        return Err(syn::Error::new(
            reference.span(),
            "mutable reference parameters cannot cross an rspyts boundary",
        ));
    }
    match reference.elem.as_ref() {
        SynType::Slice(slice) => {
            let item = slice.elem.as_ref();
            Ok((syn::parse_quote!(Vec<#item>), quote!(&#ident)))
        }
        SynType::Path(path) if path.path.is_ident("str") => {
            Ok((syn::parse_quote!(String), quote!(&#ident)))
        }
        other => Ok((other.clone(), quote!(&#ident))),
    }
}

fn host_return_body(
    output: &ReturnType,
    invocation: TokenStream2,
    backend: HostBackend,
    return_boundary: Option<&str>,
    declared_error: Option<&SynType>,
) -> syn::Result<TokenStream2> {
    let result = resolved_result_types(output, declared_error)?.is_some();
    let ty = return_value_type_tokens(output, return_boundary, declared_error)?;
    match (backend, result) {
        (HostBackend::Python, true) => Ok(quote! {
            match #invocation {
                Ok(value) => {
                    let __rspyts_type = #ty;
                    let __rspyts_wire = ::rspyts::codec::encode(
                        &value,
                        &__rspyts_type,
                        &__rspyts_types,
                    ).map_err(|error| {
                        ::rspyts::__private::pyo3::exceptions::PyValueError::new_err(error.to_string())
                    })?;
                    ::rspyts::backend::python::encode_typed(
                        py,
                        &__rspyts_wire,
                        &__rspyts_type,
                        &__rspyts_types,
                    )
                },
                Err(error) => Err(::rspyts::__private::pyo3::exceptions::PyRuntimeError::new_err((
                    ::rspyts::runtime::ContractError::code(&error),
                    error.to_string(),
                ))),
            }
        }),
        (HostBackend::Python, false) => Ok(quote! {
            let value = #invocation;
            let __rspyts_type = #ty;
            let __rspyts_wire = ::rspyts::codec::encode(
                &value,
                &__rspyts_type,
                &__rspyts_types,
            ).map_err(|error| {
                ::rspyts::__private::pyo3::exceptions::PyValueError::new_err(error.to_string())
            })?;
            ::rspyts::backend::python::encode_typed(
                py,
                &__rspyts_wire,
                &__rspyts_type,
                &__rspyts_types,
            )
        }),
        (HostBackend::Wasm, true) => Ok(quote! {
            match #invocation {
                Ok(value) => {
                    let __rspyts_type = #ty;
                    let __rspyts_wire = ::rspyts::codec::encode(
                        &value,
                        &__rspyts_type,
                        &__rspyts_types,
                    ).map_err(|error| {
                        ::rspyts::__private::wasm_bindgen::JsValue::from_str(&error.to_string())
                    })?;
                    ::rspyts::backend::typescript::encode_typed(
                        &__rspyts_wire,
                        &__rspyts_type,
                        &__rspyts_types,
                    ).map_err(::rspyts::__private::wasm_bindgen::JsValue::from)
                },
                Err(error) => Err(::rspyts::__private::wasm_bindgen::JsValue::from_str(
                    &format!("{}\n{}", ::rspyts::runtime::ContractError::code(&error), error),
                )),
            }
        }),
        (HostBackend::Wasm, false) => Ok(quote! {
            let value = #invocation;
            let __rspyts_type = #ty;
            let __rspyts_wire = ::rspyts::codec::encode(
                &value,
                &__rspyts_type,
                &__rspyts_types,
            ).map_err(|error| {
                ::rspyts::__private::wasm_bindgen::JsValue::from_str(&error.to_string())
            })?;
            ::rspyts::backend::typescript::encode_typed(
                &__rspyts_wire,
                &__rspyts_type,
                &__rspyts_types,
            ).map_err(::rspyts::__private::wasm_bindgen::JsValue::from)
        }),
    }
}

fn return_value_type_tokens(
    output: &ReturnType,
    return_boundary: Option<&str>,
    declared_error: Option<&SynType>,
) -> syn::Result<TokenStream2> {
    let ReturnType::Type(_, ty) = output else {
        if declared_error.is_some() {
            return Err(syn::Error::new(
                Span::call_site(),
                "`error = ...` requires a Result<T> return type",
            ));
        }
        return Ok(quote!(::rspyts::ir::TypeRef::Unit));
    };
    if let Some((ok, _)) = resolved_result_types(output, declared_error)? {
        type_ref_tokens(&ok, return_boundary)
    } else {
        type_ref_tokens(ty, return_boundary)
    }
}

fn expand_const(target: ExportTarget, item: ItemConst) -> syn::Result<TokenStream2> {
    ensure_public(&item.vis, item.ident.span())?;
    constant_tokens(target, quote!(#item), &item.ident, &item.ty)
}

fn expand_static(target: ExportTarget, item: ItemStatic) -> syn::Result<TokenStream2> {
    ensure_public(&item.vis, item.ident.span())?;
    constant_tokens(target, quote!(#item), &item.ident, &item.ty)
}

fn constant_tokens(
    target: ExportTarget,
    item: TokenStream2,
    ident: &Ident,
    ty: &SynType,
) -> syn::Result<TokenStream2> {
    let rust_name = ident.to_string();
    let host_name = rust_name.clone();
    let target = target.tokens();
    Ok(quote! {
        #item
        const _: () = {
            fn __rspyts_constant_registration() -> ::rspyts::ir::ConstantDef {
                ::rspyts::ir::ConstantDef {
                    owner: ::rspyts::ir::CargoPackageId::new(env!("CARGO_PKG_NAME")),
                    rust_name: #rust_name.to_owned(),
                    host_name: #host_name.to_owned(),
                    docs: None,
                    target: #target,
                    ty: <#ty as ::rspyts::ContractType>::type_ref(),
                    value: ::rspyts::__private::serde_json::to_value(&#ident)
                        .expect("an exported rspyts constant must serialize"),
                }
            }
            ::rspyts::__private::inventory::submit! {
                ::rspyts::registry::ConstantRegistration(__rspyts_constant_registration)
            }
        };
    })
}

fn expand_resource(target: ExportTarget, mut item: ItemImpl) -> syn::Result<TokenStream2> {
    if matches!(target, ExportTarget::Static) {
        return Err(syn::Error::new(
            item.impl_token.span,
            "resources cannot target static TypeScript output",
        ));
    }
    if item.trait_.is_some() {
        return Err(syn::Error::new(
            item.impl_token.span,
            "only inherent impl blocks can be exported as resources",
        ));
    }
    reject_generics(&item.generics, item.impl_token.span)?;
    let wrapper_source = item.clone();
    let resource_ty = (*item.self_ty).clone();
    let resource_name = type_last_ident(&resource_ty)?.to_string();
    let resource_id = quote!(concat!(module_path!(), "::", #resource_name).to_owned());
    let mut constructors = Vec::new();
    let mut methods = Vec::new();
    for impl_item in &mut item.items {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };
        if !matches!(method.vis, syn::Visibility::Public(_)) {
            continue;
        }
        let options = take_method_options(&mut method.attrs)?;
        if options.skip {
            continue;
        }
        reject_signature(&method.sig)?;
        let method_target = options.target.unwrap_or(target);
        if options.constructor {
            if options.returns.is_some() {
                return Err(syn::Error::new(
                    method.sig.span(),
                    "resource constructors cannot declare a return boundary",
                ));
            }
            if method.sig.receiver().is_some() {
                return Err(syn::Error::new(
                    method.sig.span(),
                    "a resource constructor cannot take self",
                ));
            }
            constructors.push(resource_constructor_tokens(
                method,
                &resource_ty,
                method_target,
                options.error.as_ref(),
            )?);
        } else {
            reject_reserved_resource_method(method)?;
            methods.push(resource_method_tokens(
                method,
                method_target,
                options.returns.as_deref(),
                options.error.as_ref(),
            )?);
        }
    }
    if constructors.is_empty() {
        return Err(syn::Error::new(
            item.self_ty.span(),
            "an exported resource needs at least one `#[rspyts(constructor)]`",
        ));
    }
    let python_wrapper = if target.includes_python() {
        python_resource_wrapper(&wrapper_source, target)?
    } else {
        TokenStream2::new()
    };
    let wasm_wrapper = if target.includes_wasm() {
        wasm_resource_wrapper(&wrapper_source, target)?
    } else {
        TokenStream2::new()
    };
    let target = target.tokens();
    Ok(quote! {
        #item
        const _: () = {
            fn __rspyts_resource_registration() -> ::rspyts::ir::ResourceDef {
                ::rspyts::ir::ResourceDef {
                    owner: ::rspyts::ir::CargoPackageId::new(env!("CARGO_PKG_NAME")),
                    id: #resource_id,
                    name: #resource_name.to_owned(),
                    docs: None,
                    target: #target,
                    constructors: vec![#(#constructors),*],
                    methods: vec![#(#methods),*],
                }
            }
            ::rspyts::__private::inventory::submit! {
                ::rspyts::registry::ResourceRegistration(__rspyts_resource_registration)
            }
        };

        #python_wrapper
        #wasm_wrapper
    })
}

fn python_resource_wrapper(
    item: &ItemImpl,
    resource_target: ExportTarget,
) -> syn::Result<TokenStream2> {
    let resource_ty = item.self_ty.as_ref();
    let resource_name = type_last_ident(resource_ty)?.to_string();
    let wrapper_ident = format_ident!("__RspytsPython{}", resource_name);
    let register_ident = format_ident!("__rspyts_register_python_resource_{}", resource_name);
    let constructors = exported_resource_constructors(item, resource_target, HostBackend::Python)?;
    let constructor = primary_constructor(&constructors)?;
    let constructor_ident = &constructor.sig.ident;
    let constructor_options = method_options(&constructor.attrs)?;
    let constructor_params = wrapper_params(&constructor.sig.inputs, HostBackend::Python)?;
    let constructor_declarations = constructor_params.iter().map(|param| &param.declaration);
    let constructor_decodes = constructor_params.iter().map(|param| &param.decode);
    let constructor_calls = constructor_params.iter().map(|param| &param.call);
    let constructor_call = quote!(#resource_ty::#constructor_ident(#(#constructor_calls),*));
    let constructor_body =
        if return_result(&constructor.sig.output, constructor_options.error.as_ref())? {
            quote! {
                let inner = match #constructor_call {
                    Ok(value) => value,
                    Err(error) => return Err(
                        ::rspyts::__private::pyo3::exceptions::PyRuntimeError::new_err((
                            ::rspyts::runtime::ContractError::code(&error),
                            error.to_string(),
                        )),
                    ),
                };
                Ok(#wrapper_ident { inner: Some(inner) })
            }
        } else {
            quote! {
                Ok(Self { inner: Some(#constructor_call) })
            }
        };
    let factories = constructors
        .iter()
        .copied()
        .filter(|candidate| !std::ptr::eq(*candidate, constructor))
        .map(|constructor| python_resource_factory(resource_ty, constructor))
        .collect::<syn::Result<Vec<_>>>()?;
    let methods = item
        .items
        .iter()
        .filter_map(|item| match item {
            ImplItem::Fn(method)
                if matches!(method.vis, syn::Visibility::Public(_))
                    && method_exported_to(method, resource_target, HostBackend::Python, false)
                        .unwrap_or(false) =>
            {
                Some(method)
            }
            _ => None,
        })
        .map(|method| python_resource_method(resource_ty, method))
        .collect::<syn::Result<Vec<_>>>()?;

    Ok(quote! {
        #[cfg(all(feature = "python", not(target_arch = "wasm32")))]
        #[::rspyts::__private::pyo3::pyclass(name = #resource_name)]
        #[pyo3(crate = "::rspyts::__private::pyo3")]
        struct #wrapper_ident {
            inner: Option<#resource_ty>,
        }

        #[cfg(all(feature = "python", not(target_arch = "wasm32")))]
        #[::rspyts::__private::pyo3::pymethods]
        #[pyo3(crate = "::rspyts::__private::pyo3")]
        impl #wrapper_ident {
            #[new]
            fn new<'py>(#(#constructor_declarations),*) -> ::rspyts::__private::pyo3::PyResult<Self> {
                let __rspyts_types = ::rspyts::registry::type_definitions().map_err(|error| {
                    ::rspyts::__private::pyo3::exceptions::PyRuntimeError::new_err(error.to_string())
                })?;
                #(#constructor_decodes)*
                #constructor_body
            }

            #(#methods)*
            #(#factories)*

            fn close(&mut self) {
                self.inner.take();
            }
        }

        #[cfg(all(feature = "python", not(target_arch = "wasm32")))]
        fn #register_ident(
            module: &::rspyts::__private::pyo3::Bound<'_, ::rspyts::__private::pyo3::types::PyModule>,
        ) -> ::rspyts::__private::pyo3::PyResult<()> {
            ::rspyts::__private::pyo3::types::PyModuleMethods::add_class::<#wrapper_ident>(module)
        }

        #[cfg(all(feature = "python", not(target_arch = "wasm32")))]
        ::rspyts::__private::inventory::submit! {
            ::rspyts::runtime::python::Registration {
                owner: env!("CARGO_PKG_NAME"),
                register: #register_ident,
            }
        }
    })
}

fn python_resource_factory(
    resource_ty: &SynType,
    constructor: &ImplItemFn,
) -> syn::Result<TokenStream2> {
    let method_ident = &constructor.sig.ident;
    let host_name = apply_case(&method_ident.to_string(), Some("camelCase"));
    let params = wrapper_params(&constructor.sig.inputs, HostBackend::Python)?;
    let declarations = params.iter().map(|param| &param.declaration);
    let decodes = params.iter().map(|param| &param.decode);
    let calls = params.iter().map(|param| &param.call);
    let call = quote!(#resource_ty::#method_ident(#(#calls),*));
    let options = method_options(&constructor.attrs)?;
    let body = if return_result(&constructor.sig.output, options.error.as_ref())? {
        quote! {
            let inner = match #call {
                Ok(value) => value,
                Err(error) => return Err(
                    ::rspyts::__private::pyo3::exceptions::PyRuntimeError::new_err((
                        ::rspyts::runtime::ContractError::code(&error),
                        error.to_string(),
                    )),
                ),
            };
            Ok(Self { inner: Some(inner) })
        }
    } else {
        quote!(Ok(Self { inner: Some(#call) }))
    };
    Ok(quote! {
        #[staticmethod]
        #[pyo3(name = #host_name)]
        fn #method_ident<'py>(#(#declarations),*) -> ::rspyts::__private::pyo3::PyResult<Self> {
            let __rspyts_types = ::rspyts::registry::type_definitions().map_err(|error| {
                ::rspyts::__private::pyo3::exceptions::PyRuntimeError::new_err(error.to_string())
            })?;
            #(#decodes)*
            #body
        }
    })
}

fn python_resource_method(resource_ty: &SynType, method: &ImplItemFn) -> syn::Result<TokenStream2> {
    let method_ident = &method.sig.ident;
    let host_name = apply_case(&method_ident.to_string(), Some("camelCase"));
    let params = wrapper_params(&method.sig.inputs, HostBackend::Python)?;
    let declarations = params.iter().map(|param| &param.declaration);
    let decodes = params.iter().map(|param| &param.decode);
    let calls = params.iter().map(|param| &param.call);
    let invocation = quote!(inner.#method_ident(#(#calls),*));
    let options = method_options(&method.attrs)?;
    let body = host_return_body(
        &method.sig.output,
        invocation,
        HostBackend::Python,
        options.returns.as_deref(),
        options.error.as_ref(),
    )?;
    Ok(quote! {
        #[pyo3(name = #host_name)]
        fn #method_ident<'py>(
            &mut self,
            py: ::rspyts::__private::pyo3::Python<'py>,
            #(#declarations),*
        ) -> ::rspyts::__private::pyo3::PyResult<
            ::rspyts::__private::pyo3::Py<::rspyts::__private::pyo3::PyAny>
        > {
            let inner: &mut #resource_ty = self.inner.as_mut().ok_or_else(|| {
                ::rspyts::__private::pyo3::exceptions::PyRuntimeError::new_err("resource is closed")
            })?;
            let __rspyts_types = ::rspyts::registry::type_definitions().map_err(|error| {
                ::rspyts::__private::pyo3::exceptions::PyRuntimeError::new_err(error.to_string())
            })?;
            #(#decodes)*
            #body
        }
    })
}

fn wasm_resource_wrapper(
    item: &ItemImpl,
    resource_target: ExportTarget,
) -> syn::Result<TokenStream2> {
    let resource_ty = item.self_ty.as_ref();
    let resource_name = type_last_ident(resource_ty)?.to_string();
    let wrapper_ident = format_ident!("__RspytsWasm{}", resource_name);
    let constructors = exported_resource_constructors(item, resource_target, HostBackend::Wasm)?;
    let constructor = primary_constructor(&constructors)?;
    let constructor_ident = &constructor.sig.ident;
    let constructor_options = method_options(&constructor.attrs)?;
    let constructor_params = wrapper_params(&constructor.sig.inputs, HostBackend::Wasm)?;
    let constructor_declarations = constructor_params.iter().map(|param| &param.declaration);
    let constructor_decodes = constructor_params.iter().map(|param| &param.decode);
    let constructor_calls = constructor_params.iter().map(|param| &param.call);
    let constructor_call = quote!(#resource_ty::#constructor_ident(#(#constructor_calls),*));
    let constructor_body = if return_result(
        &constructor.sig.output,
        constructor_options.error.as_ref(),
    )? {
        quote! {
            let inner = match #constructor_call {
                Ok(value) => value,
                Err(error) => return Err(
                    ::rspyts::__private::wasm_bindgen::JsValue::from_str(
                        &format!("{}\n{}", ::rspyts::runtime::ContractError::code(&error), error),
                    ),
                ),
            };
            Ok(#wrapper_ident { inner: Some(inner) })
        }
    } else {
        quote!(Ok(#wrapper_ident { inner: Some(#constructor_call) }))
    };
    let factories = constructors
        .iter()
        .copied()
        .filter(|candidate| !std::ptr::eq(*candidate, constructor))
        .map(|constructor| wasm_resource_factory(resource_ty, constructor))
        .collect::<syn::Result<Vec<_>>>()?;
    let methods = item
        .items
        .iter()
        .filter_map(|item| match item {
            ImplItem::Fn(method)
                if matches!(method.vis, syn::Visibility::Public(_))
                    && method_exported_to(method, resource_target, HostBackend::Wasm, false)
                        .unwrap_or(false) =>
            {
                Some(method)
            }
            _ => None,
        })
        .map(|method| wasm_resource_method(resource_ty, method))
        .collect::<syn::Result<Vec<_>>>()?;

    Ok(quote! {
        #[cfg(all(feature = "wasm", target_arch = "wasm32"))]
        #[doc(hidden)]
        #[allow(missing_docs)]
        #[wasm_bindgen::prelude::wasm_bindgen]
        pub struct #wrapper_ident {
            inner: Option<#resource_ty>,
        }

        #[cfg(all(feature = "wasm", target_arch = "wasm32"))]
        #[doc(hidden)]
        #[allow(missing_docs)]
        #[wasm_bindgen::prelude::wasm_bindgen]
        impl #wrapper_ident {
            #[wasm_bindgen(constructor)]
            pub fn new(#(#constructor_declarations),*) -> ::std::result::Result<
                Self,
                ::rspyts::__private::wasm_bindgen::JsValue
            > {
                let __rspyts_types = ::rspyts::registry::type_definitions().map_err(|error| {
                    ::rspyts::__private::wasm_bindgen::JsValue::from_str(&error.to_string())
                })?;
                #(#constructor_decodes)*
                #constructor_body
            }

            #(#methods)*
            #(#factories)*

            pub fn close(&mut self) {
                self.inner.take();
            }
        }
    })
}

fn wasm_resource_factory(
    resource_ty: &SynType,
    constructor: &ImplItemFn,
) -> syn::Result<TokenStream2> {
    let method_ident = &constructor.sig.ident;
    let host_name = apply_case(&method_ident.to_string(), Some("camelCase"));
    let params = wrapper_params(&constructor.sig.inputs, HostBackend::Wasm)?;
    let declarations = params.iter().map(|param| &param.declaration);
    let decodes = params.iter().map(|param| &param.decode);
    let calls = params.iter().map(|param| &param.call);
    let call = quote!(#resource_ty::#method_ident(#(#calls),*));
    let options = method_options(&constructor.attrs)?;
    let body = if return_result(&constructor.sig.output, options.error.as_ref())? {
        quote! {
            let inner = match #call {
                Ok(value) => value,
                Err(error) => return Err(
                    ::rspyts::__private::wasm_bindgen::JsValue::from_str(
                        &format!("{}\n{}", ::rspyts::runtime::ContractError::code(&error), error),
                    ),
                ),
            };
            Ok(Self { inner: Some(inner) })
        }
    } else {
        quote!(Ok(Self { inner: Some(#call) }))
    };
    Ok(quote! {
        #[wasm_bindgen(js_name = #host_name)]
        pub fn #method_ident(
            #(#declarations),*
        ) -> ::std::result::Result<Self, ::rspyts::__private::wasm_bindgen::JsValue> {
            let __rspyts_types = ::rspyts::registry::type_definitions().map_err(|error| {
                ::rspyts::__private::wasm_bindgen::JsValue::from_str(&error.to_string())
            })?;
            #(#decodes)*
            #body
        }
    })
}

fn wasm_resource_method(resource_ty: &SynType, method: &ImplItemFn) -> syn::Result<TokenStream2> {
    let method_ident = &method.sig.ident;
    let host_name = apply_case(&method_ident.to_string(), Some("camelCase"));
    let params = wrapper_params(&method.sig.inputs, HostBackend::Wasm)?;
    let declarations = params.iter().map(|param| &param.declaration);
    let decodes = params.iter().map(|param| &param.decode);
    let calls = params.iter().map(|param| &param.call);
    let invocation = quote!(inner.#method_ident(#(#calls),*));
    let options = method_options(&method.attrs)?;
    let body = host_return_body(
        &method.sig.output,
        invocation,
        HostBackend::Wasm,
        options.returns.as_deref(),
        options.error.as_ref(),
    )?;
    Ok(quote! {
        #[wasm_bindgen(js_name = #host_name)]
        pub fn #method_ident(
            &mut self,
            #(#declarations),*
        ) -> ::std::result::Result<
            ::rspyts::__private::wasm_bindgen::JsValue,
            ::rspyts::__private::wasm_bindgen::JsValue
        > {
            let inner: &mut #resource_ty = self.inner.as_mut().ok_or_else(|| {
                ::rspyts::__private::wasm_bindgen::JsValue::from_str("resource is closed")
            })?;
            let __rspyts_types = ::rspyts::registry::type_definitions().map_err(|error| {
                ::rspyts::__private::wasm_bindgen::JsValue::from_str(&error.to_string())
            })?;
            #(#decodes)*
            #body
        }
    })
}

fn exported_resource_constructors(
    item: &ItemImpl,
    resource_target: ExportTarget,
    backend: HostBackend,
) -> syn::Result<Vec<&ImplItemFn>> {
    let constructors = item
        .items
        .iter()
        .filter_map(|item| match item {
            ImplItem::Fn(method)
                if method_exported_to(method, resource_target, backend, true).unwrap_or(false) =>
            {
                Some(method)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if constructors.is_empty() {
        return Err(syn::Error::new(
            item.self_ty.span(),
            "an exported resource needs a constructor for each enabled backend",
        ));
    }
    Ok(constructors)
}

fn primary_constructor<'a>(constructors: &[&'a ImplItemFn]) -> syn::Result<&'a ImplItemFn> {
    constructors
        .iter()
        .copied()
        .find(|method| method.sig.ident == "new")
        .or_else(|| constructors.first().copied())
        .ok_or_else(|| syn::Error::new(Span::call_site(), "resource has no constructor"))
}

fn return_result(output: &ReturnType, declared_error: Option<&SynType>) -> syn::Result<bool> {
    Ok(resolved_result_types(output, declared_error)?.is_some())
}

fn resource_constructor_tokens(
    method: &mut ImplItemFn,
    resource_ty: &SynType,
    target: ExportTarget,
    declared_error: Option<&SynType>,
) -> syn::Result<TokenStream2> {
    let rust_name = method.sig.ident.to_string();
    let host_name = apply_case(&rust_name, Some("camelCase"));
    let docs = docs_tokens(&method.attrs);
    let params = params_tokens(&mut method.sig.inputs)?;
    let (_, error) = return_tokens(&method.sig.output, None, declared_error)?;
    let target = target.tokens();
    Ok(quote!(::rspyts::ir::FunctionDef {
        owner: ::rspyts::ir::CargoPackageId::new(env!("CARGO_PKG_NAME")),
        rust_name: #rust_name.to_owned(),
        host_name: #host_name.to_owned(),
        docs: #docs,
        target: #target,
        params: vec![#(#params),*],
        returns: ::rspyts::ir::TypeRef::Named {
            identity: ::rspyts::ir::DefinitionId::new(
                env!("CARGO_PKG_NAME"),
                concat!(module_path!(), "::", stringify!(#resource_ty)),
            ),
        },
        error: #error,
    }))
}

fn resource_method_tokens(
    method: &mut ImplItemFn,
    target: ExportTarget,
    return_boundary: Option<&str>,
    declared_error: Option<&SynType>,
) -> syn::Result<TokenStream2> {
    let receiver = method.sig.receiver().ok_or_else(|| {
        syn::Error::new(
            method.sig.span(),
            "exported resource methods must take `&self` or `&mut self`",
        )
    })?;
    if receiver.reference.is_none() {
        return Err(syn::Error::new(
            receiver.span(),
            "resource methods cannot consume self",
        ));
    }
    let mutable = receiver.mutability.is_some();
    let rust_name = method.sig.ident.to_string();
    let host_name = apply_case(&rust_name, Some("camelCase"));
    let docs = docs_tokens(&method.attrs);
    let params = params_tokens(&mut method.sig.inputs)?;
    let (returns, error) = return_tokens(&method.sig.output, return_boundary, declared_error)?;
    let target = target.tokens();
    Ok(quote!(::rspyts::ir::MethodDef {
        rust_name: #rust_name.to_owned(),
        host_name: #host_name.to_owned(),
        docs: #docs,
        target: #target,
        mutable: #mutable,
        params: vec![#(#params),*],
        returns: #returns,
        error: #error,
    }))
}

fn expand_module(input: ModuleInput) -> TokenStream2 {
    let module = input.module;
    let python = if matches!(input.target, ModuleTarget::Both | ModuleTarget::Python) {
        quote! {
            #[cfg(all(feature = "python", not(target_arch = "wasm32")))]
            #[::rspyts::__private::pyo3::pymodule]
            #[pyo3(crate = "::rspyts::__private::pyo3")]
            fn #module(
                module: &::rspyts::__private::pyo3::Bound<'_, ::rspyts::__private::pyo3::types::PyModule>,
            ) -> ::rspyts::__private::pyo3::PyResult<()> {
                ::rspyts::runtime::python::register(env!("CARGO_PKG_NAME"), module)
            }
        }
    } else {
        TokenStream2::new()
    };
    let typescript = if matches!(input.target, ModuleTarget::Both | ModuleTarget::Typescript) {
        quote! {
            #[cfg(all(feature = "wasm", target_arch = "wasm32"))]
            #[wasm_bindgen::prelude::wasm_bindgen]
            pub fn rspyts_contract_json() -> String {
                let manifest = ::rspyts::registry::manifest(
                    env!("CARGO_PKG_NAME"),
                    env!("CARGO_PKG_VERSION"),
                    stringify!(#module),
                ).expect("invalid rspyts registry");
                ::rspyts::__private::serde_json::to_string(&manifest)
                    .expect("rspyts manifest serialization failed")
            }
        }
    } else {
        TokenStream2::new()
    };
    quote! {
        #[cfg(not(target_arch = "wasm32"))]
        #[unsafe(no_mangle)]
        pub extern "C" fn rspyts_contract() -> *mut ::std::ffi::c_char {
            let manifest = ::rspyts::registry::manifest(
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION"),
                stringify!(#module),
            ).expect("invalid rspyts registry");
            let json = ::rspyts::__private::serde_json::to_string(&manifest)
                .expect("rspyts manifest serialization failed");
            ::std::ffi::CString::new(json)
                .expect("rspyts manifest contained a NUL byte")
                .into_raw()
        }

        #[cfg(not(target_arch = "wasm32"))]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn rspyts_contract_free(pointer: *mut ::std::ffi::c_char) {
            if !pointer.is_null() {
                drop(unsafe { ::std::ffi::CString::from_raw(pointer) });
            }
        }

        #python
        #typescript
    }
}

fn params_tokens(inputs: &mut Punctuated<FnArg, Comma>) -> syn::Result<Vec<TokenStream2>> {
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

fn return_tokens(
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

fn field_tokens(field: &syn::Field, rename_all: Option<&str>) -> syn::Result<TokenStream2> {
    let ident = field.ident.as_ref().expect("named field");
    let rust_name = ident.to_string();
    let wire_name =
        serde_rename(&field.attrs)?.unwrap_or_else(|| apply_case(&rust_name, rename_all));
    let docs = docs_tokens(&field.attrs);
    let options = field_options(&field.attrs)?;
    validate_field_options(field, &options)?;
    let ty = type_ref_tokens(&field.ty, options.boundary.as_deref())?;
    let has_serde_default = serde_has_flag(&field.attrs, "default")?;
    let required = options.required
        || (!is_option(&field.ty) && !has_serde_default && options.default.is_none());
    let default = scalar_option_tokens(options.default.as_ref());
    let literal = scalar_option_tokens(options.literal.as_ref());
    let min_length = option_u64_tokens(options.min_length);
    let max_length = option_u64_tokens(options.max_length);
    let ge = option_i64_tokens(options.ge);
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
struct FieldOptions {
    boundary: Option<String>,
    required: bool,
    literal: Option<SpannedScalar>,
    min_length: Option<u64>,
    max_length: Option<u64>,
    ge: Option<i64>,
    default: Option<SpannedScalar>,
}

fn field_options(attrs: &[Attribute]) -> syn::Result<FieldOptions> {
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
            } else if meta.path.is_ident("default") {
                let value = parse_scalar(meta.value()?.parse::<Expr>()?)?;
                if options.default.replace(value).is_some() {
                    return Err(meta.error("`default` may be declared only once"));
                }
            } else {
                return Err(meta.error(
                    "supported field attributes are buffer, bytes, required, literal, min_length, max_length, ge, and default",
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

fn validate_field_options(field: &syn::Field, options: &FieldOptions) -> syn::Result<()> {
    let has_serde_default = serde_has_flag(&field.attrs, "default")?;
    if let (true, Some(default)) = (options.required, options.default.as_ref()) {
        return Err(syn::Error::new(
            default.span,
            "`required` and an explicit rspyts default cannot be combined",
        ));
    }
    if has_serde_default && !is_option(&field.ty) && options.default.is_none() {
        return Err(syn::Error::new(
            field.span(),
            "`#[serde(default)]` on a non-Option rspyts field requires `#[rspyts(default = ...)]`",
        ));
    }
    if let (Some(minimum), Some(maximum)) = (options.min_length, options.max_length)
        && minimum > maximum
    {
        return Err(syn::Error::new(
            field.span(),
            "`min_length` cannot exceed `max_length`",
        ));
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
    if options.ge.is_some() && !matches!(kind, FieldKind::Integer | FieldKind::Unknown) {
        return Err(syn::Error::new(
            field.ty.span(),
            "`ge` applies only to integer fields",
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

fn type_ref_tokens(ty: &SynType, boundary: Option<&str>) -> syn::Result<TokenStream2> {
    match boundary {
        Some("bytes") => Ok(quote!(::rspyts::ir::TypeRef::Bytes)),
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
        None => Ok(quote!(<#ty as ::rspyts::ContractType>::type_ref())),
    }
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

fn resolved_result_types(
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

fn ensure_public(visibility: &syn::Visibility, span: Span) -> syn::Result<()> {
    if matches!(visibility, syn::Visibility::Public(_)) {
        Ok(())
    } else {
        Err(syn::Error::new(
            span,
            "exported rspyts items must be public",
        ))
    }
}

fn reject_generics(generics: &syn::Generics, span: Span) -> syn::Result<()> {
    if generics.params.is_empty() && generics.where_clause.is_none() {
        Ok(())
    } else {
        Err(syn::Error::new(
            span,
            "generic rspyts contracts are not supported",
        ))
    }
}

fn reject_signature(signature: &syn::Signature) -> syn::Result<()> {
    reject_generics(&signature.generics, signature.ident.span())?;
    if signature.asyncness.is_some() {
        return Err(syn::Error::new(
            signature.span(),
            "async exports are not supported in v0.4",
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

fn reject_reserved_resource_method(method: &ImplItemFn) -> syn::Result<()> {
    let name = method.sig.ident.to_string();
    if matches!(name.as_str(), "close" | "free") {
        return Err(syn::Error::new(
            method.sig.ident.span(),
            format!("`{name}` is reserved for generated resource lifecycle behavior"),
        ));
    }
    Ok(())
}

#[derive(Default)]
struct SerdeContainer {
    rename_all: Option<String>,
    rename_all_fields: Option<String>,
    tag: Option<String>,
    transparent: bool,
}

fn serde_container(attrs: &[Attribute]) -> syn::Result<SerdeContainer> {
    let mut result = SerdeContainer::default();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("serde")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename_all") {
                result.rename_all = Some(meta.value()?.parse::<LitStr>()?.value());
            } else if meta.path.is_ident("rename_all_fields") {
                result.rename_all_fields = Some(meta.value()?.parse::<LitStr>()?.value());
            } else if meta.path.is_ident("tag") {
                result.tag = Some(meta.value()?.parse::<LitStr>()?.value());
            } else if meta.path.is_ident("transparent") {
                result.transparent = true;
            } else if meta.path.is_ident("deny_unknown_fields") {
                // The generated hosts are closed by default; this is accepted metadata.
            } else if meta.path.is_ident("rename") {
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

fn serde_rename(attrs: &[Attribute]) -> syn::Result<Option<String>> {
    let mut value = None;
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("serde")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                value = Some(meta.value()?.parse::<LitStr>()?.value());
            } else if meta.path.is_ident("skip_serializing_if")
                || meta.path.is_ident("default")
                || meta.path.is_ident("alias")
            {
                if meta.input.peek(syn::Token![=]) {
                    let _ = meta.value()?.parse::<Expr>()?;
                }
            } else {
                return Err(meta.error("unsupported serde field or variant attribute"));
            }
            Ok(())
        })?;
    }
    Ok(value)
}

fn serde_has_flag(attrs: &[Attribute], name: &str) -> syn::Result<bool> {
    let mut found = false;
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("serde")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident(name) {
                found = true;
                if meta.input.peek(syn::Token![=]) {
                    let _ = meta.value()?.parse::<Expr>()?;
                }
            } else if meta.path.is_ident("rename")
                || meta.path.is_ident("skip_serializing_if")
                || meta.path.is_ident("default")
                || meta.path.is_ident("alias")
            {
                if meta.input.peek(syn::Token![=]) {
                    let _ = meta.value()?.parse::<Expr>()?;
                }
            } else {
                return Err(meta.error("unsupported serde field attribute"));
            }
            Ok(())
        })?;
    }
    Ok(found)
}

fn rspyts_type_override(attrs: &[Attribute]) -> syn::Result<Option<SynType>> {
    let mut result = None;
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("rspyts")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("wire") {
                result = Some(meta.value()?.parse::<SynType>()?);
                Ok(())
            } else {
                Err(meta.error("unsupported type-level rspyts attribute"))
            }
        })?;
    }
    Ok(result)
}

fn boundary_attr(attrs: &[Attribute]) -> syn::Result<Option<String>> {
    let mut result = None;
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("rspyts")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("buffer") || meta.path.is_ident("bytes") {
                result = Some(meta.path.get_ident().expect("ident").to_string());
            } else if meta.path.is_ident("required") {
            } else {
                return Err(
                    meta.error("supported field attributes are buffer, bytes, and required")
                );
            }
            Ok(())
        })?;
    }
    Ok(result)
}

fn take_boundary_attr(attrs: &mut Vec<Attribute>) -> syn::Result<Option<String>> {
    let result = boundary_attr(attrs)?;
    attrs.retain(|attr| !attr.path().is_ident("rspyts"));
    Ok(result)
}

#[derive(Default)]
struct FunctionOptions {
    returns: Option<String>,
    error: Option<SynType>,
}

fn function_options(attrs: &[Attribute]) -> syn::Result<FunctionOptions> {
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

fn take_function_options(attrs: &mut Vec<Attribute>) -> syn::Result<FunctionOptions> {
    let options = function_options(attrs)?;
    attrs.retain(|attr| !attr.path().is_ident("rspyts"));
    Ok(options)
}

#[derive(Default)]
struct MethodOptions {
    constructor: bool,
    skip: bool,
    python: bool,
    wasm: bool,
    target: Option<ExportTarget>,
    returns: Option<String>,
    error: Option<SynType>,
}

fn method_options(attrs: &[Attribute]) -> syn::Result<MethodOptions> {
    let mut options = MethodOptions::default();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("rspyts")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("constructor") {
                options.constructor = true;
            } else if meta.path.is_ident("skip") {
                options.skip = true;
            } else if meta.path.is_ident("python") {
                options.python = true;
            } else if meta.path.is_ident("wasm") || meta.path.is_ident("typescript") {
                options.wasm = true;
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
                    "method attributes are constructor, skip, python, wasm, typescript, returns(buffer|bytes), or error = path::Error",
                ));
            }
            Ok(())
        })?;
    }
    options.target = match (options.python, options.wasm) {
        (true, true) => Some(ExportTarget::Both),
        (true, false) => Some(ExportTarget::Python),
        (false, true) => Some(ExportTarget::Typescript),
        (false, false) => None,
    };
    Ok(options)
}

fn take_method_options(attrs: &mut Vec<Attribute>) -> syn::Result<MethodOptions> {
    let options = method_options(attrs)?;
    attrs.retain(|attr| !attr.path().is_ident("rspyts"));
    Ok(options)
}

fn method_exported_to(
    method: &ImplItemFn,
    resource_target: ExportTarget,
    backend: HostBackend,
    constructor: bool,
) -> syn::Result<bool> {
    let options = method_options(&method.attrs)?;
    if options.skip || options.constructor != constructor {
        return Ok(false);
    }
    let target = options.target.unwrap_or(resource_target);
    Ok(match backend {
        HostBackend::Python => target.includes_python(),
        HostBackend::Wasm => target.includes_wasm(),
    })
}

fn docs_tokens(attrs: &[Attribute]) -> TokenStream2 {
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

fn apply_case(value: &str, rule: Option<&str>) -> String {
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

fn type_last_ident(ty: &SynType) -> syn::Result<&Ident> {
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
    fn generated_resource_lifecycle_names_are_reserved() {
        for name in ["close", "free"] {
            let method: ImplItemFn = syn::parse_str(&format!("pub fn {name}(&self) {{}}")).unwrap();
            let error = reject_reserved_resource_method(&method).unwrap_err();
            assert!(error.to_string().contains("reserved"));
        }

        let method: ImplItemFn = syn::parse_quote!(
            pub fn dispose(&self) {}
        );
        reject_reserved_resource_method(&method).unwrap();
    }

    #[test]
    fn contradictory_field_constraints_fail_at_macro_expansion_time() {
        let field: syn::Field = syn::parse_quote! {
            #[rspyts(min_length = 3, max_length = 2)]
            values: Vec<String>
        };
        let options = field_options(&field.attrs).unwrap();
        let error = validate_field_options(&field, &options).unwrap_err();
        assert!(error.to_string().contains("cannot exceed"));
    }

    #[test]
    fn static_target_rejects_executable_exports() {
        let function: ItemFn = syn::parse_quote!(
            pub fn calculate() {}
        );
        let error = expand_function(ExportTarget::Static, function).unwrap_err();
        assert!(error.to_string().contains("functions cannot target static"));

        let resource: ItemImpl = syn::parse_quote! {
            impl Counter {
                #[rspyts(constructor)]
                pub fn new() -> Self { Self }
            }
        };
        let error = expand_resource(ExportTarget::Static, resource).unwrap_err();
        assert!(error.to_string().contains("resources cannot target static"));
    }
}
