use crate::{
    endpoint::NewConnection,
    peer::Peer,
    wire::{read_request, write_response},
    Connection, Endpoint, Incoming, PeerId, Request, Response, Result,
};
use anyhow::anyhow;
use bytes::Bytes;
use futures::{
    stream::{Fuse, FuturesUnordered},
    StreamExt,
};
use quinn::{Datagrams, IncomingBiStreams, IncomingUniStreams, RecvStream, SendStream};
use std::{convert::Infallible, net::SocketAddr, sync::Arc};
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};
use tower::{util::BoxCloneService, ServiceExt};
use tracing::{info, trace, warn};

mod connection_manager;
pub use connection_manager::KnownPeers;
use connection_manager::{ActivePeers, ConnectionManager, ConnectionManagerRequest};

#[derive(Clone)]
pub struct Network(Arc<NetworkInner>);

//TODO
// There might be a chicken and egg problem with setting up components that need network access as
// well as want to provide a service. One thought would be to split the network building process in
// two.
// fn builder() -> (Builder, NetworkHandle)
//
// The Network handle could contain a oncecell that is initialized once the builder is finished and
// until such point, all access results in a Panic.
impl Network {
    /// Start a network and return a handle to it
    ///
    /// Requires that this is called from within the context of a tokio runtime
    pub fn start(
        endpoint: Endpoint,
        incoming: Incoming,
        service: BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>,
    ) -> Self {
        let endpoint = Arc::new(endpoint);
        let active_peers = ActivePeers::new(128);
        let known_peers = KnownPeers::new();

        let (connection_manager, connection_manager_handle) = ConnectionManager::new(
            endpoint.clone(),
            active_peers.clone(),
            known_peers.clone(),
            incoming,
            service,
        );

        let network = Self(Arc::new(NetworkInner {
            endpoint,
            active_peers,
            known_peers,
            connection_manager_handle,
        }));

        info!("Starting network");

        tokio::spawn(connection_manager.start());

        network
    }

    pub fn peers(&self) -> Vec<PeerId> {
        self.0.peers()
    }

    pub fn peer(&self, peer_id: PeerId) -> Option<Peer> {
        self.0.peer(peer_id)
    }

    pub fn known_peers(&self) -> &KnownPeers {
        self.0.known_peers()
    }

    pub async fn connect(&self, addr: SocketAddr) -> Result<PeerId> {
        self.0.connect(addr).await
    }

    pub fn disconnect(&self, peer: PeerId) -> Result<()> {
        self.0.disconnect(peer)
    }

    pub async fn rpc(&self, peer: PeerId, request: Request<Bytes>) -> Result<Response<Bytes>> {
        self.0.rpc(peer, request).await
    }

    /// Returns the socket address that this Network is listening on
    pub fn local_addr(&self) -> SocketAddr {
        self.0.local_addr()
    }

    pub fn peer_id(&self) -> PeerId {
        self.0.peer_id()
    }
}

struct NetworkInner {
    endpoint: Arc<Endpoint>,
    active_peers: ActivePeers,
    known_peers: KnownPeers,
    connection_manager_handle: tokio::sync::mpsc::Sender<ConnectionManagerRequest>,
}

impl NetworkInner {
    fn peers(&self) -> Vec<PeerId> {
        self.active_peers.peers()
    }

    fn known_peers(&self) -> &KnownPeers {
        &self.known_peers
    }

    /// Returns the socket address that this Network is listening on
    fn local_addr(&self) -> SocketAddr {
        self.endpoint.local_addr()
    }

    fn peer_id(&self) -> PeerId {
        self.endpoint.peer_id()
    }

    async fn connect(&self, addr: SocketAddr) -> Result<PeerId> {
        let (sender, reciever) = tokio::sync::oneshot::channel();
        self.connection_manager_handle
            .send(ConnectionManagerRequest::ConnectRequest(addr, sender))
            .await
            .expect("ConnectionManager should still be up");
        reciever.await?
    }

