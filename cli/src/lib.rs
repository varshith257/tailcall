#![allow(clippy::module_inception)]
#![allow(clippy::mutable_key_type)]
mod command;
pub(crate) mod env;
mod error;
pub(crate) mod file;
mod fmt;
pub(crate) mod http;
#[cfg(feature = "js")]
pub mod javascript;
pub mod server;
mod tc;

use std::hash::Hash;
use std::sync::Arc;

pub(crate) mod update_checker;
pub use env::EnvNative;
pub use error::CLIError;
pub use file::NativeFileIO;
pub use http::NativeHttp;
use tailcall::cache::InMemoryCache;
use tailcall::config::Upstream;
use tailcall::{blueprint, EnvIO, FileIO, HttpIO};
pub use tc::run;

// Provides access to env in native rust environment
pub fn init_env() -> Arc<dyn EnvIO> {
    Arc::new(EnvNative::init())
}

// Provides access to file system in native rust environment
pub fn init_file() -> Arc<dyn FileIO> {
    Arc::new(NativeFileIO::init())
}

pub fn init_hook_http(http: impl HttpIO, script: Option<blueprint::Script>) -> Arc<dyn HttpIO> {
    #[cfg(feature = "js")]
    if let Some(script) = script {
        return javascript::init_http(http, script);
    }

    #[cfg(not(feature = "js"))]
    log::warn!("JS capabilities are disabled in this build");
    let _ = script;

    Arc::new(http)
}

// Provides access to http in native rust environment
pub fn init_http(upstream: &Upstream, script: Option<blueprint::Script>) -> Arc<dyn HttpIO> {
    let http_io = NativeHttp::init(upstream);
    init_hook_http(http_io, script)
}

// Provides access to http in native rust environment
pub fn init_http2_only(upstream: &Upstream, script: Option<blueprint::Script>) -> Arc<dyn HttpIO> {
    let http_io = NativeHttp::init(&upstream.clone().http2_only(true));
    init_hook_http(http_io, script)
}

pub fn init_in_memory_cache<K: Hash + Eq, V: Clone>() -> InMemoryCache<K, V> {
    InMemoryCache::new()
}
