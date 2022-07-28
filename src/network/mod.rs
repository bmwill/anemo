use crate::{
    config::EndpointConfig,
    endpoint::{Endpoint, Incoming},
    Config, PeerId, Request, Response, Result,
};
use anyhow::anyhow;
use bytes::Bytes;
use std::{convert::Infallible, net::SocketAddr, sync::Arc};
use tower::util::BoxCloneService;
use tracing::info;

mod connection_manager;
pub use connection_manager::KnownPeers;
use connection_manager::{ActivePeers, ConnectionManager, ConnectionManagerRequest};

mod peer;
pub use peer::Peer;

mod request_handler;
mod wire;

#[cfg(test)]
mod tests;

pub struct Builder {
    socket: std::net::UdpSocket,
    config: Option<Config>,
    server_name: Option<String>,
    keypair: Option<ed25519_dalek::Keypair>,
}

impl Builder {
    pub fn config(mut self, config: Config) -> Self {
        self.config = Some(config);
        self
    }

    pub fn server_name<T: Into<String>>(mut self, server_name: T) -> Self {
        self.server_name = Some(server_name.into());
        self
    }

    pub fn keypair(mut self, keypair: ed25519_dalek::Keypair) -> Self {
        self.keypair = Some(keypair);
        self
    }

    #[cfg(test)]
    pub(crate) fn random_keypair(self) -> Self {
        let mut rng = rand::thread_rng();
        let keypair = ed25519_dalek::Keypair::generate(&mut rng);

        self.keypair(keypair)
    }

    pub fn start(
        self,
        service: BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>,
    ) -> Result<Network> {
        let config = self.config.unwrap_or_default();
        let server_name = self.server_name.unwrap();
        let keypair = self.keypair.unwrap();

        let endpoint_config = EndpointConfig::builder()
            .transport_config(config.transport_config())
            .server_name(server_name)
            .keypair(keypair)
            .build()?;
        let (endpoint, incoming) = Endpoint::new(endpoint_config, self.socket)?;

        Ok(Self::network_start(endpoint, incoming, service, config))
    }

    /// Start a network and return a handle to it
    ///
    /// Requires that this is called from within the context of a tokio runtime
    fn network_start(
        endpoint: Endpoint,
        incoming: Incoming,
        service: BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>,
        config: Config,
    ) -> Network {
        let config = Arc::new(config);

        let endpoint = Arc::new(endpoint);
        let active_peers = ActivePeers::new(config.peer_event_broadcast_channel_capacity());
        let known_peers = KnownPeers::new();

        let (connection_manager, connection_manager_handle) = ConnectionManager::new(
            config.clone(),
            endpoint.clone(),
            active_peers.clone(),
            known_peers.clone(),
            incoming,
            service,
        );

        let network = Network(Arc::new(NetworkInner {
            _config: config,
            endpoint,
            active_peers,
            known_peers,
            connection_manager_handle,
        }));

        info!("Starting network");

        tokio::spawn(connection_manager.start());

        network
    }
}

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
    pub fn bind<A: std::net::ToSocketAddrs>(addr: A) -> Result<Builder> {
        let socket = std::net::UdpSocket::bind(addr)?;
        Ok(Builder {
            socket,
            config: None,
            server_name: None,
            keypair: None,
        })
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
        self.0.connect(addr, None).await
    }

    pub async fn connect_with_peer_id(&self, addr: SocketAddr, peer_id: PeerId) -> Result<PeerId> {
        self.0.connect(addr, Some(peer_id)).await
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
    _config: Arc<Config>,
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

    async fn connect(&self, addr: SocketAddr, peer_id: Option<PeerId>) -> Result<PeerId> {
        let (sender, reciever) = tokio::sync::oneshot::channel();
        self.connection_manager_handle
            .send(ConnectionManagerRequest::ConnectRequest(
                addr, peer_id, sender,
            ))
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