    fn disconnect(&self, peer_id: PeerId) -> Result<()> {
        self.active_peers
            .remove(&peer_id, crate::types::DisconnectReason::Requested);
        Ok(())
    }

    pub fn peer(&self, peer_id: PeerId) -> Option<Peer> {
        let connection = self.active_peers.get(&peer_id)?;
        Some(Peer::new(connection))
    }

    async fn rpc(&self, peer_id: PeerId, request: Request<Bytes>) -> Result<Response<Bytes>> {
        self.peer(peer_id)
            .ok_or_else(|| anyhow!("not connected to peer {peer_id}"))?
            .rpc(request)
            .await
    }

    // async fn send_message(&self, peer_id: PeerId, message: Request<Bytes>) -> Result<()> {
    //     self.peer(peer_id)
    //         .ok_or_else(|| anyhow!("not connected to peer {peer_id}"))?
    //         .message(message)
    //         .await
    // }
}

impl Drop for NetworkInner {
    fn drop(&mut self) {
        self.endpoint.close()
    }
}

struct InboundRequestHandler {
    connection: Connection,
    incoming_bi: Fuse<IncomingBiStreams>,
    incoming_uni: Fuse<IncomingUniStreams>,
    incoming_datagrams: Fuse<Datagrams>,

    service: BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>,
    active_peers: ActivePeers,
}

impl InboundRequestHandler {
    fn new(
        NewConnection {
            connection,
            uni_streams,
            bi_streams,
            datagrams,
        }: NewConnection,
        service: BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>,
        active_peers: ActivePeers,
    ) -> Self {
        Self {
            connection,
            incoming_uni: uni_streams.fuse(),
            incoming_bi: bi_streams.fuse(),
            incoming_datagrams: datagrams.fuse(),
            service,
            active_peers,
        }
    }

    async fn start(mut self) {
        info!(peer =% self.connection.peer_id(), "InboundRequestHandler started");

        let mut inflight_requests = FuturesUnordered::new();

        loop {
            futures::select! {
                // anemo does not currently use uni streams so we can
                // just ignore and drop the stream
                uni = self.incoming_uni.select_next_some() => {
                    match uni {
                        Ok(recv_stream) => trace!("incoming uni stream! {}", recv_stream.id()),
                        Err(e) => {
                            warn!("error listening for incoming uni streams: {e}");
                            break;
                        }
                    }
                },
                bi = self.incoming_bi.select_next_some() => {
                    match bi {
                        Ok((bi_tx, bi_rx)) => {
                            info!("incoming bi stream! {}", bi_tx.id());
                            let request_handler =
                                BiStreamRequestHandler::new(self.connection.peer_id(), self.service.clone(), bi_tx, bi_rx);
                            inflight_requests.push(request_handler.handle());
                        }
                        Err(e) => {
                            warn!("error listening for incoming bi streams: {e}");
                            break;
                        }
                    }
                },
                // anemo does not currently use datagrams so we can
                // just ignore them
                datagram = self.incoming_datagrams.select_next_some() => {
                    match datagram {
                        Ok(datagram) => trace!("incoming datagram of length: {}", datagram.len()),
                        Err(e) => {
                            warn!("error listening for datagrams: {e}");
                            break;
                        }
                    }
                },
                () = inflight_requests.select_next_some() => {},
                complete => break,
            }
        }

        self.active_peers.remove_with_stable_id(
            self.connection.peer_id(),
            self.connection.stable_id(),
            crate::types::DisconnectReason::ConnectionLost,
        );

        info!("InboundRequestHandler ended");
    }
}

/// Returns a fully configured length-delimited codec for writing/reading
/// serialized frames to/from a socket.
pub(crate) fn network_message_frame_codec() -> LengthDelimitedCodec {
    const MAX_FRAME_SIZE: usize = 1 << 23; // 8 MiB

    LengthDelimitedCodec::builder()
        .max_frame_length(MAX_FRAME_SIZE)
        .length_field_length(4)
        .big_endian()
        .new_codec()
}

