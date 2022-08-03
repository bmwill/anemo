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
