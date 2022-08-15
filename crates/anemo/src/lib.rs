mod config;
mod connection;
mod crypto;
mod endpoint;
mod error;
mod network;
mod routing;
pub mod rpc;
pub mod types;

pub use config::{Config, QuicConfig};
pub use error::{Error, Result};
pub use network::{KnownPeers, Network, Peer};
pub use routing::Router;
pub use types::{request::Request, response::Response, ConnectionOrigin, PeerId};

#[doc(hidden)]
pub mod codegen {
    pub use super::error::BoxError;
    pub use super::types::response::IntoResponse;
    pub use super::Request;
    pub use super::Response;
    pub use async_trait::async_trait;
    pub use bytes::Bytes;
    pub use futures::future::BoxFuture;
    pub use std::future::Future;
    pub use std::pin::Pin;
    pub use std::sync::Arc;
    pub use std::task::{Context, Poll};
    pub use tower::Service;
}

#[cfg(test)]
pub fn init_tracing_for_testing() -> ::tracing::dispatcher::DefaultGuard {
    use tracing_subscriber::{fmt::format::FmtSpan, EnvFilter, FmtSubscriber};

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,quinn=warn"));

    let subscriber = FmtSubscriber::builder()
        .with_env_filter(filter)
        .with_file(true)
        .with_line_number(true)
        .with_target(false)
        .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
        .with_test_writer()
        .finish();

    ::tracing::subscriber::set_default(subscriber)
}