struct BiStreamRequestHandler {
    peer_id: PeerId,
    service: BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>,
    send_stream: FramedWrite<SendStream, LengthDelimitedCodec>,
    recv_stream: FramedRead<RecvStream, LengthDelimitedCodec>,
}

impl BiStreamRequestHandler {
    fn new(
        peer_id: PeerId,
        service: BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>,
        send_stream: SendStream,
        recv_stream: RecvStream,
    ) -> Self {
        Self {
            peer_id,
            service,
            send_stream: FramedWrite::new(send_stream, network_message_frame_codec()),
            recv_stream: FramedRead::new(recv_stream, network_message_frame_codec()),
        }
    }

    async fn handle(self) {
        if let Err(e) = self.do_handle().await {
            warn!("handling request failed: {e}");
        }
    }

    async fn do_handle(mut self) -> Result<()> {
        //
        // Read Request
        //

        let mut request = read_request(&mut self.recv_stream).await?;

        // Set the PeerId of this peer
        request.extensions_mut().insert(self.peer_id);

        // Issue request to configured Service
        let response = self.service.oneshot(request).await.expect("Infallible");

        //
        // Write Response
        //

        write_response(&mut self.send_stream, response).await?;
        self.send_stream.get_mut().finish().await?;

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{config::EndpointConfig, Result};
    use std::{
        net::{Ipv4Addr, SocketAddrV4},
        time::Duration,
    };
    use tracing::trace;

    #[tokio::test]
    async fn basic_network() -> Result<()> {
        let _gaurd = crate::init_tracing_for_testing();

        let msg = b"The Way of Kings";

        let network_1 = build_network()?;
        let network_2 = build_network()?;

        let peer = network_1.connect(network_2.local_addr()).await?;
        let response = network_1
            .rpc(peer, Request::new(msg.as_ref().into()))
            .await?;
        assert_eq!(response.into_body(), msg.as_ref());

        let msg = b"Words of Radiance";
        let peer_id_1 = network_1.peer_id();
        let response = network_2
            .rpc(peer_id_1, Request::new(msg.as_ref().into()))
            .await?;
        assert_eq!(response.into_body(), msg.as_ref());
        Ok(())
    }

    fn build_network() -> Result<Network> {
        let config = EndpointConfig::random("test");
        let (endpoint, incoming) = Endpoint::new(config, "localhost:0")?;
        trace!(
            address =% endpoint.local_addr(),
            peer_id =% endpoint.peer_id(),
            "starting network"
        );

        let network = Network::start(endpoint, incoming, echo_service());
        Ok(network)
    }

    fn echo_service() -> BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible> {
        let handle = move |request: Request<Bytes>| async move {
            trace!("recieved: {}", request.body().escape_ascii());
            let response = Response::new(request.into_body());
            Result::<Response<Bytes>, Infallible>::Ok(response)
        };

        tower::service_fn(handle).boxed_clone()
    }

    #[tokio::test]
    async fn ip6_calling_ip4() -> Result<()> {
        let _gaurd = crate::init_tracing_for_testing();

        let config = EndpointConfig::random("test");
        let (endpoint, incoming) = Endpoint::new(config, "[::]:0")?;
        info!(
            address =% endpoint.local_addr(),
            peer_id =% endpoint.peer_id(),
            "starting network"
        );

        let network_1 = Network::start(endpoint, incoming, echo_service());

        let config = EndpointConfig::random("test");
        let (endpoint, incoming) = Endpoint::new(config, "127.0.0.1:0")?;
        info!(
            address =% endpoint.local_addr(),
            peer_id =% endpoint.peer_id(),
            "starting network"
        );

        let network_2 = Network::start(endpoint, incoming, echo_service());

        let msg = b"The Way of Kings";
        let peer = network_1.connect(network_2.local_addr()).await?;
        let response = network_1
            .rpc(peer, Request::new(msg.as_ref().into()))
            .await?;

        println!("{}", response.body().escape_ascii());

        Ok(())
    }

    #[tokio::test]
    async fn localhost_calling_anyaddr() -> Result<()> {
        let _gaurd = crate::init_tracing_for_testing();

        let config = EndpointConfig::random("test");
        let (endpoint, incoming) = Endpoint::new(config, "0.0.0.0:0")?;
        info!(
            address =% endpoint.local_addr(),
            peer_id =% endpoint.peer_id(),
            "starting network"
        );

        let network_1 = Network::start(endpoint, incoming, echo_service());

        let config = EndpointConfig::random("test");
        let (endpoint, incoming) = Endpoint::new(config, "127.0.0.1:0")?;
        info!(
            address =% endpoint.local_addr(),
            peer_id =% endpoint.peer_id(),
            "starting network"
        );

        let network_2 = Network::start(endpoint, incoming, echo_service());

        let msg = b"The Way of Kings";
        let peer = network_2
            .connect(SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::LOCALHOST,
                network_1.local_addr().port(),
            )))
            .await?;

