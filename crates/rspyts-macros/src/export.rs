//! Expansion of callable, resource, and constant exports.
//!
//! Each callable is processed in three independent stages: contract metadata,
//! a native Python wrapper, and a wasm-bindgen wrapper. Direct byte and numeric
//! buffer annotations select specialized ABI plans; all other values use the
//! shared Serde bridge. Resource expansion applies the same machinery to
//! constructors and methods while preserving Rust-owned state.

use heck::ToSnakeCase;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::{
    FnArg, GenericArgument, Ident, ImplItem, ImplItemFn, Item, ItemConst, ItemFn, ItemImpl,
    ItemStatic, Pat, PathArguments, ReturnType, Type as SynType, ext::IdentExt,
    punctuated::Punctuated, spanned::Spanned, token::Comma,
};

use crate::attributes::{
    apply_case, boundary_attr, docs_tokens, method_exported, method_options, take_function_options,
    take_method_options, type_last_ident,
};
use crate::types::{
    ensure_public, native_export_name, params_tokens, reject_generics,
    reject_reserved_resource_method, reject_signature, resolved_result_types, return_tokens,
    type_ref_tokens,
};

// Free-function exports -----------------------------------------------------

/// Dispatch an export attribute to the supported declaration expander.
pub(super) fn expand_export(item: Item) -> syn::Result<TokenStream2> {
    match item {
        Item::Fn(function) => expand_function(function),
        Item::Impl(item_impl) => expand_resource(item_impl),
        Item::Const(item_const) => expand_const(&item_const),
        Item::Static(item_static) => expand_static(&item_static),
        other => Err(syn::Error::new(
            other.span(),
            "`#[rspyts::export]` supports public functions, inherent impl blocks, consts, and statics",
        )),
    }
}

