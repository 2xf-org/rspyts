//! Expansion of `#[bridge]` on inherent impl blocks — opaque classes
//! (type-system §7, ABI §3/§8).
//!
//! The impl block is re-emitted with the `#[bridge(…)]` method markers
//! stripped, followed by:
//!
//! - one `static __RSPYTS_SLAB_{Type}: Slab<Type>` holding live instances;
//! - `rspyts_cls__{Type}__new` — decodes args, runs the constructor, and
//!   returns the freshly inserted handle (a `u64`, which serializes as a
//!   JSON number); omitted for factory-only classes;
//! - `rspyts_cls__{Type}__{name}` — one shim per method (handle first,
//!   locking the object for the duration of the call) and per
//!   `#[bridge(static)]` method (no handle; factories returning `Self` or
//!   `Result<Self, E>` insert into the slab and return the handle);
//! - `rspyts_cls__{Type}__drop` — idempotent destruction;
//! - an inventory registration building the `ClassDecl` at manifest time.

use crate::attrs::{BridgeArgs, TargetArg, is_bridge_attr};
use crate::docs::extract_docs;
use crate::emit;
use crate::sig;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};

/// Everything recorded about one bridged method (constructor, instance
/// method, or static) before emission, so the mutable pass over the impl
/// items can finish first.
struct MethodInfo {
    ident: syn::Ident,
    docs: String,
    params: Vec<sig::BridgedParam>,
    ret: sig::RetKind,
    /// `&mut self` (instance methods only).
    mutable: bool,
    /// Statics only: returns `Self` / `Result<Self, E>` — a factory whose
    /// shim inserts into the slab and returns the handle.
    returns_self: bool,
    /// `#[bridge(target = "…")]` scoping, when present.
    target: Option<TargetArg>,
}

/// The `#[bridge(…)]` marker found on one method of the impl block.
enum Marker {
    /// No marker (or just `target = …`): an ordinary instance method.
    Plain(Option<TargetArg>),
    Ctor,
    Static(Option<TargetArg>),
}

pub fn expand_impl(args: BridgeArgs, mut item: syn::ItemImpl) -> syn::Result<TokenStream> {
    args.deny_error("impl blocks")?;
    args.deny_constructor()?;
    args.deny_static("the impl block itself; mark the individual method")?;
    args.deny_tag("impl blocks")?;
    args.deny_rename_all("impl blocks")?;
    args.deny_serde("impl blocks; it adopts Serde derives on data types")?;
    // `target = "…"` on the impl block sets the default for every method
    // and static; a member's own `target` overrides it. The constructor
    // (and the class itself) is never scoped — `CtorDecl` carries no
    // targets — so impl-level scoping cannot silently hide construction.
    let default_target = args.target.map(|(target, _)| target);

    if let Some((_, trait_path, _)) = &item.trait_ {
        return Err(syn::Error::new_spanned(
            trait_path,
            "#[bridge] impl blocks must be inherent; trait impls are not supported",
        ));
    }
    if let Some(token) = &item.unsafety {
        return Err(syn::Error::new_spanned(
            token,
            "unsafe impls cannot be bridged",
        ));
    }
    sig::ensure_no_generics(&item.generics, "impl blocks")?;
    let ty_ident = self_ty_ident(&item.self_ty)?;

    let impl_docs = extract_docs(&item.attrs);
    let mut ctor: Option<MethodInfo> = None;
    let mut methods: Vec<MethodInfo> = Vec::new();
    let mut statics: Vec<MethodInfo> = Vec::new();

    for impl_item in &mut item.items {
        let method = match impl_item {
            syn::ImplItem::Fn(method) => method,
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "only methods may appear in a #[bridge] impl block; move other \
                     items to a separate impl",
                ));
            }
        };
        let marker = take_method_marker(method)?;
        sig::ensure_plain_signature(&method.sig, "methods")?;

        match marker {
            Marker::Ctor => {
                if ctor.is_some() {
                    return Err(syn::Error::new_spanned(
                        &method.sig.ident,
                        "duplicate #[bridge(constructor)] — a class has exactly one constructor",
                    ));
                }
                ctor = Some(analyze_ctor(method, &ty_ident)?);
            }
            Marker::Static(target) => statics.push(analyze_static(
                method,
                &ty_ident,
                target.or(default_target),
            )?),
            Marker::Plain(target) => {
                methods.push(analyze_method(method, target.or(default_target))?)
            }
        }
    }

    if ctor.is_none() && !statics.iter().any(|st| st.returns_self) {
        return Err(syn::Error::new_spanned(
            &ty_ident,
            "class has no way to be constructed — mark one method \
             #[bridge(constructor)] or add a #[bridge(static)] factory \
             returning `Self` (docs/design/type-system.md §7)",
        ));
    }

    // Methods and statics share the `rspyts_cls__{Type}__{name}` symbol
    // namespace. (rustc also rejects duplicate fn names in one impl block;
    // this keeps the diagnostic ours and about the generated symbol.)
    {
        let mut seen = std::collections::HashSet::new();
        for info in methods.iter().chain(statics.iter()) {
            if !seen.insert(info.ident.to_string()) {
                return Err(syn::Error::new_spanned(
                    &info.ident,
                    format!(
                        "duplicate bridged member `{}` — methods and statics share the \
                         `rspyts_cls__{{Type}}__{{name}}` symbol namespace",
                        info.ident
                    ),
                ));
            }
        }
    }

    let slab = format_ident!("__RSPYTS_SLAB_{}", ty_ident);
    let ctor_shim = ctor.as_ref().map(|ctor| ctor_shim(&ty_ident, &slab, ctor));
    let method_shims: Vec<TokenStream> = methods
        .iter()
        .map(|method| method_shim(&ty_ident, &slab, method))
        .collect();
    let static_shims: Vec<TokenStream> = statics
        .iter()
        .map(|st| static_shim(&ty_ident, &slab, st))
        .collect();
    let drop_symbol = format_ident!("rspyts_cls__{}__drop", ty_ident);
    let registration = class_registration(&ty_ident, &impl_docs, ctor.as_ref(), &methods, &statics);

    Ok(quote! {
        #item

        #[doc(hidden)]
        #[allow(non_upper_case_globals)]
        static #slab: ::rspyts::__private::Slab<#ty_ident> = ::rspyts::__private::Slab::new();

        #ctor_shim

        #(#method_shims)*

        #(#static_shims)*

        #[unsafe(no_mangle)]
        #[doc(hidden)]
        #[allow(non_snake_case)]
        pub extern "C" fn #drop_symbol(__rspyts_handle: u64) {
            ::rspyts::__private::shim::run_drop(|| #slab.remove(__rspyts_handle));
        }

        #registration
    })
}

