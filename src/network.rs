use crate::{
    peer::Peer,
    wire::{read_request, read_response, write_request, write_response},
    Connection, ConnectionOrigin, Endpoint, Incoming, PeerId, Request, Response, Result,
};
use anyhow::anyhow;
use bytes::Bytes;
use futures::{
    stream::{Fuse, FuturesUnordered},
    StreamExt,
};
use parking_lot::{Mutex, RwLock};
use quinn::{IncomingBiStreams, IncomingUniStreams, RecvStream, SendStream};
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
        rpc_service: BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>,
    ) -> Self {
        let network = Self(Arc::new(NetworkInner {
            endpoint,
            connections: Default::default(),
            on_bi: Mutex::new(rpc_service),
        }));

        info!("Starting network");

        let inbound_connection_handler = InboundConnectionHandler::new(network.clone(), incoming);

        tokio::spawn(inbound_connection_handler.start());

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

    pub async fn rpc_with_addr(
        &self,
        addr: SocketAddr,
        request: Request<Bytes>,
    ) -> Result<Response<Bytes>> {
        self.0.rpc_with_addr(addr, request).await
    }

    pub async fn send_message(&self, peer: PeerId, request: Request<Bytes>) -> Result<()> {
        self.0.send_message(peer, request).await
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
    endpoint: Endpoint,
    connections: Arc<RwLock<HashMap<PeerId, Connection>>>,
    //TODO these shouldn't need to be wrapped in mutexes
    on_bi: Mutex<BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>>,
}

impl NetworkInner {
    fn peers(&self) -> Vec<PeerId> {
        self.connections.read().keys().copied().collect()
    }

    /// Returns the socket address that this Network is listening on
    fn local_addr(&self) -> SocketAddr {
        self.endpoint.local_addr()
    }

    fn peer_id(&self) -> PeerId {
        self.endpoint.peer_id()
    }

    async fn connect(&self, addr: SocketAddr) -> Result<PeerId> {
        let (connection, incoming_uni, incoming_bi) = self.endpoint.connect(addr)?.await?;
        let peer_id = PeerId(connection.peer_identity());
        self.add_peer(connection, incoming_uni, incoming_bi);
        Ok(peer_id)
    }

    fn disconnect(&self, peer: PeerId) -> Result<()> {
        if let Some(connection) = self.connections.write().remove(&peer) {
            connection.close();
        }
        Ok(())
    }

    pub fn peer(&self, peer_id: PeerId) -> Option<Peer> {
        let connection = self.connections.read().get(&peer_id)?.clone();
        Some(Peer::new(connection))
    }

    async fn rpc(&self, peer: PeerId, request: Request<Bytes>) -> Result<Response<Bytes>> {
        let connection = self
            .connections
            .read()
            .get(&peer)
            .ok_or_else(|| anyhow!("not connected to peer {peer}"))?
            .clone();
        Self::do_rpc(connection, request).await
    }

    async fn rpc_with_addr(
        &self,
        addr: SocketAddr,
        request: Request<Bytes>,
    ) -> Result<Response<Bytes>> {
        let maybe_connection = self
            .connections
            .read()
            .values()
            .find(|connection| connection.remote_address() == addr)
            .cloned();
        let connection = if let Some(connection) = maybe_connection {
            connection
        } else {
            let peer_id = self.connect(addr).await?;
            self.connections.read().get(&peer_id).unwrap().clone()
        };

        Self::do_rpc(connection, request).await
    }

    async fn do_rpc(connection: Connection, request: Request<Bytes>) -> Result<Response<Bytes>> {
        let (send_stream, recv_stream) = connection.open_bi().await?;
        let mut send_stream = FramedWrite::new(send_stream, network_message_frame_codec());
        let mut recv_stream = FramedRead::new(recv_stream, network_message_frame_codec());

        //
        // Write Request
        //

        write_request(&mut send_stream, request).await?;
        send_stream.get_mut().finish().await?;

        //
        // Read Response
        //

        let response = read_response(&mut recv_stream).await?;

        Ok(response)
    }

    async fn send_message(&self, peer: PeerId, request: Request<Bytes>) -> Result<()> {
        let connection = self
            .connections
            .read()
            .get(&peer)
            .ok_or_else(|| anyhow!("not connected to peer {peer}"))?
            .clone();

        let send_stream = connection.open_uni().await?;
        let mut send_stream = FramedWrite::new(send_stream, network_message_frame_codec());

        //
        // Write Request
        //

        write_request(&mut send_stream, request).await?;
        send_stream.get_mut().finish().await?;

        Ok(())
    }

    fn add_peer(
        &self,
        connection: Connection,
        incoming_uni: IncomingUniStreams,
        incoming_bi: IncomingBiStreams,
    ) {
        // TODO drop Connection if you've somehow connected out ourself

        let peer_id = PeerId(connection.peer_identity());
        match self.connections.write().entry(peer_id) {
            Entry::Occupied(mut entry) => {
                if Self::simultaneous_dial_tie_breaking(
                    self.peer_id(),
                    peer_id,
                    entry.get().origin(),
                    connection.origin(),
                ) {
                    info!("closing old connection with {peer_id:?} to mitigate simultaneous dial");
                    let old_connection = entry.insert(connection);
                    old_connection.close();
                } else {
                    info!("closing new connection with {peer_id:?} to mitigate simultaneous dial");
                    connection.close();
                    // Early return to avoid standing up Incoming Request handlers
                    return;
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(connection);
            }
        }
        let request_handler = InboundRequestHandler::new(
            self.connections.clone(),
            peer_id,
            self.on_bi.lock().clone(),
            incoming_uni,
            incoming_bi,
        );

        tokio::spawn(request_handler.start());
    }

    /// In the event two peers simultaneously dial each other we need to be able to do
    /// tie-breaking to determine which connection to keep and which to drop in a deterministic
    /// way. One simple way is to compare our local PeerId with that of the remote's PeerId and
    /// keep the connection where the peer with the greater PeerId is the dialer.
    ///
    /// Returns `true` if the existing connection should be dropped and `false` if the new
    /// connection should be dropped.
    fn simultaneous_dial_tie_breaking(
        own_peer_id: PeerId,
        remote_peer_id: PeerId,
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

impl Drop for NetworkInner {
    fn drop(&mut self) {
        self.endpoint.close()
    }
}

struct InboundConnectionHandler {
    // TODO we probably don't want this to be Network but some other internal type that doesn't keep
    // the network alive after the application layer drops the handle
    network: Network,
    incoming: Fuse<Incoming>,
}

impl InboundConnectionHandler {
    fn new(network: Network, incoming: Incoming) -> Self {
        Self {
            network,
            incoming: incoming.fuse(),
        }
    }

    async fn start(mut self) {
        info!("InboundConnectionHandler started");

        let mut pending_connections = FuturesUnordered::new();

        loop {
            futures::select! {
                connecting = self.incoming.select_next_some() => {
                    info!("recieved new incoming connection");
                    pending_connections.push(connecting);
                },
                maybe_connection = pending_connections.select_next_some() => {
                    match maybe_connection {
                        Ok((connection, incoming_uni, incoming_bi)) => {
                            info!("new connection complete");
                            self.network.0.add_peer(connection, incoming_uni, incoming_bi);
                        }
                        Err(e) => {
                            error!("inbound connection failed: {e}");
                        }
                    }
                },
                complete => break,
            }
        }

        info!("InboundConnectionHandler ended");
    }
}

struct InboundRequestHandler {
    connections: Arc<RwLock<HashMap<PeerId, Connection>>>,
    peer_id: PeerId,
    service: BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>,
    incoming_bi: Fuse<IncomingBiStreams>,
    incoming_uni: Fuse<IncomingUniStreams>,
}

impl InboundRequestHandler {
    fn new(
        connections: Arc<RwLock<HashMap<PeerId, Connection>>>,
        peer_id: PeerId,
        service: BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>,
        incoming_uni: IncomingUniStreams,
        incoming_bi: IncomingBiStreams,
    ) -> Self {
        Self {
            connections,
            peer_id,
            service,
            incoming_uni: incoming_uni.fuse(),
            incoming_bi: incoming_bi.fuse(),
        }
    }

    async fn start(mut self) {
        info!(peer =% self.peer_id, "InboundRequestHandler started");

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
                                BiStreamRequestHandler::new(self.peer_id, self.service.clone(), bi_tx, bi_rx);
                            inflight_requests.push(request_handler.handle());
                        }
                        Err(e) => {
                            warn!("error listening for incoming bi streams: {e}");
                            break;
                        }
                    }
                },
                () = inflight_requests.select_next_some() => {},
                complete => break,
            }
        }

        if let Some(connection) = self.connections.write().remove(&self.peer_id) {
            connection.close();
        }

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
}