/// Expand one free function into metadata and both host wrappers.
fn expand_function(mut function: ItemFn) -> syn::Result<TokenStream2> {
    ensure_public(&function.vis, function.sig.ident.span())?;
    reject_signature(&function.sig)?;
    let options = take_function_options(&mut function.attrs)?;
    let ident = &function.sig.ident;
    let rust_name = ident.unraw().to_string();
    let host_name = apply_case(&rust_name, Some("camelCase"));
    let native_name = native_export_name(ident.span(), "function", &host_name);
    let docs = docs_tokens(&function.attrs);
    let python_wrapper = python_function_wrapper(
        &function,
        &native_name,
        options.returns.as_deref(),
        options.error.as_ref(),
    )?;
    let wasm_wrapper = wasm_function_wrapper(
        &function,
        &native_name,
        options.returns.as_deref(),
        options.error.as_ref(),
    )?;
    let params = params_tokens(&mut function.sig.inputs)?;
    let (returns, error) = return_tokens(
        &function.sig.output,
        options.returns.as_deref(),
        options.error.as_ref(),
    )?;
    Ok(quote! {
        #function

        const _: () = {
            fn __rspyts_function_registration() -> ::rspyts::ir::FunctionDef {
                ::rspyts::ir::FunctionDef {
                    owner: ::rspyts::ir::CargoPackageId::new(env!("CARGO_PKG_NAME")),
                    rust_module: module_path!().to_owned(),
                    rust_name: #rust_name.to_owned(),
                    host_name: #host_name.to_owned(),
                    native_name: #native_name.to_owned(),
                    docs: #docs,
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

/// Emit the PyO3 wrapper and inventory registration for a free function.
fn python_function_wrapper(
    function: &ItemFn,
    native_name: &str,
    return_boundary: Option<&str>,
    declared_error: Option<&SynType>,
) -> syn::Result<TokenStream2> {
    let function_ident = &function.sig.ident;
    let wrapper_ident = format_ident!("__rspyts_python_{}", function_ident);
    let register_ident = format_ident!("__rspyts_register_python_{}", function_ident);
    let params = wrapper_params(&function.sig.inputs, HostBackend::Python)?;
    let declarations = params.iter().map(|param| &param.declaration);
    let decodes = params.iter().map(|param| &param.decode);
    let calls = params.iter().map(|param| &param.call);
    let invocation = quote!(#function_ident(#(#calls),*));
    let return_plan = host_return_plan(
        &function.sig.output,
        &invocation,
        HostBackend::Python,
        return_boundary,
        declared_error,
    )?;
    let return_ty = &return_plan.ty;
    let body = &return_plan.body;
    Ok(quote! {
        #[cfg(not(target_arch = "wasm32"))]
        #[::rspyts::__private::pyo3::pyfunction(name = #native_name)]
        #[pyo3(crate = "::rspyts::__private::pyo3")]
        fn #wrapper_ident<'py>(
            __rspyts_py: ::rspyts::__private::pyo3::Python<'py>,
            #(#declarations),*
        ) -> ::rspyts::__private::pyo3::PyResult<#return_ty> {
            #(#decodes)*
            #body
        }

        #[cfg(not(target_arch = "wasm32"))]
        fn #register_ident(
            __rspyts_module: &::rspyts::__private::pyo3::Bound<'_, ::rspyts::__private::pyo3::types::PyModule>,
        ) -> ::rspyts::__private::pyo3::PyResult<()> {
            ::rspyts::__private::pyo3::types::PyModuleMethods::add_function(
                __rspyts_module,
                ::rspyts::__private::pyo3::wrap_pyfunction!(#wrapper_ident, __rspyts_module)?,
            )?;
            Ok(())
        }

        #[cfg(not(target_arch = "wasm32"))]
        ::rspyts::__private::inventory::submit! {
            ::rspyts::runtime::python::Registration(#register_ident)
        }
    })
}

/// Emit the wasm-bindgen wrapper for a free function.
fn wasm_function_wrapper(
    function: &ItemFn,
    native_name: &str,
    return_boundary: Option<&str>,
    declared_error: Option<&SynType>,
) -> syn::Result<TokenStream2> {
    let function_ident = &function.sig.ident;
    let wrapper_ident = format_ident!("__rspyts_wasm_{}", function_ident);
    let module_ident = format_ident!("__rspyts_wasm_function_{}", function_ident);
    let params = wrapper_params(&function.sig.inputs, HostBackend::Wasm)?;
    let declarations = params.iter().map(|param| &param.declaration);
    let decodes = params.iter().map(|param| &param.decode);
    let calls = params.iter().map(|param| &param.call);
    let invocation = quote!(#function_ident(#(#calls),*));
    let return_plan = host_return_plan(
        &function.sig.output,
        &invocation,
        HostBackend::Wasm,
        return_boundary,
        declared_error,
    )?;
    let return_ty = &return_plan.ty;
    let body = &return_plan.body;
    Ok(quote! {
        #[cfg(target_arch = "wasm32")]
        mod #module_ident {
            use super::*;
            use ::rspyts::__private::wasm_bindgen::{self, prelude::wasm_bindgen};

            #[doc(hidden)]
            #[allow(missing_docs)]
            #[wasm_bindgen(
                js_name = #native_name,
                wasm_bindgen = ::rspyts::__private::wasm_bindgen
            )]
            pub fn #wrapper_ident(
                #(#declarations),*
            ) -> ::std::result::Result<#return_ty, ::rspyts::__private::wasm_bindgen::JsValue> {
                #(#decodes)*
                #body
            }
        }
    })
}

/// Host backend for which wrapper tokens are being planned.
#[derive(Clone, Copy)]
enum HostBackend {
    Python,
    Wasm,
}

/// One host-wrapper parameter after ABI selection and ownership planning.
struct WrapperParam {
    declaration: TokenStream2,
    decode: TokenStream2,
    call: TokenStream2,
}

/// Plan declarations, decoding, and Rust call expressions for all parameters.
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
            reject_nested_parameter_references(&argument.ty)?;
            let (owned, call) = owned_boundary_type(&argument.ty, ident)?;
            let boundary = boundary_attr(&argument.attrs)?;
            if let Some(param) = match backend {
                HostBackend::Python => {
                    python_direct_boundary_param(&argument.ty, ident, boundary.as_deref(), &call)?
                }
                HostBackend::Wasm => {
                    wasm_direct_boundary_param(&argument.ty, ident, boundary.as_deref(), &call)?
                }
            } {
                return Ok(param);
            }
            let declaration = match backend {
                HostBackend::Python => quote!(
                    #ident: &::rspyts::__private::pyo3::Bound<'py, ::rspyts::__private::pyo3::PyAny>
                ),
                HostBackend::Wasm => quote!(
                    #ident: ::rspyts::__private::wasm_bindgen::JsValue
                ),
            };
            let decode = match backend {
                HostBackend::Python => quote! {
                    let #ident: #owned = ::rspyts::bridge::python::from_host(#ident)?;
                },
                HostBackend::Wasm => quote! {
                    let #ident: #owned = ::rspyts::bridge::wasm::from_host(#ident)?;
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

// Direct parameter boundaries ----------------------------------------------

/// Plan a direct Python byte or numeric-buffer parameter.
fn python_direct_boundary_param(
    ty: &SynType,
    ident: &Ident,
    boundary: Option<&str>,
    owned_call: &TokenStream2,
) -> syn::Result<Option<WrapperParam>> {
    let Some(boundary @ ("bytes" | "buffer")) = boundary else {
        return Ok(None);
    };
    type_ref_tokens(ty, Some(boundary))?;
    let declaration = quote!(
        #ident: &::rspyts::__private::pyo3::Bound<'py, ::rspyts::__private::pyo3::PyAny>
    );
    let (borrowed, value_ty) = match ty {
        SynType::Reference(reference) => (true, reference.elem.as_ref()),
        value => (false, value),
    };
    if boundary == "bytes" {
        if let SynType::Array(array) = value_ty {
            let length = &array.len;
            let call = if borrowed {
                quote!(&#ident)
            } else {
                quote!(#ident)
            };
            return Ok(Some(WrapperParam {
                declaration,
                decode: quote! {
                    let #ident = ::rspyts::bridge::python::bytes_from_host(#ident)?;
                    let #ident: [::core::primitive::u8; #length] =
                        <[::core::primitive::u8; #length] as ::core::convert::TryFrom<
                            ::std::vec::Vec<::core::primitive::u8>
                        >>::try_from(#ident).map_err(|value| {
                            ::rspyts::__private::pyo3::exceptions::PyValueError::new_err(
                                ::std::format!(
                                    "parameter `{}` must contain exactly {} bytes, received {}",
                                    ::core::stringify!(#ident),
                                    #length,
                                    value.len(),
                                ),
                            )
                        })?;
                },
                call,
            }));
        }
        return Ok(Some(WrapperParam {
            declaration,
            decode: quote! {
                let #ident = ::rspyts::bridge::python::bytes_from_host(#ident)?;
            },
            call: owned_call.clone(),
        }));
    }
    let item = direct_sequence_item(value_ty).ok_or_else(|| {
        syn::Error::new(
            ty.span(),
            "`buffer` requires exactly `Vec<T>`, `&Vec<T>`, or `&[T]` with a supported numeric scalar",
        )
    })?;
    Ok(Some(WrapperParam {
        declaration,
        decode: quote! {
            let #ident = ::rspyts::bridge::python::buffer_from_host::<#item>(#ident)?;
        },
        call: owned_call.clone(),
    }))
}

/// Plan a direct wasm-bindgen byte or typed-array parameter.
fn wasm_direct_boundary_param(
    ty: &SynType,
    ident: &Ident,
    boundary: Option<&str>,
    owned_call: &TokenStream2,
) -> syn::Result<Option<WrapperParam>> {
    let Some(boundary @ ("bytes" | "buffer")) = boundary else {
        return Ok(None);
    };
    type_ref_tokens(ty, Some(boundary))?;

    let (borrowed, value_ty) = match ty {
        SynType::Reference(reference) => (true, reference.elem.as_ref()),
        value => (false, value),
    };
    match value_ty {
        SynType::Slice(slice) => {
            let item = &slice.elem;
            Ok(Some(WrapperParam {
                declaration: quote!(#ident: &[#item]),
                decode: TokenStream2::new(),
                call: quote!(#ident),
            }))
        }
        SynType::Path(_) if direct_vector_item(value_ty).is_some() => Ok(Some(WrapperParam {
            declaration: quote!(#ident: #value_ty),
            decode: TokenStream2::new(),
            call: owned_call.clone(),
        })),
        SynType::Array(array) if boundary == "bytes" => {
            let length = &array.len;
            let call = if borrowed {
                quote!(&#ident)
            } else {
                quote!(#ident)
            };
            Ok(Some(WrapperParam {
                declaration: quote!(#ident: ::std::vec::Vec<::core::primitive::u8>),
                decode: quote! {
                    let #ident: [::core::primitive::u8; #length] =
                        <[::core::primitive::u8; #length] as ::core::convert::TryFrom<
                            ::std::vec::Vec<::core::primitive::u8>
                        >>::try_from(#ident).map_err(|_| {
                            ::rspyts::__private::wasm_bindgen::JsValue::from_str(
                                &::std::format!(
                                    "parameter `{}` must contain exactly {} bytes",
                                    ::core::stringify!(#ident),
                                    ::core::mem::size_of::<[
                                        ::core::primitive::u8;
                                        #length
                                    ]>(),
                                ),
                            )
                        })?;
                },
                call,
            }))
        }
        _ => Err(syn::Error::new(
            ty.span(),
            format!("`{boundary}` does not support this Wasm input type"),
        )),
    }
}

/// Return the element type of a vector, slice, or array direct boundary.
fn direct_sequence_item(ty: &SynType) -> Option<&SynType> {
    match ty {
        SynType::Slice(slice) => Some(&slice.elem),
        value => direct_vector_item(value),
    }
}

/// Return the element type only when a direct boundary is an owned vector.
fn direct_vector_item(ty: &SynType) -> Option<&SynType> {
    let SynType::Path(path) = ty else {
        return None;
    };
    if path.qself.is_some() {
        return None;
    }
    let segments = path
        .path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>();
    if !matches!(segments.as_slice(), [name] if name == "Vec")
        && !matches!(segments.as_slice(), [root, module, name]
            if matches!(root.as_str(), "std" | "alloc") && module == "vec" && name == "Vec")
    {
        return None;
    }
    let segment = path.path.segments.last()?;
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    let mut arguments = arguments.args.iter();
    let item = match arguments.next() {
        Some(GenericArgument::Type(item)) => item,
        _ => return None,
    };
    arguments.next().is_none().then_some(item)
}

/// Reject references nested where wrapper-owned temporaries cannot satisfy them.
fn reject_nested_parameter_references(ty: &SynType) -> syn::Result<()> {
    let root = match ty {
        SynType::Reference(reference) if reference.mutability.is_none() => reference.elem.as_ref(),
        ty => ty,
    };
    if contains_reference(root) {
        return Err(syn::Error::new(
            root.span(),
            "references are supported only at the outermost level of an exported parameter; use owned values inside Option, Vec, maps, and tuples",
        ));
    }
    Ok(())
}

/// Return whether any node in a syntax type tree is a reference.
fn contains_reference(ty: &SynType) -> bool {
    match ty {
        SynType::Reference(_) => true,
        SynType::Array(array) => contains_reference(&array.elem),
        SynType::Slice(slice) => contains_reference(&slice.elem),
        SynType::Tuple(tuple) => tuple.elems.iter().any(contains_reference),
        SynType::Paren(paren) => contains_reference(&paren.elem),
        SynType::Group(group) => contains_reference(&group.elem),
        SynType::Path(path) => path.path.segments.iter().any(|segment| {
            let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
                return false;
            };
            arguments.args.iter().any(
                |argument| matches!(argument, GenericArgument::Type(ty) if contains_reference(ty)),
            )
        }),
        _ => false,
    }
}

/// Select an owned decode type and a call expression for a Rust parameter.
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

// Return boundaries ---------------------------------------------------------

/// Generated wrapper return type and the statements that produce it.
struct HostReturnPlan {
    ty: TokenStream2,
    body: TokenStream2,
}

/// Specialized vector ABI for an annotated successful return value.
struct DirectReturnPlan {
    ty: TokenStream2,
    value: TokenStream2,
    element: TokenStream2,
}

/// Plan host return conversion for ordinary and specialized boundaries.
fn host_return_plan(
    output: &ReturnType,
    invocation: &TokenStream2,
    backend: HostBackend,
    return_boundary: Option<&str>,
    declared_error: Option<&SynType>,
) -> syn::Result<HostReturnPlan> {
    let resolved = resolved_result_types(output, declared_error)?;
    let result = resolved.is_some();
    if let Some(boundary @ ("bytes" | "buffer")) = return_boundary {
        let success_ty = match (&resolved, output) {
            (Some((ok, _)), _) => ok,
            (None, ReturnType::Type(_, ty)) => ty.as_ref(),
            (None, ReturnType::Default) => {
                return Err(syn::Error::new(
                    output.span(),
                    format!("`returns({boundary})` requires a return value"),
                ));
            }
        };
        let direct = direct_return_plan(success_ty, boundary)?;
        let return_ty = match backend {
            HostBackend::Python => {
                quote!(::rspyts::__private::pyo3::Py<::rspyts::__private::pyo3::PyAny>)
            }
            HostBackend::Wasm => direct.ty.clone(),
        };
        let converted = &direct.value;
        let success = match (backend, boundary) {
            (HostBackend::Python, "bytes") => quote! {
                let __rspyts_value = #converted;
                Ok(::rspyts::bridge::python::bytes_to_host(__rspyts_py, &__rspyts_value))
            },
            (HostBackend::Python, "buffer") => {
                let element = &direct.element;
                quote! {
                    let __rspyts_value = #converted;
                    Ok(::rspyts::bridge::python::buffer_to_host::<#element>(
                        __rspyts_py,
                        &__rspyts_value,
                    ))
                }
            }
            (HostBackend::Wasm, _) => quote! {
                let __rspyts_value = #converted;
                Ok(__rspyts_value)
            },
            _ => unreachable!("validated return boundary"),
        };
        let contract_error = match backend {
            HostBackend::Python => quote!(::rspyts::bridge::python::contract_error),
            HostBackend::Wasm => quote!(::rspyts::bridge::wasm::contract_error),
        };
        let body = if result {
            quote! {
                match #invocation {
                    Ok(__rspyts_value) => { #success }
                    Err(__rspyts_error) => Err(#contract_error(&__rspyts_error)),
                }
            }
        } else {
            quote! {
                let __rspyts_value = #invocation;
                #success
            }
        };
        return Ok(HostReturnPlan {
            ty: return_ty,
            body,
        });
    }
    let ty = match backend {
        HostBackend::Python => {
            quote!(::rspyts::__private::pyo3::Py<::rspyts::__private::pyo3::PyAny>)
        }
        HostBackend::Wasm => quote!(::rspyts::__private::wasm_bindgen::JsValue),
    };
    let body = match (backend, result) {
        (HostBackend::Python, true) => quote! {
            match #invocation {
                Ok(__rspyts_value) => ::rspyts::bridge::python::to_host(
                    __rspyts_py,
                    &__rspyts_value,
                ),
                Err(__rspyts_error) => Err(::rspyts::bridge::python::contract_error(
                    &__rspyts_error,
                )),
            }
        },
        (HostBackend::Python, false) => quote! {
            let __rspyts_value = #invocation;
            ::rspyts::bridge::python::to_host(
                __rspyts_py,
                &__rspyts_value,
            )
        },
        (HostBackend::Wasm, true) => quote! {
            match #invocation {
                Ok(__rspyts_value) => ::rspyts::bridge::wasm::to_host(&__rspyts_value),
                Err(__rspyts_error) => Err(::rspyts::bridge::wasm::contract_error(
                    &__rspyts_error,
                )),
            }
        },
        (HostBackend::Wasm, false) => quote! {
            let __rspyts_value = #invocation;
            ::rspyts::bridge::wasm::to_host(&__rspyts_value)
        },
    };
    Ok(HostReturnPlan { ty, body })
}

/// Validate and plan an annotated direct vector return ABI.
fn direct_return_plan(ty: &SynType, boundary: &str) -> syn::Result<DirectReturnPlan> {
    type_ref_tokens(ty, Some(boundary))?;
    let (borrowed, value_ty) = match ty {
        SynType::Reference(reference) => (true, reference.elem.as_ref()),
        value => (false, value),
    };
    if boundary == "bytes" {
        let value = if borrowed {
            quote!(__rspyts_value.to_vec())
        } else if matches!(value_ty, SynType::Array(_)) {
            quote!(::std::vec::Vec::from(__rspyts_value))
        } else {
            quote!(__rspyts_value)
        };
        return Ok(DirectReturnPlan {
            ty: quote!(::std::vec::Vec<::core::primitive::u8>),
            value,
            element: quote!(::core::primitive::u8),
        });
    }
    let item = direct_sequence_item(value_ty).ok_or_else(|| {
        syn::Error::new(
            ty.span(),
            "`returns(buffer)` requires exactly `Vec<T>`, `&Vec<T>`, or `&[T]` with a supported numeric scalar",
        )
    })?;
    let value = if borrowed || matches!(value_ty, SynType::Slice(_)) {
        quote!(__rspyts_value.to_vec())
    } else {
        quote!(__rspyts_value)
    };
    Ok(DirectReturnPlan {
        ty: quote!(::std::vec::Vec<#item>),
        value,
        element: quote!(#item),
    })
}

// Constant exports ----------------------------------------------------------

/// Expand a public constant into a lazy contract registration.
fn expand_const(item: &ItemConst) -> syn::Result<TokenStream2> {
    ensure_public(&item.vis, item.ident.span())?;
    let docs = docs_tokens(&item.attrs);
    let item_tokens = quote!(#item);
    constant_tokens(&item_tokens, &item.ident, &item.ty, &docs)
}

/// Expand a public static into a lazy contract registration.
fn expand_static(item: &ItemStatic) -> syn::Result<TokenStream2> {
    ensure_public(&item.vis, item.ident.span())?;
    let docs = docs_tokens(&item.attrs);
    let item_tokens = quote!(#item);
    constant_tokens(&item_tokens, &item.ident, &item.ty, &docs)
}

/// Emit common serialized-value registration for a constant or static.
fn constant_tokens(
    item: &TokenStream2,
    ident: &Ident,
    ty: &SynType,
    docs: &TokenStream2,
) -> syn::Result<TokenStream2> {
    let rust_name = ident.unraw().to_string();
    let host_name = rust_name.clone();
    let ty_ref = type_ref_tokens(ty, None)?;
    Ok(quote! {
        #item
        const _: () = {
            fn __rspyts_constant_registration() -> ::std::result::Result<
                ::rspyts::ir::ConstantDef,
                ::std::string::String,
            > {
                let __rspyts_type = #ty_ref;
                let __rspyts_value = ::rspyts::__private::serde_json::to_value(&self::#ident).map_err(|__rspyts_error| {
                    ::std::format!("constant `{}` could not serialize as JSON: {__rspyts_error}", #host_name)
                })?;
                ::std::result::Result::Ok(::rspyts::ir::ConstantDef {
                    owner: ::rspyts::ir::CargoPackageId::new(env!("CARGO_PKG_NAME")),
                    rust_module: module_path!().to_owned(),
                    host_name: #host_name.to_owned(),
                    docs: #docs,
                    ty: __rspyts_type,
                    value: __rspyts_value,
                })
            }
            ::rspyts::__private::inventory::submit! {
                ::rspyts::registry::ConstantRegistration(__rspyts_constant_registration)
            }
        };
    })
}

// Stateful resource exports -------------------------------------------------

/// Expand an inherent implementation into metadata and host resource classes.
fn expand_resource(mut item: ItemImpl) -> syn::Result<TokenStream2> {
    if item.trait_.is_some() {
        return Err(syn::Error::new(
            item.impl_token.span,
            "only inherent impl blocks can be exported as resources",
        ));
    }
    reject_generics(&item.generics, item.impl_token.span)?;
    let docs = docs_tokens(&item.attrs);
    let wrapper_source = item.clone();
    let resource_ty = (*item.self_ty).clone();
    let resource_ident = type_last_ident(&resource_ty)?;
    let resource_name = resource_ident.unraw().to_string();
    let native_name = native_export_name(resource_ident.span(), "resource", &resource_name);
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
                &native_name,
                options.error.as_ref(),
            )?);
        } else {
            reject_reserved_resource_method(method)?;
            methods.push(resource_method_tokens(
                method,
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
    let python_wrapper = python_resource_wrapper(&wrapper_source, &native_name)?;
    let wasm_wrapper = wasm_resource_wrapper(&wrapper_source, &native_name)?;
    Ok(quote! {
        #item
        const _: () = {
            fn __rspyts_resource_registration() -> ::rspyts::ir::ResourceDef {
                ::rspyts::ir::ResourceDef {
                    owner: ::rspyts::ir::CargoPackageId::new(env!("CARGO_PKG_NAME")),
                    rust_module: module_path!().to_owned(),
                    name: #resource_name.to_owned(),
                    native_name: #native_name.to_owned(),
                    docs: #docs,
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

/// Emit the PyO3 class, factories, methods, and registration for a resource.
fn python_resource_wrapper(item: &ItemImpl, native_name: &str) -> syn::Result<TokenStream2> {
    let resource_ty = item.self_ty.as_ref();
    let resource_name = type_last_ident(resource_ty)?.to_string();
    let wrapper_ident = format_ident!("__RspytsPython{}", resource_name);
    let register_ident = format_ident!("__rspyts_register_python_resource_{}", resource_name);
    let constructors = exported_resource_constructors(item)?;
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
                let __rspyts_inner = match #constructor_call {
                    Ok(__rspyts_value) => __rspyts_value,
                    Err(__rspyts_error) => return Err(
                        ::rspyts::bridge::python::contract_error(&__rspyts_error),
                    ),
                };
                Ok(#wrapper_ident { inner: Some(__rspyts_inner) })
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
    let methods = exported_resource_methods(item, false)?
        .into_iter()
        .map(|method| python_resource_method(resource_ty, method))
        .collect::<syn::Result<Vec<_>>>()?;

    Ok(quote! {
        #[cfg(not(target_arch = "wasm32"))]
        #[::rspyts::__private::pyo3::pyclass(name = #native_name)]
        #[pyo3(crate = "::rspyts::__private::pyo3")]
        struct #wrapper_ident {
            inner: Option<#resource_ty>,
        }

        #[cfg(not(target_arch = "wasm32"))]
        #[::rspyts::__private::pyo3::pymethods]
        #[pyo3(crate = "::rspyts::__private::pyo3")]
        impl #wrapper_ident {
            #[new]
            fn new<'py>(#(#constructor_declarations),*) -> ::rspyts::__private::pyo3::PyResult<Self> {
                #(#constructor_decodes)*
                #constructor_body
            }

            #(#methods)*
            #(#factories)*

            fn close(&mut self) {
                self.inner.take();
            }
        }

        #[cfg(not(target_arch = "wasm32"))]
        fn #register_ident(
            __rspyts_module: &::rspyts::__private::pyo3::Bound<'_, ::rspyts::__private::pyo3::types::PyModule>,
        ) -> ::rspyts::__private::pyo3::PyResult<()> {
            ::rspyts::__private::pyo3::types::PyModuleMethods::add_class::<#wrapper_ident>(__rspyts_module)
        }

        #[cfg(not(target_arch = "wasm32"))]
        ::rspyts::__private::inventory::submit! {
            ::rspyts::runtime::python::Registration(#register_ident)
        }
    })
}

/// Emit one Python classmethod that constructs a resource.
fn python_resource_factory(
    resource_ty: &SynType,
    constructor: &ImplItemFn,
) -> syn::Result<TokenStream2> {
    let method_ident = &constructor.sig.ident;
    let host_name = apply_case(&method_ident.unraw().to_string(), Some("camelCase"));
    let params = wrapper_params(&constructor.sig.inputs, HostBackend::Python)?;
    let declarations = params.iter().map(|param| &param.declaration);
    let decodes = params.iter().map(|param| &param.decode);
    let calls = params.iter().map(|param| &param.call);
    let call = quote!(#resource_ty::#method_ident(#(#calls),*));
    let options = method_options(&constructor.attrs)?;
    let body = if return_result(&constructor.sig.output, options.error.as_ref())? {
        quote! {
            let __rspyts_inner = match #call {
                Ok(__rspyts_value) => __rspyts_value,
                Err(__rspyts_error) => return Err(
                    ::rspyts::bridge::python::contract_error(&__rspyts_error),
                ),
            };
            Ok(Self { inner: Some(__rspyts_inner) })
        }
    } else {
        quote!(Ok(Self { inner: Some(#call) }))
    };
    Ok(quote! {
        #[staticmethod]
        #[pyo3(name = #host_name)]
        fn #method_ident<'py>(#(#declarations),*) -> ::rspyts::__private::pyo3::PyResult<Self> {
            #(#decodes)*
            #body
        }
    })
}

/// Emit one Python instance method for a resource.
fn python_resource_method(resource_ty: &SynType, method: &ImplItemFn) -> syn::Result<TokenStream2> {
    let method_ident = &method.sig.ident;
    let host_name = apply_case(&method_ident.unraw().to_string(), Some("camelCase"));
    let params = wrapper_params(&method.sig.inputs, HostBackend::Python)?;
    let declarations = params.iter().map(|param| &param.declaration);
    let decodes = params.iter().map(|param| &param.decode);
    let calls = params.iter().map(|param| &param.call);
    let invocation = quote!(__rspyts_inner.#method_ident(#(#calls),*));
    let options = method_options(&method.attrs)?;
    let return_plan = host_return_plan(
        &method.sig.output,
        &invocation,
        HostBackend::Python,
        options.returns.as_deref(),
        options.error.as_ref(),
    )?;
    let return_ty = &return_plan.ty;
    let body = &return_plan.body;
    Ok(quote! {
        #[pyo3(name = #host_name)]
        fn #method_ident<'py>(
            &mut self,
            __rspyts_py: ::rspyts::__private::pyo3::Python<'py>,
            #(#declarations),*
        ) -> ::rspyts::__private::pyo3::PyResult<#return_ty> {
            let __rspyts_inner: &mut #resource_ty = self.inner.as_mut().ok_or_else(|| {
                ::rspyts::__private::pyo3::exceptions::PyRuntimeError::new_err("resource is closed")
            })?;
            #(#decodes)*
            #body
        }
    })
}

/// Emit the wasm-bindgen class, factories, and methods for a resource.
fn wasm_resource_wrapper(item: &ItemImpl, native_name: &str) -> syn::Result<TokenStream2> {
    let resource_ty = item.self_ty.as_ref();
    let resource_name = type_last_ident(resource_ty)?.to_string();
    let wrapper_ident = format_ident!("RspytsWasm{}", resource_name);
    let module_ident = format_ident!("__rspyts_wasm_resource_{}", resource_name.to_snake_case());
    let constructors = exported_resource_constructors(item)?;
    let constructor = primary_constructor(&constructors)?;
    let constructor_ident = &constructor.sig.ident;
    let constructor_options = method_options(&constructor.attrs)?;
    let constructor_params = wrapper_params(&constructor.sig.inputs, HostBackend::Wasm)?;
    let constructor_declarations = constructor_params.iter().map(|param| &param.declaration);
    let constructor_decodes = constructor_params.iter().map(|param| &param.decode);
    let constructor_calls = constructor_params.iter().map(|param| &param.call);
    let constructor_call = quote!(#resource_ty::#constructor_ident(#(#constructor_calls),*));
    let constructor_body =
        if return_result(&constructor.sig.output, constructor_options.error.as_ref())? {
            quote! {
                let __rspyts_inner = match #constructor_call {
                    Ok(__rspyts_value) => __rspyts_value,
                    Err(__rspyts_error) => return Err(
                        ::rspyts::bridge::wasm::contract_error(&__rspyts_error),
                    ),
                };
                Ok(#wrapper_ident { inner: Some(__rspyts_inner) })
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
    let methods = exported_resource_methods(item, false)?
        .into_iter()
        .map(|method| wasm_resource_method(resource_ty, method))
        .collect::<syn::Result<Vec<_>>>()?;

    Ok(quote! {
        #[cfg(target_arch = "wasm32")]
        mod #module_ident {
            use super::*;
            use ::rspyts::__private::wasm_bindgen::{self, prelude::wasm_bindgen};

            #[doc(hidden)]
            #[allow(missing_docs)]
            #[wasm_bindgen(
                js_name = #native_name,
                wasm_bindgen = ::rspyts::__private::wasm_bindgen
            )]
            pub struct #wrapper_ident {
                inner: Option<#resource_ty>,
            }

            #[doc(hidden)]
            #[allow(missing_docs)]
            #[wasm_bindgen(
                js_class = #native_name,
                wasm_bindgen = ::rspyts::__private::wasm_bindgen
            )]
            impl #wrapper_ident {
                #[wasm_bindgen(constructor)]
                pub fn new(#(#constructor_declarations),*) -> ::std::result::Result<
                    Self,
                    ::rspyts::__private::wasm_bindgen::JsValue
                > {
                    #(#constructor_decodes)*
                    #constructor_body
                }

                #(#methods)*
                #(#factories)*

                pub fn close(&mut self) {
                    self.inner.take();
                }
            }
        }
    })
}

/// Emit one JavaScript static factory that constructs a resource.
fn wasm_resource_factory(
    resource_ty: &SynType,
    constructor: &ImplItemFn,
) -> syn::Result<TokenStream2> {
    let method_ident = &constructor.sig.ident;
    let host_name = apply_case(&method_ident.unraw().to_string(), Some("camelCase"));
    let params = wrapper_params(&constructor.sig.inputs, HostBackend::Wasm)?;
    let declarations = params.iter().map(|param| &param.declaration);
    let decodes = params.iter().map(|param| &param.decode);
    let calls = params.iter().map(|param| &param.call);
    let call = quote!(#resource_ty::#method_ident(#(#calls),*));
    let options = method_options(&constructor.attrs)?;
    let body = if return_result(&constructor.sig.output, options.error.as_ref())? {
        quote! {
            let __rspyts_inner = match #call {
                Ok(__rspyts_value) => __rspyts_value,
                Err(__rspyts_error) => return Err(
                    ::rspyts::bridge::wasm::contract_error(&__rspyts_error),
                ),
            };
            Ok(Self { inner: Some(__rspyts_inner) })
        }
    } else {
        quote!(Ok(Self { inner: Some(#call) }))
    };
    Ok(quote! {
        #[wasm_bindgen(js_name = #host_name)]
        pub fn #method_ident(
            #(#declarations),*
        ) -> ::std::result::Result<Self, ::rspyts::__private::wasm_bindgen::JsValue> {
            #(#decodes)*
            #body
        }
    })
}

/// Emit one JavaScript instance method for a resource.
fn wasm_resource_method(resource_ty: &SynType, method: &ImplItemFn) -> syn::Result<TokenStream2> {
    let method_ident = &method.sig.ident;
    let host_name = apply_case(&method_ident.unraw().to_string(), Some("camelCase"));
    let params = wrapper_params(&method.sig.inputs, HostBackend::Wasm)?;
    let declarations = params.iter().map(|param| &param.declaration);
    let decodes = params.iter().map(|param| &param.decode);
    let calls = params.iter().map(|param| &param.call);
    let invocation = quote!(__rspyts_inner.#method_ident(#(#calls),*));
    let options = method_options(&method.attrs)?;
    let return_plan = host_return_plan(
        &method.sig.output,
        &invocation,
        HostBackend::Wasm,
        options.returns.as_deref(),
        options.error.as_ref(),
    )?;
    let return_ty = &return_plan.ty;
    let body = &return_plan.body;
    Ok(quote! {
        #[wasm_bindgen(js_name = #host_name)]
        pub fn #method_ident(
            &mut self,
            #(#declarations),*
        ) -> ::std::result::Result<#return_ty, ::rspyts::__private::wasm_bindgen::JsValue> {
            let __rspyts_inner: &mut #resource_ty = self.inner.as_mut().ok_or_else(|| {
                ::rspyts::__private::wasm_bindgen::JsValue::from_str("resource is closed")
            })?;
            #(#decodes)*
            #body
        }
    })
}

/// Collect resource constructors while preserving attribute parse errors.
fn exported_resource_constructors(item: &ItemImpl) -> syn::Result<Vec<&ImplItemFn>> {
    let constructors = exported_resource_methods(item, true)?;
    if constructors.is_empty() {
        return Err(syn::Error::new(
            item.self_ty.span(),
            "an exported resource needs a constructor for each enabled backend",
        ));
    }
    Ok(constructors)
}

/// Collect public methods in one resource export category.
fn exported_resource_methods(item: &ItemImpl, constructors: bool) -> syn::Result<Vec<&ImplItemFn>> {
    let mut methods = Vec::new();
    for item in &item.items {
        let ImplItem::Fn(method) = item else {
            continue;
        };
        if matches!(method.vis, syn::Visibility::Public(_))
            && method_exported(method, constructors)?
        {
            methods.push(method);
        }
    }
    Ok(methods)
}

/// Select `new`, or otherwise the first exported resource constructor.
fn primary_constructor<'a>(constructors: &[&'a ImplItemFn]) -> syn::Result<&'a ImplItemFn> {
    constructors
        .iter()
        .copied()
        .find(|method| method.sig.ident == "new")
        .or_else(|| constructors.first().copied())
        .ok_or_else(|| syn::Error::new(Span::call_site(), "resource has no constructor"))
}

/// Return whether a callable uses a validated `Result` return contract.
fn return_result(output: &ReturnType, declared_error: Option<&SynType>) -> syn::Result<bool> {
    Ok(resolved_result_types(output, declared_error)?.is_some())
}

/// Render one resource constructor's host-neutral metadata.
fn resource_constructor_tokens(
    method: &mut ImplItemFn,
    resource_ty: &SynType,
    native_name: &str,
    declared_error: Option<&SynType>,
) -> syn::Result<TokenStream2> {
    let rust_name = method.sig.ident.unraw().to_string();
    let host_name = apply_case(&rust_name, Some("camelCase"));
    let docs = docs_tokens(&method.attrs);
    let params = params_tokens(&mut method.sig.inputs)?;
    let (_, error) = return_tokens(&method.sig.output, None, declared_error)?;
    Ok(quote!(::rspyts::ir::FunctionDef {
        owner: ::rspyts::ir::CargoPackageId::new(env!("CARGO_PKG_NAME")),
        rust_module: module_path!().to_owned(),
        rust_name: #rust_name.to_owned(),
        host_name: #host_name.to_owned(),
        native_name: #native_name.to_owned(),
        docs: #docs,
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

/// Render one resource method's host-neutral metadata.
fn resource_method_tokens(
    method: &mut ImplItemFn,
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
    let rust_name = method.sig.ident.unraw().to_string();
    let host_name = apply_case(&rust_name, Some("camelCase"));
    let docs = docs_tokens(&method.attrs);
    let params = params_tokens(&mut method.sig.inputs)?;
    let (returns, error) = return_tokens(&method.sig.output, return_boundary, declared_error)?;
    Ok(quote!(::rspyts::ir::MethodDef {
        rust_name: #rust_name.to_owned(),
        host_name: #host_name.to_owned(),
        docs: #docs,
        params: vec![#(#params),*],
        returns: #returns,
        error: #error,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wasm_param(source: ItemFn) -> WrapperParam {
        wrapper_params(&source.sig.inputs, HostBackend::Wasm)
            .expect("generate Wasm wrapper parameter")
            .into_iter()
            .next()
            .expect("one wrapper parameter")
    }

    fn python_param(source: ItemFn) -> WrapperParam {
        wrapper_params(&source.sig.inputs, HostBackend::Python)
            .expect("generate Python wrapper parameter")
            .into_iter()
            .next()
            .expect("one wrapper parameter")
    }

    #[test]
    fn bytes_slice_uses_the_direct_wasm_vector_abi() {
        let param = wasm_param(syn::parse_quote! {
            fn boundary(#[rspyts(bytes)] input: &[u8]) {}
        });

        assert_eq!(param.declaration.to_string(), "input : & [u8]");
        assert!(param.decode.is_empty());
        assert_eq!(param.call.to_string(), "input");
    }

    #[test]
    fn owned_bytes_use_the_direct_wasm_vector_abi() {
        let param = wasm_param(syn::parse_quote! {
            fn boundary(#[rspyts(bytes)] input: Vec<u8>) {}
        });

        assert_eq!(param.declaration.to_string(), "input : Vec < u8 >");
        assert!(param.decode.is_empty());
        assert_eq!(param.call.to_string(), "input");
    }

    #[test]
    fn numeric_buffer_uses_the_direct_wasm_vector_abi() {
        let param = wasm_param(syn::parse_quote! {
            fn boundary(#[rspyts(buffer)] input: &[f64]) {}
        });

        assert_eq!(param.declaration.to_string(), "input : & [f64]");
        assert!(param.decode.is_empty());
        assert_eq!(param.call.to_string(), "input");
    }

    #[test]
    fn fixed_bytes_validate_length_after_the_direct_wasm_vector_abi() {
        let param = wasm_param(syn::parse_quote! {
            fn boundary(#[rspyts(bytes)] input: &[u8; 16]) {}
        });

        assert!(
            param
                .declaration
                .to_string()
                .contains("Vec < :: core :: primitive :: u8 >")
        );
        assert!(param.decode.to_string().contains("must contain exactly"));
        assert_eq!(param.call.to_string(), "& input");
    }

    #[test]
    fn ordinary_inputs_keep_the_serde_wasm_boundary() {
        let param = wasm_param(syn::parse_quote! {
            fn boundary(input: String) {}
        });

        assert!(param.declaration.to_string().contains("JsValue"));
        assert!(param.decode.to_string().contains("wasm :: from_host"));
    }

    #[test]
    fn python_bytes_and_buffers_bypass_serde() {
        let bytes = python_param(syn::parse_quote! {
            fn boundary(#[rspyts(bytes)] input: &[u8]) {}
        });
        let buffer = python_param(syn::parse_quote! {
            fn boundary(#[rspyts(buffer)] input: &[f64]) {}
        });

        assert!(bytes.decode.to_string().contains("bytes_from_host"));
        assert!(!bytes.decode.to_string().contains("python :: from_host"));
        assert!(buffer.decode.to_string().contains("buffer_from_host"));
    }

    #[test]
    fn annotated_wasm_returns_use_the_vector_abi() {
        let function: ItemFn = syn::parse_quote! {
            fn boundary() -> Vec<u8> { Vec::new() }
        };
        let plan = host_return_plan(
            &function.sig.output,
            &quote!(boundary()),
            HostBackend::Wasm,
            Some("bytes"),
            None,
        )
        .expect("generate direct return");

        assert!(
            plan.ty
                .to_string()
                .contains("Vec < :: core :: primitive :: u8 >")
        );
        assert!(!plan.body.to_string().contains("wasm :: to_host"));
    }

    #[test]
    fn nested_parameter_references_are_rejected_clearly() {
        let function: ItemFn = syn::parse_quote! {
            fn boundary(input: Option<&str>) {}
        };
        let error = wrapper_params(&function.sig.inputs, HostBackend::Wasm)
            .err()
            .expect("nested borrow must be rejected");

        assert!(error.to_string().contains("outermost level"));
    }

    #[test]
    fn resource_constructor_collection_preserves_attribute_errors() {
        let implementation: ItemImpl = syn::parse_quote! {
            impl Resource {
                #[rspyts(unknown)]
                pub fn new() -> Self {
                    Self
                }
            }
        };

        let error = match exported_resource_constructors(&implementation) {
            Ok(_) => panic!("invalid constructor attributes must be reported"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("method attributes are"));
    }
}