/// The impl target must be a bare, non-generic type name: the class name
/// appears verbatim in exported symbols and the manifest.
fn self_ty_ident(self_ty: &syn::Type) -> syn::Result<syn::Ident> {
    if let syn::Type::Path(path) = self_ty {
        if path.qself.is_none() && path.path.segments.len() == 1 {
            let segment = &path.path.segments[0];
            if segment.arguments.is_none() {
                return Ok(segment.ident.clone());
            }
        }
    }
    Err(syn::Error::new_spanned(
        self_ty,
        "#[bridge] impl blocks must target a plain, non-generic type name \
         (write the impl next to the type's definition)",
    ))
}

/// Strip the `#[bridge(…)]` marker from a method and classify it. Valid
/// spellings: `#[bridge(constructor)]`, `#[bridge(static)]`,
/// `#[bridge(target = "…")]`, and `#[bridge(static, target = "…")]`.
fn take_method_marker(method: &mut syn::ImplItemFn) -> syn::Result<Marker> {
    let mut marker = Marker::Plain(None);
    let mut seen = false;
    let mut kept = Vec::with_capacity(method.attrs.len());
    for attr in method.attrs.drain(..) {
        if !is_bridge_attr(&attr) {
            kept.push(attr);
            continue;
        }
        if seen {
            return Err(syn::Error::new_spanned(
                &attr,
                "duplicate #[bridge] attribute on a method",
            ));
        }
        seen = true;
        let nested = match &attr.meta {
            syn::Meta::List(list) => list.tokens.clone(),
            _ => TokenStream::new(),
        };
        let margs = BridgeArgs::parse(nested)?;
        margs.deny_error("methods")?;
        margs.deny_tag("methods")?;
        margs.deny_rename_all("methods")?;
        margs.deny_serde("methods; it adopts Serde derives on data types")?;
        marker = match (margs.constructor, margs.statik, margs.target) {
            (Some(_), Some(span), _) => {
                return Err(syn::Error::new(
                    span,
                    "`constructor` and `static` are mutually exclusive",
                ));
            }
            (Some(_), None, Some((_, span))) => {
                return Err(syn::Error::new(
                    span,
                    "`target` does not apply to constructors — the constructor \
                     exists in every projection",
                ));
            }
            (Some(_), None, None) => Marker::Ctor,
            (None, Some(_), target) => Marker::Static(target.map(|(target, _)| target)),
            (None, None, Some((target, _))) => Marker::Plain(Some(target)),
            (None, None, None) => {
                return Err(syn::Error::new_spanned(
                    &attr,
                    "an empty #[bridge] marker does nothing on a method — expected \
                     `constructor`, `static`, or `target = \"…\"`",
                ));
            }
        };
    }
    method.attrs = kept;
    Ok(marker)
}

