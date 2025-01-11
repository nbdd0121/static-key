#![warn(unsafe_op_in_unsafe_fn)]

#[doc(hidden)]
pub use static_key_macros::parse_static_match;

mod patch;
mod static_if;
mod static_match;

pub use static_match::{StaticKey, CallSite};