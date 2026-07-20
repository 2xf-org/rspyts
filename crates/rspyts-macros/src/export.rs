use super::*;
use crate::{attributes::*, types::*};

pub(super) fn expand_export(item: Item) -> syn::Result<TokenStream2> {
    match item {
        Item::Fn(function) => expand_function(function),
        Item::Impl(item_impl) => expand_resource(item_impl),
        Item::Const(item_const) => expand_const(item_const),
        Item::Static(item_static) => expand_static(item_static),
        other => Err(syn::Error::new(
            other.span(),
            "`#[rspyts::export]` supports public functions, inherent impl blocks, consts, and statics",
        )),
    }
}

fn expand_function(mut function: ItemFn) -> syn::Result<TokenStream2> {
    ensure_public(&function.vis, function.sig.ident.span())?;
    reject_signature(&function.sig)?;
    let options = take_function_options(&mut function.attrs)?;
    let ident = &function.sig.ident;
    let rust_name = ident.to_string();
    let host_name = apply_case(&rust_name, Some("camelCase"));
    let docs = docs_tokens(&function.attrs);
    let python_wrapper = python_function_wrapper(
        &function,
        options.returns.as_deref(),
        options.error.as_ref(),
    )?;
    let wasm_wrapper = wasm_function_wrapper(
        &function,
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
                    rust_name: #rust_name.to_owned(),
                    host_name: #host_name.to_owned(),
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
        #[cfg(not(target_arch = "wasm32"))]
        #[::rspyts::__private::pyo3::pyfunction(name = #host_name)]
        #[pyo3(crate = "::rspyts::__private::pyo3")]
        fn #wrapper_ident<'py>(
            __rspyts_py: ::rspyts::__private::pyo3::Python<'py>,
            #(#declarations),*
        ) -> ::rspyts::__private::pyo3::PyResult<
            ::rspyts::__private::pyo3::Py<::rspyts::__private::pyo3::PyAny>
        > {
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

fn wasm_function_wrapper(
    function: &ItemFn,
    return_boundary: Option<&str>,
    declared_error: Option<&SynType>,
) -> syn::Result<TokenStream2> {
    let function_ident = &function.sig.ident;
    let wrapper_ident = format_ident!("__rspyts_wasm_{}", function_ident);
    let module_ident = format_ident!("__rspyts_wasm_function_{}", function_ident);
    let host_name = apply_case(&function_ident.to_string(), Some("camelCase"));
    let native_host_name = wasm_native_host_name(&host_name);
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
        #[cfg(target_arch = "wasm32")]
        mod #module_ident {
            use super::*;
            use ::rspyts::__private::wasm_bindgen::{self, prelude::wasm_bindgen};

            #[doc(hidden)]
            #[allow(missing_docs)]
            #[wasm_bindgen(
                js_name = #native_host_name,
                wasm_bindgen = ::rspyts::__private::wasm_bindgen
            )]
            pub fn #wrapper_ident(
                #(#declarations),*
            ) -> ::std::result::Result<
                ::rspyts::__private::wasm_bindgen::JsValue,
                ::rspyts::__private::wasm_bindgen::JsValue
            > {
                #(#decodes)*
                #body
            }
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
            let _boundary = boundary_attr(&argument.attrs)?;
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
    _return_boundary: Option<&str>,
    declared_error: Option<&SynType>,
) -> syn::Result<TokenStream2> {
    let result = resolved_result_types(output, declared_error)?.is_some();
    match (backend, result) {
        (HostBackend::Python, true) => Ok(quote! {
            match #invocation {
                Ok(__rspyts_value) => ::rspyts::bridge::python::to_host(
                    __rspyts_py,
                    &__rspyts_value,
                ),
                Err(__rspyts_error) => Err(::rspyts::bridge::python::contract_error(
                    &__rspyts_error,
                )),
            }
        }),
        (HostBackend::Python, false) => Ok(quote! {
            let __rspyts_value = #invocation;
            ::rspyts::bridge::python::to_host(
                __rspyts_py,
                &__rspyts_value,
            )
        }),
        (HostBackend::Wasm, true) => Ok(quote! {
            match #invocation {
                Ok(__rspyts_value) => ::rspyts::bridge::wasm::to_host(&__rspyts_value),
                Err(__rspyts_error) => Err(::rspyts::bridge::wasm::contract_error(
                    &__rspyts_error,
                )),
            }
        }),
        (HostBackend::Wasm, false) => Ok(quote! {
            let __rspyts_value = #invocation;
            ::rspyts::bridge::wasm::to_host(&__rspyts_value)
        }),
    }
}