fn analyze_ctor(method: &syn::ImplItemFn, ty_ident: &syn::Ident) -> syn::Result<MethodInfo> {
    if let Some(receiver) = method.sig.receiver() {
        return Err(syn::Error::new_spanned(
            receiver,
            "the constructor must not take `self`; it produces the instance",
        ));
    }
    let ret = sig::classify_ret(&method.sig.output);
    let valid = match &ret {
        sig::RetKind::Plain(ty) => sig::is_self_ty(ty, ty_ident),
        sig::RetKind::Result { ok, .. } => sig::is_self_ty(ok, ty_ident),
        sig::RetKind::Unit => false,
    };
    if !valid {
        return Err(syn::Error::new_spanned(
            &method.sig.output,
            "the constructor must return `Self` or `Result<Self, E>` \
             (docs/design/type-system.md §7)",
        ));
    }
    let params = sig::bridged_params(method.sig.inputs.iter())?;
    sig::validate_param_wire_names(&params)?;
    Ok(MethodInfo {
        ident: method.sig.ident.clone(),
        docs: extract_docs(&method.attrs),
        params,
        ret,
        mutable: false,
        returns_self: true,
        target: None,
    })
}

fn analyze_static(
    method: &syn::ImplItemFn,
    ty_ident: &syn::Ident,
    target: Option<TargetArg>,
) -> syn::Result<MethodInfo> {
    if let Some(receiver) = method.sig.receiver() {
        return Err(syn::Error::new_spanned(
            receiver,
            "a #[bridge(static)] method must not take `self`; drop the `static` \
             marker to bridge it as an instance method",
        ));
    }
    ensure_not_reserved(&method.sig.ident)?;
    let ret = sig::classify_ret(&method.sig.output);
    let returns_self = match &ret {
        sig::RetKind::Plain(ty) => sig::is_self_ty(ty, ty_ident),
        sig::RetKind::Result { ok, .. } => sig::is_self_ty(ok, ty_ident),
        sig::RetKind::Unit => false,
    };
    let params = sig::bridged_params(method.sig.inputs.iter())?;
    sig::validate_param_wire_names(&params)?;
    Ok(MethodInfo {
        ident: method.sig.ident.clone(),
        docs: extract_docs(&method.attrs),
        params,
        ret,
        mutable: false,
        returns_self,
        target,
    })
}

fn analyze_method(method: &syn::ImplItemFn, target: Option<TargetArg>) -> syn::Result<MethodInfo> {
    let Some(receiver) = method.sig.receiver() else {
        return Err(syn::Error::new_spanned(
            &method.sig.ident,
            "a method without `self` must be marked #[bridge(constructor)] or \
             #[bridge(static)]; instance methods take `&self` or `&mut self`",
        ));
    };
    if receiver.reference.is_none() || receiver.colon_token.is_some() {
        return Err(syn::Error::new_spanned(
            receiver,
            "methods must take `&self` or `&mut self`; `self` by value is not \
             supported (docs/design/type-system.md §7)",
        ));
    }
    ensure_not_reserved(&method.sig.ident)?;
    let params = sig::bridged_params(method.sig.inputs.iter().skip(1))?;
    sig::validate_param_wire_names(&params)?;
    Ok(MethodInfo {
        ident: method.sig.ident.clone(),
        docs: extract_docs(&method.attrs),
        params,
        ret: sig::classify_ret(&method.sig.output),
        mutable: receiver.mutability.is_some(),
        returns_self: false,
        target,
    })
}

/// `new` and `drop` are reserved: the generated constructor and destructor
/// symbols are `rspyts_cls__{Type}__new` / `__drop`.
fn ensure_not_reserved(ident: &syn::Ident) -> syn::Result<()> {
    for reserved in ["new", "drop"] {
        if ident == reserved {
            return Err(syn::Error::new_spanned(
                ident,
                format!(
                    "method name `{reserved}` collides with the generated \
                     `rspyts_cls__{{Type}}__{reserved}` symbol; rename the method"
                ),
            ));
        }
    }
    Ok(())
}

