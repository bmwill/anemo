use crate::{
    endpoint::NewConnection,
    peer::Peer,
    wire::{read_request, write_response},
    Connecting, Connection, ConnectionOrigin, Endpoint, Incoming, PeerId, Request, Response,
    Result,
};
use anyhow::anyhow;
use bytes::Bytes;
use futures::FutureExt;
use futures::{
    stream::{Fuse, FuturesUnordered},
    StreamExt,
};
use parking_lot::RwLock;
use quinn::{Datagrams, IncomingBiStreams, IncomingUniStreams, RecvStream, SendStream};
use std::{
    collections::{hash_map::Entry, HashMap},
    convert::Infallible,
    net::SocketAddr,
    sync::Arc,
};
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};
use tower::{util::BoxCloneService, ServiceExt};
use tracing::{error, info, trace, warn};

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
        let active_peers = Arc::new(RwLock::new(ActivePeers::new(128)));

        let (connection_manager, connection_manager_handle) =
            ConnectionManager::new(endpoint.clone(), active_peers.clone(), incoming, service);

        let network = Self(Arc::new(NetworkInner {
            endpoint,
            active_peers,
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

struct ActivePeers {
    connections: HashMap<PeerId, Connection>,
    peer_event_sender: tokio::sync::broadcast::Sender<crate::types::PeerEvent>,
}

impl ActivePeers {
    pub fn new(channel_size: usize) -> Self {
        let (sender, _reciever) = tokio::sync::broadcast::channel(channel_size);
        Self {
            connections: Default::default(),
            peer_event_sender: sender,
        }
    }

    #[allow(unused)]
    pub fn subscribe(
        &self,
    ) -> (
        tokio::sync::broadcast::Receiver<crate::types::PeerEvent>,
        Vec<PeerId>,
    ) {
        let peers = self.peers();
        let reciever = self.peer_event_sender.subscribe();
        (reciever, peers)
    }

    pub fn peers(&self) -> Vec<PeerId> {
        self.connections.keys().copied().collect()
    }

    pub fn get(&self, peer_id: &PeerId) -> Option<Connection> {
        self.connections.get(peer_id).cloned()
    }

    pub fn remove(&mut self, peer_id: &PeerId, reason: crate::types::DisconnectReason) {
        if let Some(connection) = self.connections.remove(peer_id) {
            // maybe actually provide reason to other side?
            connection.close();

            self.send_event(crate::types::PeerEvent::LostPeer(*peer_id, reason));
        }
    }

    fn remove_with_stable_id(
        &mut self,
        peer_id: PeerId,
        stable_id: usize,
        reason: crate::types::DisconnectReason,
    ) {
        match self.connections.entry(peer_id) {
            Entry::Occupied(entry) => {
                // Only remove the entry if the stable id matches
                if entry.get().stable_id() == stable_id {
                    let (peer_id, connection) = entry.remove_entry();
                    // maybe actually provide reason to other side?
                    connection.close();

                    self.send_event(crate::types::PeerEvent::LostPeer(peer_id, reason));
                }
            }
            Entry::Vacant(_) => {}
        }
    }

    fn send_event(&self, event: crate::types::PeerEvent) {
        // We don't care if anyone is listening
        let _ = self.peer_event_sender.send(event);
    }

    #[must_use]
    pub fn add(
        &mut self,
        own_peer_id: &PeerId,
        new_connection: NewConnection,
    ) -> Option<NewConnection> {
        // TODO drop Connection if you've somehow connected out ourself

        let peer_id = new_connection.connection.peer_id();
        match self.connections.entry(peer_id) {
            Entry::Occupied(mut entry) => {
                if Self::simultaneous_dial_tie_breaking(
                    own_peer_id,
                    &peer_id,
                    entry.get().origin(),
                    new_connection.connection.origin(),
                ) {
                    info!("closing old connection with {peer_id:?} to mitigate simultaneous dial");
                    let old_connection = entry.insert(new_connection.connection.clone());
                    old_connection.close();
                    self.send_event(crate::types::PeerEvent::LostPeer(
                        peer_id,
                        crate::types::DisconnectReason::Requested,
                    ));
                } else {
                    info!("closing new connection with {peer_id:?} to mitigate simultaneous dial");
                    new_connection.connection.close();
                    // Early return to avoid standing up Incoming Request handlers
                    return None;
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(new_connection.connection.clone());
            }
        }

        self.send_event(crate::types::PeerEvent::NewPeer(peer_id));

        Some(new_connection)
    }

    /// In the event two peers simultaneously dial each other we need to be able to do
    /// tie-breaking to determine which connection to keep and which to drop in a deterministic
    /// way. One simple way is to compare our local PeerId with that of the remote's PeerId and
    /// keep the connection where the peer with the greater PeerId is the dialer.
    ///
    /// Returns `true` if the existing connection should be dropped and `false` if the new
    /// connection should be dropped.
    fn simultaneous_dial_tie_breaking(
        own_peer_id: &PeerId,
        remote_peer_id: &PeerId,
        existing_origin: ConnectionOrigin,
        new_origin: ConnectionOrigin,
    ) -> bool {
        match (existing_origin, new_origin) {
            // If the remote dials while an existing connection is open, the older connection is
            // dropped.
            (ConnectionOrigin::Inbound, ConnectionOrigin::Inbound) => true,
            // We should never dial the same peer twice, but if we do drop the old connection
            (ConnectionOrigin::Outbound, ConnectionOrigin::Outbound) => true,
            (ConnectionOrigin::Inbound, ConnectionOrigin::Outbound) => remote_peer_id < own_peer_id,
            (ConnectionOrigin::Outbound, ConnectionOrigin::Inbound) => own_peer_id < remote_peer_id,
        }
    }
}

struct NetworkInner {
    endpoint: Arc<Endpoint>,
    active_peers: Arc<RwLock<ActivePeers>>,
    connection_manager_handle: tokio::sync::mpsc::Sender<ConnectionManagerRequest>,
}

impl NetworkInner {
    fn peers(&self) -> Vec<PeerId> {
        self.active_peers.read().peers()
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
            .write()
            .remove(&peer_id, crate::types::DisconnectReason::Requested);
        Ok(())
    }

    pub fn peer(&self, peer_id: PeerId) -> Option<Peer> {
        let connection = self.active_peers.read().get(&peer_id)?;
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

#[derive(Debug)]
enum ConnectionManagerRequest {
    ConnectRequest(SocketAddr, tokio::sync::oneshot::Sender<Result<PeerId>>),
}

struct ConnectingOutput {
    connecting_result: Result<NewConnection>,
    maybe_oneshot: Option<tokio::sync::oneshot::Sender<Result<PeerId>>>,
}

struct ConnectionManager {
    endpoint: Arc<Endpoint>,

    mailbox: Fuse<tokio_stream::wrappers::ReceiverStream<ConnectionManagerRequest>>,
    pending_connections: FuturesUnordered<JoinHandle<ConnectingOutput>>,

    active_peers: Arc<RwLock<ActivePeers>>,
    incoming: Fuse<Incoming>,

    service: BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>,
}

impl ConnectionManager {
    fn new(
        endpoint: Arc<Endpoint>,
        active_peers: Arc<RwLock<ActivePeers>>,
        incoming: Incoming,
        service: BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>,
    ) -> (Self, tokio::sync::mpsc::Sender<ConnectionManagerRequest>) {
        let (sender, reciever) = tokio::sync::mpsc::channel(128);
        (
            Self {
                endpoint,
                mailbox: tokio_stream::wrappers::ReceiverStream::new(reciever).fuse(),
                pending_connections: FuturesUnordered::new(),
                active_peers,
                incoming: incoming.fuse(),
                service,
            },
            sender,
        )
    }

    async fn start(mut self) {
        info!("ConnectionManager started");

        loop {
            futures::select! {
                request = self.mailbox.select_next_some() => {
                    info!("recieved new request");
                    match request {
                        ConnectionManagerRequest::ConnectRequest(address, oneshot) => {
                            self.handle_connect_request(address, oneshot);
                        }
                    }
                }
                connecting = self.incoming.select_next_some() => {
                    self.handle_incoming(connecting);
                },
                connecting_output = self.pending_connections.select_next_some() => {
                    self.handle_connecting_result(connecting_output);
                },
                complete => break,
            }
        }

        info!("ConnectionManager ended");
    }

    fn add_peer(&mut self, new_connection: NewConnection) {
        if let Some(new_connection) = self
            .active_peers
            .write()
            .add(&self.endpoint.peer_id(), new_connection)
        {
            let request_handler = InboundRequestHandler::new(
                new_connection,
                self.service.clone(),
                self.active_peers.clone(),
            );

            tokio::spawn(request_handler.start());
        }
    }

    fn handle_connect_request(
        &mut self,
        address: SocketAddr,
        oneshot: tokio::sync::oneshot::Sender<Result<PeerId>>,
    ) {
        let connecting = self.endpoint.connect(address);
        let join_handle = JoinHandle(tokio::spawn(async move {
            let connecting_result = match connecting {
                Ok(connecting) => connecting.await,
                Err(e) => Err(e),
            };
            ConnectingOutput {
                connecting_result,
                maybe_oneshot: Some(oneshot),
            }
        }));
        self.pending_connections.push(join_handle);
    }

    fn handle_incoming(&mut self, connecting: Connecting) {
        info!("recieved new incoming connection");
        let join_handle = JoinHandle(tokio::spawn(connecting.map(|connecting_result| {
            ConnectingOutput {
                connecting_result,
                maybe_oneshot: None,
            }
        })));
        self.pending_connections.push(join_handle);
    }

    fn handle_connecting_result(
        &mut self,
        ConnectingOutput {
            connecting_result,
            maybe_oneshot,
        }: ConnectingOutput,
    ) {
        match connecting_result {
            Ok(new_connection) => {
                info!("new connection complete");
                let peer_id = new_connection.connection.peer_id();
                self.add_peer(new_connection);
                if let Some(oneshot) = maybe_oneshot {
                    let _ = oneshot.send(Ok(peer_id));
                }
            }
            Err(e) => {
                error!("inbound connection failed: {e}");
                if let Some(oneshot) = maybe_oneshot {
                    let _ = oneshot.send(Err(e));
                }
            }
        }
    }
}

// JoinHandle that aborts on drop
#[derive(Debug)]
#[must_use]
pub struct JoinHandle<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for JoinHandle<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl<T> std::future::Future for JoinHandle<T> {
    type Output = T;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        // If the task panics just propagate it up
        std::pin::Pin::new(&mut self.0).poll(cx).map(Result::unwrap)
    }
}

struct InboundRequestHandler {
    connection: Connection,
    incoming_bi: Fuse<IncomingBiStreams>,
    incoming_uni: Fuse<IncomingUniStreams>,
    incoming_datagrams: Fuse<Datagrams>,

    service: BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>,
    active_peers: Arc<RwLock<ActivePeers>>,
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
        active_peers: Arc<RwLock<ActivePeers>>,
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

        self.active_peers.write().remove_with_stable_id(
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
}
