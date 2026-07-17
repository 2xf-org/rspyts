//! Compile one Rust API into self-contained Python and TypeScript packages.

pub mod backend;
pub mod codec;
pub mod ir;
pub mod registry;
pub mod runtime;
mod types;
mod wire;

pub use rspyts_macros::{Error, Type, export, module};
pub use types::ContractType;

#[cfg(all(feature = "python", not(target_arch = "wasm32")))]
#[doc(hidden)]
#[macro_export]
macro_rules! __python {
    ($($tokens:tt)*) => { $($tokens)* };
}

#[cfg(not(all(feature = "python", not(target_arch = "wasm32"))))]
#[doc(hidden)]
#[macro_export]
macro_rules! __python {
    ($($tokens:tt)*) => {};
}

#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __typescript {
    ($($tokens:tt)*) => { $($tokens)* };
}

#[cfg(not(all(feature = "wasm", target_arch = "wasm32")))]
#[doc(hidden)]
#[macro_export]
macro_rules! __typescript {
    ($($tokens:tt)*) => {};
}

#[doc(hidden)]
pub mod __private {
    pub use crate::wire::{BoundaryError, BufferDtype, BufferValue, WireValue};
    pub use inventory;
    pub use serde;
    pub use serde_json;

    #[cfg(all(feature = "python", not(target_arch = "wasm32")))]
    pub use pyo3;
    #[cfg(all(feature = "wasm", target_arch = "wasm32"))]
    pub use wasm_bindgen;
}
