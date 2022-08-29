use crate::{
    config::EndpointConfig,
    endpoint::{Endpoint, Incoming},
    types::Address,
    Config, PeerId, Request, Response, Result,
};
use anyhow::anyhow;
use bytes::Bytes;
use std::{convert::Infallible, net::SocketAddr, sync::Arc};
use tower::{util::BoxCloneService, Service, ServiceExt};
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

/// A builder for a [`Network`].
pub struct Builder {
    bind_address: Address,
    config: Option<Config>,
    server_name: Option<String>,

    /// Ed25519 Private Key
    private_key: Option<[u8; 32]>,
}

impl Builder {
    /// Set the [`Config`] that this network should use.
    pub fn config(mut self, config: Config) -> Self {
        self.config = Some(config);
        self
    }

    /// Set the `server-name` that will be used when constructing a self-signed X.509 certificate to
    /// be used in the TLS handshake.
    ///
    /// Traditionally a `server-name` is intended to be the DNS name that is being dialed, although
    /// since Anemo does not require parties to use DNS names, nor does it rely on a central CA,
    /// `server-name` is instead used to identify the network name that this Peer can connect to.
    /// In other words, The TLS handshake will only be successful if all parties use the same
    /// `server-name`.
    pub fn server_name<T: Into<String>>(mut self, server_name: T) -> Self {
        self.server_name = Some(server_name.into());
        self
    }

    /// Set the Ed25519 Private Key that will be used to perform the TLS handshake.
    /// The corresponding Public Key will be this node's [`PeerId`].
    pub fn private_key(mut self, private_key: [u8; 32]) -> Self {
        self.private_key = Some(private_key);
        self
    }

    #[cfg(test)]
    pub(crate) fn random_private_key(self) -> Self {
        let mut rng = rand::thread_rng();
        let mut bytes = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rng, &mut bytes[..]);

        self.private_key(bytes)
    }

    /// Start a [`Network`] and return a handle to it.
    ///
    /// # Panics
    ///
    /// This method will panic if:
    /// * not called from within the context of a tokio runtime.
    /// * no `private-key` or `server-name` were set.
    pub fn start<T>(self, service: T) -> Result<Network>
    where
        T: Clone + Send + 'static,
        T: Service<Request<Bytes>, Response = Response<Bytes>, Error = Infallible>,
        <T as Service<Request<Bytes>>>::Future: Send + 'static,
    {
        let config = self.config.unwrap_or_default();
        let server_name = self.server_name.unwrap();
        let private_key = self.private_key.unwrap();

        let endpoint_config = EndpointConfig::builder()
            .transport_config(config.transport_config())
            .server_name(server_name)
            .private_key(private_key)
            .build()?;
        let socket = std::net::UdpSocket::bind(self.bind_address)?;
        let (endpoint, incoming) = Endpoint::new(endpoint_config, socket)?;

        let service = service.boxed_clone();
        Ok(Self::network_start(endpoint, incoming, service, config))
    }

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

/// Handle to a network.
///
/// This handle can be cheaply cloned and shared across many threads.
/// Once all handles have been dropped the network will gracefully shutdown.
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
    /// Binds to the provided address, and returns a [`Builder`].
    pub fn bind<A: Into<Address>>(addr: A) -> Builder {
        Builder {
            bind_address: addr.into(),
            config: None,
            server_name: None,
            private_key: None,
        }
    }

    pub fn peers(&self) -> Vec<PeerId> {
        self.0.peers()
    }

    pub fn subscribe(
        &self,
    ) -> (
        tokio::sync::broadcast::Receiver<crate::types::PeerEvent>,
        Vec<PeerId>,
    ) {
        self.0.active_peers.subscribe()
    }

    pub fn peer(&self, peer_id: PeerId) -> Option<Peer> {
        self.0.peer(peer_id)
    }

    pub fn known_peers(&self) -> &KnownPeers {
        self.0.known_peers()
    }

    pub async fn connect<A: Into<Address>>(&self, addr: A) -> Result<PeerId> {
        self.0.connect(addr.into(), None).await
    }

    pub async fn connect_with_peer_id<A: Into<Address>>(
        &self,
        addr: A,
        peer_id: PeerId,
    ) -> Result<PeerId> {
        self.0.connect(addr.into(), Some(peer_id)).await
    }

    pub fn disconnect(&self, peer: PeerId) -> Result<()> {
        self.0.disconnect(peer)
    }

    pub async fn rpc(&self, peer: PeerId, request: Request<Bytes>) -> Result<Response<Bytes>> {
        self.0.rpc(peer, request).await
    }

    /// Return the local address that this Network is listening on.
    pub fn local_addr(&self) -> SocketAddr {
        self.0.local_addr()
    }

    /// Return the [`PeerId`] of this Network.
    pub fn peer_id(&self) -> PeerId {
        self.0.peer_id()
    }

    #[cfg(test)]
    pub(crate) fn downgrade(&self) -> NetworkRef {
        NetworkRef(Arc::downgrade(&self.0))
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

    async fn connect(&self, addr: Address, peer_id: Option<PeerId>) -> Result<PeerId> {
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

    fn peer(&self, peer_id: PeerId) -> Option<Peer> {
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

// TODO look into providing `NetworkRef` via extention to request handlers
#[derive(Clone)]
#[cfg(test)]
pub(crate) struct NetworkRef(std::sync::Weak<NetworkInner>);

#[cfg(test)]
impl NetworkRef {
    pub fn upgrade(&self) -> Option<Network> {
        self.0.upgrade().map(Network)
    }
}
