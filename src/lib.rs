#![allow(clippy::module_inception)]
#![allow(clippy::mutable_key_type)]
mod app_context;
pub mod async_graphql_hyper;
pub mod blueprint;
pub mod cache;
pub mod channel;
#[cfg(feature = "cli")]
pub mod cli;
pub mod config;
pub mod data_loader;
pub mod directive;
pub mod document;
pub mod endpoint;
pub mod graphql;
pub mod grpc;
pub mod has_headers;
pub mod helpers;
pub mod http;
pub mod json;
pub mod lambda;
pub mod mustache;
pub mod path;
pub mod print_schema;
pub mod try_fold;
pub mod valid;

use std::hash::Hash;
use std::num::NonZeroU64;
use std::ops::Deref;
use std::sync::Arc;

use async_graphql_value::ConstValue;
use http::Response;

pub trait EnvIO: Send + Sync + 'static {
    fn get(&self, key: &str) -> Option<String>;
}

#[async_trait::async_trait]
pub trait HttpIO: Sync + Send + 'static {
    async fn execute(
        &self,
        request: reqwest::Request,
    ) -> anyhow::Result<Response<hyper::body::Bytes>>;
}

impl<Http: HttpIO> HttpIO for Arc<Http> {
    fn execute<'life0, 'async_trait>(
        &'life0 self,
        request: reqwest::Request,
    ) -> core::pin::Pin<
        Box<
            dyn core::future::Future<Output = anyhow::Result<Response<hyper::body::Bytes>>>
                + core::marker::Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        self.deref().execute(request)
    }
}

#[async_trait::async_trait]
pub trait FileIO: Send + Sync {
    async fn write<'a>(&'a self, path: &'a str, content: &'a [u8]) -> anyhow::Result<()>;
    async fn read<'a>(&'a self, path: &'a str) -> anyhow::Result<String>;
}

#[async_trait::async_trait]
pub trait Cache: Send + Sync {
    type Key: Hash + Eq;
    type Value;
    async fn set<'a>(
        &'a self,
        key: Self::Key,
        value: Self::Value,
        ttl: NonZeroU64,
    ) -> anyhow::Result<Self::Value>;
    async fn get<'a>(&'a self, key: &'a Self::Key) -> anyhow::Result<Self::Value>;
}

pub type EntityCache = dyn Cache<Key = u64, Value = ConstValue>;

#[async_trait::async_trait]
pub trait ScriptIO<Event, Command>: Send + Sync {
    async fn on_event(&self, event: Event) -> anyhow::Result<Command>;
}

fn is_default<T: Default + Eq>(val: &T) -> bool {
    *val == T::default()
}