/// `rspyts_cls__{Type}__new`: run the constructor, insert the instance
/// into the slab, and return the handle (envelope JSON payload is the
/// handle as a number, ABI §3).
fn ctor_shim(ty_ident: &syn::Ident, slab: &syn::Ident, ctor: &MethodInfo) -> TokenStream {
    let symbol = format_ident!("rspyts_cls__{}__new", ty_ident);
    factory_shim(ty_ident, slab, ctor, symbol)
}

/// `rspyts_cls__{Type}__{name}` for a `#[bridge(static)]` method: no handle
/// parameter. Factories (returning `Self`/`Result<Self, E>`) insert into
/// the slab and return the fresh handle, exactly like the constructor;
/// anything else encodes its return value ordinarily.
fn static_shim(ty_ident: &syn::Ident, slab: &syn::Ident, st: &MethodInfo) -> TokenStream {
    let symbol = format_ident!("rspyts_cls__{}__{}", ty_ident, st.ident);
    if st.returns_self {
        return factory_shim(ty_ident, slab, st, symbol);
    }

    let args_ident = format_ident!("__RspytsArgs_{}__{}", ty_ident, st.ident);
    let args_struct = emit::args_struct(&args_ident, &st.params);
    let bindings = emit::shim_bindings(&args_ident, &st.params);
    let c_params = &bindings.c_params;
    let prelude = &bindings.prelude;
    let call_args = &bindings.call_args;
    let static_ident = &st.ident;

    let call = quote!(#ty_ident::#static_ident(#(#call_args),*));
    let mapped = if st.ret.is_result() {
        quote!(::rspyts::__private::shim::map_result(#call))
    } else {
        quote!(::rspyts::__private::shim::map_plain(#call))
    };

    quote! {
        #args_struct

        /// # Safety
        /// Every pointer/length pair (`args_ptr`/`args_len` and each
        /// slice pair) must describe valid, initialized memory for the
        /// duration of the call, per ABI §3.1.
        #[unsafe(no_mangle)]
        #[doc(hidden)]
        #[allow(non_snake_case, clippy::too_many_arguments, clippy::unit_arg)]
        pub unsafe extern "C" fn #symbol(#(#c_params),*) -> *mut u8 {
            ::rspyts::__private::shim::run(|| {
                #prelude
                #mapped
            })
        }
    }
}

/// Shared body of the constructor shim and factory-static shims: run the
/// producing function, insert the instance into the slab, return the
/// handle.
fn factory_shim(
    ty_ident: &syn::Ident,
    slab: &syn::Ident,
    info: &MethodInfo,
    symbol: syn::Ident,
) -> TokenStream {
    let args_ident = format_ident!("__RspytsArgs_{}__{}", ty_ident, info.ident);
    let args_struct = emit::args_struct(&args_ident, &info.params);
    let bindings = emit::shim_bindings(&args_ident, &info.params);
    let c_params = &bindings.c_params;
    let prelude = &bindings.prelude;
    let call_args = &bindings.call_args;
    let fn_ident = &info.ident;

    let call = quote!(#ty_ident::#fn_ident(#(#call_args),*));
    let obj = if info.ret.is_result() {
        quote!(::rspyts::__private::shim::map_result(#call)?)
    } else {
        call
    };

    quote! {
        #args_struct

        /// # Safety
        /// Every pointer/length pair (`args_ptr`/`args_len` and each
        /// slice pair) must describe valid, initialized memory for the
        /// duration of the call, per ABI §3.1.
        #[unsafe(no_mangle)]
        #[doc(hidden)]
        #[allow(non_snake_case, clippy::too_many_arguments)]
        pub unsafe extern "C" fn #symbol(#(#c_params),*) -> *mut u8 {
            ::rspyts::__private::shim::run(|| {
                #prelude
                let __rspyts_obj = #obj;
                ::core::result::Result::Ok(#slab.insert(__rspyts_obj))
            })
        }
    }
}

/// `rspyts_cls__{Type}__{method}`: lock the object behind the handle and
/// invoke the method. `&mut self` methods go through `Slab::with_mut`;
/// the lock itself is uniform (ABI §8).
fn method_shim(ty_ident: &syn::Ident, slab: &syn::Ident, method: &MethodInfo) -> TokenStream {
    let method_ident = &method.ident;
    let symbol = format_ident!("rspyts_cls__{}__{}", ty_ident, method_ident);
    let args_ident = format_ident!("__RspytsArgs_{}__{}", ty_ident, method_ident);
    let args_struct = emit::args_struct(&args_ident, &method.params);
    let bindings = emit::shim_bindings(&args_ident, &method.params);
    let c_params = &bindings.c_params;
    let prelude = &bindings.prelude;
    let call_args = &bindings.call_args;

    let with_fn = if method.mutable {
        quote!(with_mut)
    } else {
        quote!(with)
    };
    let mapped = if method.ret.is_result() {
        quote!(::rspyts::__private::shim::map_result(__rspyts_ret))
    } else {
        quote!(::rspyts::__private::shim::map_plain(__rspyts_ret))
    };

    quote! {
        #args_struct

        /// # Safety
        /// Every pointer/length pair (`args_ptr`/`args_len` and each
        /// slice pair) must describe valid, initialized memory for the
        /// duration of the call, per ABI §3.1.
        #[unsafe(no_mangle)]
        #[doc(hidden)]
        #[allow(non_snake_case, clippy::too_many_arguments, clippy::unit_arg)]
        pub unsafe extern "C" fn #symbol(__rspyts_handle: u64, #(#c_params),*) -> *mut u8 {
            ::rspyts::__private::shim::run(|| {
                #prelude
                let __rspyts_ret = #slab.#with_fn(__rspyts_handle, |__rspyts_obj| {
                    __rspyts_obj.#method_ident(#(#call_args),*)
                })?;
                #mapped
            })
        }
    }
}

fn class_registration(
    ty_ident: &syn::Ident,
    impl_docs: &str,
    ctor: Option<&MethodInfo>,
    methods: &[MethodInfo],
    statics: &[MethodInfo],
) -> TokenStream {
    let name = ty_ident.to_string();

    let ctor_expr = match ctor {
        Some(ctor) => {
            let ctor_docs = &ctor.docs;
            let ctor_params: Vec<TokenStream> = ctor.params.iter().map(emit::param_decl).collect();
            let ctor_err = emit::err_name(&ctor.ret);
            quote! {
                ::std::option::Option::Some(::rspyts::__private::ir::CtorDecl {
                    docs: ::std::string::String::from(#ctor_docs),
                    params: ::std::vec![#(#ctor_params),*],
                    err: #ctor_err,
                })
            }
        }
        None => quote!(::std::option::Option::None),
    };

    let method_decls: Vec<TokenStream> = methods
        .iter()
        .map(|method| {
            let method_name = method.ident.to_string();
            let docs = &method.docs;
            let mutable = method.mutable;
            let params: Vec<TokenStream> = method.params.iter().map(emit::param_decl).collect();
            let ret = emit::ret_ty(&method.ret);
            let err = emit::err_name(&method.ret);
            let targets = emit::targets_expr(method.target);
            quote! {
                ::rspyts::__private::ir::MethodDecl {
                    name: ::std::string::String::from(#method_name),
                    docs: ::std::string::String::from(#docs),
                    mutable: #mutable,
                    params: ::std::vec![#(#params),*],
                    ret: #ret,
                    err: #err,
                    targets: #targets,
                }
            }
        })
        .collect();

    let static_decls: Vec<TokenStream> = statics
        .iter()
        .map(|st| {
            let static_name = st.ident.to_string();
            let docs = &st.docs;
            let params: Vec<TokenStream> = st.params.iter().map(emit::param_decl).collect();
            // For factories the declared return is the fresh handle, not a
            // data shape; the IR marks it `returns_self` and ignores `ret`.
            let ret = if st.returns_self {
                quote!(::rspyts::__private::ir::Ty::Unit)
            } else {
                emit::ret_ty(&st.ret)
            };
            let err = emit::err_name(&st.ret);
            let returns_self = st.returns_self;
            let targets = emit::targets_expr(st.target);
            quote! {
                ::rspyts::__private::ir::StaticDecl {
                    name: ::std::string::String::from(#static_name),
                    docs: ::std::string::String::from(#docs),
                    params: ::std::vec![#(#params),*],
                    ret: #ret,
                    err: #err,
                    returns_self: #returns_self,
                    targets: #targets,
                }
            }
        })
        .collect();

    emit::register_class(quote! {
        ::rspyts::__private::ir::ClassDecl {
            name: ::std::string::String::from(#name),
            docs: ::std::string::String::from(#impl_docs),
            constructor: #ctor_expr,
            methods: ::std::vec![#(#method_decls),*],
            statics: ::std::vec![#(#static_decls),*],
        }
    })
}
