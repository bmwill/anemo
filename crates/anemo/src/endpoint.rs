use crate::{
    config::EndpointConfig, connection::Connection, types::Address, ConnectionOrigin, PeerId,
    Result,
};
use futures::StreamExt;
use quinn::{Datagrams, IncomingBiStreams, IncomingUniStreams};
use std::{
    future::Future,
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};
use tracing::trace;

#[derive(Debug)]
pub struct Endpoint {
    inner: quinn::Endpoint,
    local_addr: SocketAddr,
    config: EndpointConfig,
}

impl Endpoint {
    pub fn new(config: EndpointConfig, socket: std::net::UdpSocket) -> Result<(Self, Incoming)> {
        let local_addr = socket.local_addr()?;
        let server_config = config.server_config().clone();
        let (endpoint, incoming) =
            quinn::Endpoint::new(Default::default(), Some(server_config), socket)?;

        let endpoint = Self {
            inner: endpoint,
            local_addr,
            config,
        };
        let incoming = Incoming::new(incoming);
        Ok((endpoint, incoming))
    }

    #[cfg(test)]
    fn new_with_address<A: Into<Address>>(
        config: EndpointConfig,
        addr: A,
    ) -> Result<(Self, Incoming)> {
        let socket = std::net::UdpSocket::bind(addr.into())?;
        Self::new(config, socket)
    }

    pub fn connect(&self, address: Address) -> Result<Connecting> {
        self.connect_with_client_config(self.config.client_config().clone(), address)
    }

    pub fn connect_with_expected_peer_id(
        &self,
        address: Address,
        peer_id: PeerId,
    ) -> Result<Connecting> {
        let config = self
            .config
            .client_config_with_expected_server_identity(peer_id);
        self.connect_with_client_config(config, address)
    }

    fn connect_with_client_config(
        &self,
        config: quinn::ClientConfig,
        address: Address,
    ) -> Result<Connecting> {
        let addr = address.resolve()?;

        self.inner
            .connect_with(config, addr, self.config.server_name())
            .map_err(Into::into)
            .map(Connecting::new_outbound)
    }

    /// Returns the socket address that this Endpoint is bound to.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn peer_id(&self) -> PeerId {
        self.config().peer_id()
    }

    pub fn config(&self) -> &EndpointConfig {
        &self.config
    }

    /// Close all of this endpoint's connections immediately and cease accepting new connections.
    pub fn close(&self) {
        trace!("Closing endpoint");
        // let _ = self.termination_tx.send(());
        self.inner.close(0_u32.into(), b"endpoint closed")
    }
}

/// Stream of incoming connections.
#[derive(Debug)]
#[must_use = "futures/streams/sinks do nothing unless you `.await` or poll them"]
pub struct Incoming(quinn::Incoming);

impl Incoming {
    pub(crate) fn new(inner: quinn::Incoming) -> Self {
        Self(inner)
    }
}

impl futures::stream::Stream for Incoming {
    type Item = Connecting;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        self.0
            .poll_next_unpin(cx)
            .map(|maybe_next| maybe_next.map(Connecting::new_inbound))
    }
}

#[derive(Debug)]
#[must_use = "futures/streams/sinks do nothing unless you `.await` or poll them"]
pub struct Connecting {
    inner: quinn::Connecting,
    origin: ConnectionOrigin,
}

impl Connecting {
    pub(crate) fn new(inner: quinn::Connecting, origin: ConnectionOrigin) -> Self {
        Self { inner, origin }
    }

    pub(crate) fn new_inbound(inner: quinn::Connecting) -> Self {
        Self::new(inner, ConnectionOrigin::Inbound)
    }

    pub(crate) fn new_outbound(inner: quinn::Connecting) -> Self {
        Self::new(inner, ConnectionOrigin::Outbound)
    }
}

impl Future for Connecting {
    type Output = Result<NewConnection>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        Pin::new(&mut self.inner).poll(cx).map(|result| {
            result
                .map_err(anyhow::Error::from)
                .and_then(
                    |quinn::NewConnection {
                         connection,
                         uni_streams,
                         bi_streams,
                         datagrams,
                         ..
                     }| {
                        Ok(NewConnection {
                            connection: Connection::new(connection, self.origin)?,
                            uni_streams,
                            bi_streams,
                            datagrams,
                        })
                    },
                )
                .map_err(|e| anyhow::anyhow!("failed establishing {} connection: {e}", self.origin))
        })
    }
}

pub struct NewConnection {
    pub connection: Connection,
    pub uni_streams: IncomingUniStreams,
    pub bi_streams: IncomingBiStreams,
    pub datagrams: Datagrams,
}

#[cfg(test)]
mod test {
    use super::*;
    use futures::{future::join, io::AsyncReadExt, stream::StreamExt};
    use std::time::Duration;

    #[tokio::test]
    async fn basic_endpoint() -> Result<()> {
        let _gaurd = crate::init_tracing_for_testing();

        let msg = b"hello";
        let config_1 = EndpointConfig::random("test");
        let (endpoint_1, _incoming_1) = Endpoint::new_with_address(config_1, "localhost:0")?;
        let peer_id_1 = endpoint_1.config.peer_id();

        println!("1: {}", endpoint_1.local_addr());

        let config_2 = EndpointConfig::random("test");
        let (endpoint_2, mut incoming_2) = Endpoint::new_with_address(config_2, "localhost:0")?;
        let peer_id_2 = endpoint_2.config.peer_id();
        let addr_2 = endpoint_2.local_addr();
        println!("2: {}", endpoint_2.local_addr());

        let peer_1 = async move {
            let NewConnection { connection, .. } =
                endpoint_1.connect(addr_2.into()).unwrap().await.unwrap();
            assert_eq!(connection.peer_id(), peer_id_2);
            {
                let mut send_stream = connection.open_uni().await.unwrap();
                send_stream.write_all(msg).await.unwrap();
                send_stream.finish().await.unwrap();
            }
            endpoint_1.close();
            endpoint_1.inner.wait_idle().await;
            // Result::<()>::Ok(())
        };

        let peer_2 = async move {
            let NewConnection {
                connection,
                mut uni_streams,
                ..
            } = incoming_2.next().await.unwrap().await.unwrap();
            assert_eq!(connection.peer_id(), peer_id_1);

            let mut recv = uni_streams.next().await.unwrap().unwrap();
            let mut buf = Vec::new();
            AsyncReadExt::read_to_end(&mut recv, &mut buf)
                .await
                .unwrap();
            println!("from remote: {}", buf.escape_ascii());
            assert_eq!(buf, msg);
            endpoint_2.close();
            endpoint_2.inner.wait_idle().await;
            // Result::<()>::Ok(())
        };

        timeout(join(peer_1, peer_2)).await?;
        Ok(())
    }

    async fn timeout<F: std::future::Future>(
        f: F,
    ) -> Result<F::Output, tokio::time::error::Elapsed> {
        tokio::time::timeout(Duration::from_millis(500), f).await
    }
}