fn expand_const(item: ItemConst) -> syn::Result<TokenStream2> {
    ensure_public(&item.vis, item.ident.span())?;
    let docs = docs_tokens(&item.attrs);
    constant_tokens(quote!(#item), &item.ident, &item.ty, docs)
}

fn expand_static(item: ItemStatic) -> syn::Result<TokenStream2> {
    ensure_public(&item.vis, item.ident.span())?;
    let docs = docs_tokens(&item.attrs);
    constant_tokens(quote!(#item), &item.ident, &item.ty, docs)
}

fn constant_tokens(
    item: TokenStream2,
    ident: &Ident,
    ty: &SynType,
    docs: TokenStream2,
) -> syn::Result<TokenStream2> {
    let rust_name = ident.to_string();
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
    let resource_name = type_last_ident(&resource_ty)?.to_string();
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
    let python_wrapper = python_resource_wrapper(&wrapper_source)?;
    let wasm_wrapper = wasm_resource_wrapper(&wrapper_source)?;
    Ok(quote! {
        #item
        const _: () = {
            fn __rspyts_resource_registration() -> ::rspyts::ir::ResourceDef {
                ::rspyts::ir::ResourceDef {
                    name: #resource_name.to_owned(),
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

fn python_resource_wrapper(item: &ItemImpl) -> syn::Result<TokenStream2> {
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
    let methods = item
        .items
        .iter()
        .filter_map(|item| match item {
            ImplItem::Fn(method)
                if matches!(method.vis, syn::Visibility::Public(_))
                    && method_exported(method, false).unwrap_or(false) =>
            {
                Some(method)
            }
            _ => None,
        })
        .map(|method| python_resource_method(resource_ty, method))
        .collect::<syn::Result<Vec<_>>>()?;

    Ok(quote! {
        #[cfg(not(target_arch = "wasm32"))]
        #[::rspyts::__private::pyo3::pyclass(name = #resource_name)]
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

fn python_resource_method(resource_ty: &SynType, method: &ImplItemFn) -> syn::Result<TokenStream2> {
    let method_ident = &method.sig.ident;
    let host_name = apply_case(&method_ident.to_string(), Some("camelCase"));
    let params = wrapper_params(&method.sig.inputs, HostBackend::Python)?;
    let declarations = params.iter().map(|param| &param.declaration);
    let decodes = params.iter().map(|param| &param.decode);
    let calls = params.iter().map(|param| &param.call);
    let invocation = quote!(__rspyts_inner.#method_ident(#(#calls),*));
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
            __rspyts_py: ::rspyts::__private::pyo3::Python<'py>,
            #(#declarations),*
        ) -> ::rspyts::__private::pyo3::PyResult<
            ::rspyts::__private::pyo3::Py<::rspyts::__private::pyo3::PyAny>
        > {
            let __rspyts_inner: &mut #resource_ty = self.inner.as_mut().ok_or_else(|| {
                ::rspyts::__private::pyo3::exceptions::PyRuntimeError::new_err("resource is closed")
            })?;
            #(#decodes)*
            #body
        }
    })
}

fn wasm_resource_wrapper(item: &ItemImpl) -> syn::Result<TokenStream2> {
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
    let methods = item
        .items
        .iter()
        .filter_map(|item| match item {
            ImplItem::Fn(method)
                if matches!(method.vis, syn::Visibility::Public(_))
                    && method_exported(method, false).unwrap_or(false) =>
            {
                Some(method)
            }
            _ => None,
        })
        .map(|method| wasm_resource_method(resource_ty, method))
        .collect::<syn::Result<Vec<_>>>()?;

    Ok(quote! {
        #[cfg(target_arch = "wasm32")]
        mod #module_ident {
            use super::*;
            use ::rspyts::__private::wasm_bindgen::{self, prelude::wasm_bindgen};

            #[doc(hidden)]
            #[allow(missing_docs)]
            #[wasm_bindgen(wasm_bindgen = ::rspyts::__private::wasm_bindgen)]
            pub struct #wrapper_ident {
                inner: Option<#resource_ty>,
            }

            #[doc(hidden)]
            #[allow(missing_docs)]
            #[wasm_bindgen(wasm_bindgen = ::rspyts::__private::wasm_bindgen)]
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

fn wasm_resource_method(resource_ty: &SynType, method: &ImplItemFn) -> syn::Result<TokenStream2> {
    let method_ident = &method.sig.ident;
    let host_name = apply_case(&method_ident.to_string(), Some("camelCase"));
    let params = wrapper_params(&method.sig.inputs, HostBackend::Wasm)?;
    let declarations = params.iter().map(|param| &param.declaration);
    let decodes = params.iter().map(|param| &param.decode);
    let calls = params.iter().map(|param| &param.call);
    let invocation = quote!(__rspyts_inner.#method_ident(#(#calls),*));
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
            let __rspyts_inner: &mut #resource_ty = self.inner.as_mut().ok_or_else(|| {
                ::rspyts::__private::wasm_bindgen::JsValue::from_str("resource is closed")
            })?;
            #(#decodes)*
            #body
        }
    })
}

fn exported_resource_constructors(item: &ItemImpl) -> syn::Result<Vec<&ImplItemFn>> {
    let constructors = item
        .items
        .iter()
        .filter_map(|item| match item {
            ImplItem::Fn(method) if method_exported(method, true).unwrap_or(false) => Some(method),
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
    declared_error: Option<&SynType>,
) -> syn::Result<TokenStream2> {
    let rust_name = method.sig.ident.to_string();
    let host_name = apply_case(&rust_name, Some("camelCase"));
    let docs = docs_tokens(&method.attrs);
    let params = params_tokens(&mut method.sig.inputs)?;
    let (_, error) = return_tokens(&method.sig.output, None, declared_error)?;
    Ok(quote!(::rspyts::ir::FunctionDef {
        rust_name: #rust_name.to_owned(),
        host_name: #host_name.to_owned(),
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
    let rust_name = method.sig.ident.to_string();
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
