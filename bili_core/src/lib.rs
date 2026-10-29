//! Shell-independent Bilibili discovery, authentication, durable jobs and remux.
//!
//! A caller supplies one live Tokio runtime handle and awaits [`Core::shutdown`]
//! before dropping that runtime. No terminal, signal handler, GUI or player is
//! owned by this library. Rust dylib consumers must use the matched SDK tuple;
//! this API does not claim a stable Rust or C ABI.
mod api;
pub mod auth;
pub mod cookie;
mod domain;
mod download;
pub mod install;
pub mod remux;
mod service;
pub mod sidecar;
mod state;
pub mod update;

pub use auth::{AuthAction, AuthSnapshot};
pub use domain::*;
pub use service::Core;