        let response = network_2
            .rpc(peer, Request::new(msg.as_ref().into()))
            .await?;

        println!("{}", response.body().escape_ascii());

        let response = network_1
            .rpc(network_2.peer_id(), Request::new(msg.as_ref().into()))
            .await?;

        println!("{}", response.body().escape_ascii());

        Ok(())
    }

    #[tokio::test]
    async fn dropped_connection() -> Result<()> {
        let _gaurd = crate::init_tracing_for_testing();

        let config = EndpointConfig::builder()
            .random_keypair()
            .server_name("test")
            .idle_timeout(Duration::from_secs(1))
            .build()?;
        let (endpoint, incoming) = Endpoint::new(config, "localhost:0")?;
        info!(
            address =% endpoint.local_addr(),
            peer_id =% endpoint.peer_id(),
            "starting network"
        );

        let network_1 = Network::start(endpoint, incoming, echo_service());

        let config = EndpointConfig::random("test");
        let (endpoint, incoming) = Endpoint::new(config, "localhost:0")?;
        info!(
            address =% endpoint.local_addr(),
            peer_id =% endpoint.peer_id(),
            "starting network"
        );

        let network_2 = Network::start(endpoint, incoming, echo_service());

        let msg = b"The Way of Kings";
        let peer = network_1.connect(network_2.local_addr()).await?;
        let response = network_1
            .rpc(peer, Request::new(msg.as_ref().into()))
            .await?;

        println!("{}", response.body().escape_ascii());

        let peer = network_1.peer(peer).unwrap();

        network_2.0.endpoint.close();

        peer.rpc(Request::new(msg.as_ref().into()))
            .await
            .unwrap_err();

        Ok(())
    }

    #[tokio::test]
    async fn basic_connectivity_check() -> Result<()> {
        use crate::types::DisconnectReason;
        use crate::types::PeerEvent::*;

        let _gaurd = crate::init_tracing_for_testing();

        let network_1 = build_network()?;
        let network_2 = build_network()?;

        let peer_id_1 = network_1.peer_id();
        let peer_id_2 = network_2.peer_id();

        let peer_info_2 = crate::types::PeerInfo {
            peer_id: peer_id_2,
            affinity: crate::types::PeerAffinity::High,
            address: vec![network_2.local_addr()],
        };
        let mut subscriber_1 = network_1.0.active_peers.subscribe().0;
        let mut subscriber_2 = network_2.0.active_peers.subscribe().0;

        network_1.known_peers().insert(peer_info_2);

        assert_eq!(NewPeer(peer_id_2), subscriber_1.recv().await?);
        assert_eq!(NewPeer(peer_id_1), subscriber_2.recv().await?);

        network_1.known_peers().remove(&peer_id_2).unwrap();
        network_1.disconnect(peer_id_2)?;

        assert_eq!(
            LostPeer(peer_id_2, DisconnectReason::Requested),
            subscriber_1.recv().await?
        );
        assert_eq!(
            LostPeer(peer_id_1, DisconnectReason::ConnectionLost),
            subscriber_2.recv().await?
        );

        Ok(())
    }
}
