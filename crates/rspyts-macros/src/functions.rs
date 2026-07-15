//! Expansion of `#[bridge]` on free functions (ABI §3.1).
//!
//! For `pub fn process_values(values: &[f64], batch_size: u32) -> …` the
//! macro emits, next to the untouched function:
//!
//! 1. `struct __RspytsArgs_process_values` — a `Deserialize` struct with
//!    one owned field per plain parameter, camelCase on the wire.
//! 2. `unsafe extern "C" fn rspyts_fn__process_values(args_ptr, args_len,
//!    s0_ptr, s0_len, …) -> *mut u8` — the panic-contained shim.
//! 3. An inventory registration building the `FnDecl` at manifest time.

use crate::attrs::BridgeArgs;
use crate::docs::extract_docs;
use crate::emit;
use crate::sig;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};

pub fn expand_fn(args: BridgeArgs, item: syn::ItemFn) -> syn::Result<TokenStream> {
    args.deny_error("functions; `error` marks an enum")?;
    args.deny_constructor()?;
    args.deny_static("free functions; `static` marks a method inside a #[bridge] impl block")?;
    args.deny_tag("functions")?;
    args.deny_rename_all("functions")?;
    args.deny_serde("functions; it adopts Serde derives on data types")?;
    sig::ensure_plain_signature(&item.sig, "functions")?;

    let params = sig::bridged_params(item.sig.inputs.iter())?;
    sig::validate_param_wire_names(&params)?;
    let ret = sig::classify_ret(&item.sig.output);

    let fn_ident = &item.sig.ident;
    let name_str = fn_ident.to_string();
    let docs = extract_docs(&item.attrs);
    let symbol = format_ident!("rspyts_fn__{}", fn_ident);
    let args_ident = format_ident!("__RspytsArgs_{}", fn_ident);

    let args_struct = emit::args_struct(&args_ident, &params);
    let bindings = emit::shim_bindings(&args_ident, &params);
    let c_params = &bindings.c_params;
    let prelude = &bindings.prelude;
    let call_args = &bindings.call_args;

    let call = quote!(#fn_ident(#(#call_args),*));
    let mapped = if ret.is_result() {
        quote!(::rspyts::__private::shim::map_result(#call))
    } else {
        quote!(::rspyts::__private::shim::map_plain(#call))
    };

    let param_decls: Vec<TokenStream> = params.iter().map(emit::param_decl).collect();
    let ret_ty = emit::ret_ty(&ret);
    let err = emit::err_name(&ret);
    let targets = emit::targets_expr(args.target.map(|(target, _)| target));
    let registration = emit::register_fn(quote! {
        ::rspyts::__private::ir::FnDecl {
            name: ::std::string::String::from(#name_str),
            docs: ::std::string::String::from(#docs),
            params: ::std::vec![#(#param_decls),*],
            ret: #ret_ty,
            err: #err,
            targets: #targets,
        }
    });

    Ok(quote! {
        #item

        #args_struct

        /// # Safety
        /// Every pointer/length pair (`args_ptr`/`args_len` and each
        /// slice pair) must describe valid, initialized memory for the
        /// duration of the call, per ABI §3.1.
        #[unsafe(no_mangle)]
        #[doc(hidden)]
        #[allow(non_snake_case, clippy::too_many_arguments, clippy::unit_arg)]
        pub unsafe extern "C" fn #symbol(#(#c_params),*) -> *mut u8 {
            ::rspyts::__private::shim::run_typed(|| {
                #prelude
                #mapped
            })
        }

        #registration
    })
}
