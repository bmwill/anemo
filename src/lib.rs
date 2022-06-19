mod common;
mod config;
mod connection;
mod crypto;
mod endpoint;
mod error;
mod network;
mod request;
mod response;

pub use common::{ConnectionOrigin, PeerId};
pub use config::{EndpointConfig, EndpointConfigBuilder};
pub use connection::Connection;
pub use endpoint::{Connecting, Endpoint, Incoming};
pub use error::{Error, Result};
pub use network::Network;
pub use request::Request;
pub use response::Response;

#[cfg(test)]
pub fn init_tracing_for_testing() -> ::tracing::dispatcher::DefaultGuard {
    let subscriber = tracing_subscriber::FmtSubscriber::builder()
        .with_env_filter(
            tracing_subscriber::EnvFilter::builder()
                .with_default_directive(tracing::metadata::LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        .with_file(true)
        .with_line_number(true)
        .with_target(false)
        // .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
        .with_test_writer()
        .finish();
    ::tracing::subscriber::set_default(subscriber)
}
