use super::request_handler::InboundRequestHandler;
use crate::{
    config::Config,
    connection::Connection,
    endpoint::{Connecting, Endpoint, Incoming, NewConnection},
    types::{Address, PeerInfo},
    ConnectionOrigin, PeerId, Request, Response, Result,
};
use bytes::Bytes;
use futures::{
    stream::{Fuse, FuturesUnordered},
    FutureExt, StreamExt,
};
use std::{
    collections::{hash_map::Entry, HashMap, HashSet},
    convert::Infallible,
    sync::{Arc, RwLock},
};
use tower::util::BoxCloneService;
use tracing::{error, info, instrument};

#[derive(Debug)]
pub enum ConnectionManagerRequest {
    ConnectRequest(
        Address,
        Option<PeerId>,
        tokio::sync::oneshot::Sender<Result<PeerId>>,
    ),
}

struct ConnectingOutput {
    connecting_result: Result<NewConnection>,
    maybe_oneshot: Option<tokio::sync::oneshot::Sender<Result<PeerId>>>,
}

pub(crate) struct ConnectionManager {
    config: Arc<Config>,

    endpoint: Arc<Endpoint>,

    mailbox: Fuse<tokio_stream::wrappers::ReceiverStream<ConnectionManagerRequest>>,
    pending_connections: FuturesUnordered<JoinHandle<ConnectingOutput>>,
    pending_dials: HashMap<PeerId, tokio::sync::oneshot::Receiver<Result<PeerId>>>,
    dial_backoff_states: HashMap<PeerId, DialBackoffState>,

    active_peers: ActivePeers,
    known_peers: KnownPeers,
    incoming: Fuse<Incoming>,

    service: BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>,
}

impl Drop for ConnectionManager {
    fn drop(&mut self) {
        self.endpoint.close()
    }
}

impl ConnectionManager {
    pub fn new(
        config: Arc<Config>,
        endpoint: Arc<Endpoint>,
        active_peers: ActivePeers,
        known_peers: KnownPeers,
        incoming: Incoming,
        service: BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>,
    ) -> (Self, tokio::sync::mpsc::Sender<ConnectionManagerRequest>) {
        let (sender, reciever) =
            tokio::sync::mpsc::channel(config.connection_manager_channel_capacity());
        (
            Self {
                config,
                endpoint,
                mailbox: tokio_stream::wrappers::ReceiverStream::new(reciever).fuse(),
                pending_connections: FuturesUnordered::new(),
                pending_dials: HashMap::default(),
                dial_backoff_states: HashMap::default(),
                active_peers,
                known_peers,
                incoming: incoming.fuse(),
                service,
            },
            sender,
        )
    }

    // Note: A great deal of care is taken to ensure that all event handlers are non-asynchronous
    // and that the only "await" points are from the select macro picking which event to handle.
    // This ensures that the event loop is able to process events at a high speed reduce the chance
    // for building up a backlog of events to process.
    #[instrument(
        name = "connection-manager",
        skip(self),
        fields(peer = %self.endpoint.peer_id().short_display(4))
    )]
    pub async fn start(mut self) {
        info!("ConnectionManager started");

        let mut interval = tokio::time::interval(self.config.connectivity_check_interval());

        loop {
            futures::select! {
                now = interval.tick().fuse() => {
                    self.handle_connectivity_check(now.into_std());
                }
                maybe_request = self.mailbox.next() => {
                    // Once all handles to the ConnectionManager's mailbox have been dropped this
                    // will yeild `None` and we can break out of the event loop and terminate the
                    // network
                    let request = if let Some(request) = maybe_request {
                        request
                    } else {
                        break;
                    };

                    info!("recieved new request");
                    match request {
                        ConnectionManagerRequest::ConnectRequest(address, peer_id, oneshot) => {
                            self.handle_connect_request(address, peer_id, oneshot);
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
            .add(&self.endpoint.peer_id(), new_connection)
        {
            let request_handler = InboundRequestHandler::new(
                new_connection,
                self.service.clone(),
                self.active_peers.clone(),
            );

            // TODO think about holding onto a handle to the ConnectionHandler task so that we can
            // cancel them and maybe even get rid of the need for the handler to hold onto the set
            // of active peers
            tokio::spawn(request_handler.start());
        }
    }

    fn handle_connect_request(
        &mut self,
        address: Address,
        peer_id: Option<PeerId>,
        oneshot: tokio::sync::oneshot::Sender<Result<PeerId>>,
    ) {
        self.dial_peer(address, peer_id, oneshot);
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
                error!("connecting failed: {e}");
                if let Some(oneshot) = maybe_oneshot {
                    let _ = oneshot.send(Err(e));
                }
            }
        }
    }

    // TODO maybe look into marking an address as invalid if we weren't able to connect due to a
    // mismatching cryptographic identity
    fn handle_connectivity_check(&mut self, now: std::time::Instant) {
        // Drain any completed dials by checking if the oneshot channel has been filled or not
        self.pending_dials
            .retain(|peer_id, oneshot| match oneshot.try_recv() {
                // We were able to successfully dial the Peer
                Ok(Ok(returned_peer_id)) => {
                    debug_assert_eq!(peer_id, &returned_peer_id);

                    self.dial_backoff_states.remove(peer_id);
                    false
                }

                // Dialing failed for some reason
                Ok(Err(_)) => {
                    match self.dial_backoff_states.entry(*peer_id) {
                        Entry::Occupied(mut entry) => {
                            entry.get_mut().update(
                                now,
                                self.config.connection_backoff(),
                                self.config.max_connection_backoff(),
                            );
                        }
                        Entry::Vacant(entry) => {
                            entry.insert(DialBackoffState::new(
                                now,
                                self.config.connection_backoff(),
                                self.config.max_connection_backoff(),
                            ));
                        }
                    }

                    false
                }

                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    panic!("BUG: connection-manager never finished dialing a peer")
                }

                // Dialing is in progress
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => true,
            });

        let active_peers = self
            .active_peers
            .peers()
            .into_iter()
            .collect::<HashSet<PeerId>>();
        let known_peers = self.known_peers.get_all();

        let eligible = known_peers
            .into_iter()
            .filter(|peer_info| {
                !peer_info.address.is_empty() // The peer has an address we can dial
                && !active_peers.contains(&peer_info.peer_id) // The node is not already connected.
                && !self.pending_dials.contains_key(&peer_info.peer_id) // There is no pending dial to this node.
                && self.dial_backoff_states  // check that `now` is after the backoff time, if it exists
                    .get(&peer_info.peer_id)
                    .map(|state| now > state.backoff)
                    .unwrap_or(true)
            })
            .collect::<Vec<_>>();

        // Limit the number of outstanding connections attempting to be established
        let number_to_dial = std::cmp::min(
            eligible.len(),
            self.config
                .max_concurrent_outstanding_connecting_connections()
                .saturating_sub(self.pending_connections.len()),
        );

        for mut peer in eligible.into_iter().take(number_to_dial) {
            let (sender, reciever) = tokio::sync::oneshot::channel();

            // Select the index of the address to dial by mapping the number of attempts we've made
            // so far into the Peer's known addresses
            let idx = self
                .dial_backoff_states
                .get(&peer.peer_id)
                .map(|state| state.attempts)
                .unwrap_or(0)
                % peer.address.len();

            let address = peer.address.remove(idx);
            self.dial_peer(address, Some(peer.peer_id), sender);
            self.pending_dials.insert(peer.peer_id, reciever);
        }
    }

    fn dial_peer(
        &mut self,
        address: Address,
        peer_id: Option<PeerId>,
        oneshot: tokio::sync::oneshot::Sender<Result<PeerId>>,
    ) {
        let connecting = if let Some(peer_id) = peer_id {
            self.endpoint
                .connect_with_expected_peer_id(address, peer_id)
        } else {
            self.endpoint.connect(address)
        };
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
}

/// The state needed to decide when and with which address another attempt to dial a peer should be
/// conducted.
#[derive(Debug)]
struct DialBackoffState {
    /// The earliest time in which we should attempt to dial this peer again.
    backoff: std::time::Instant,
    /// The number of attempts made to dial this peer.
    attempts: usize,
}

impl DialBackoffState {
    fn new(
        now: std::time::Instant,
        backoff_step: std::time::Duration,
        max_backoff: std::time::Duration,
    ) -> Self {
        let mut state = Self {
            backoff: now,
            attempts: 0,
        };

        state.update(now, backoff_step, max_backoff);
        state
    }

    fn update(
        &mut self,
        now: std::time::Instant,
        backoff_step: std::time::Duration,
        max_backoff: std::time::Duration,
    ) {
        self.attempts += 1;

        let backoff_duration = std::cmp::max(
            max_backoff,
            backoff_step.saturating_mul(self.attempts.try_into().unwrap_or(u32::MAX)),
        );

        self.backoff = now + backoff_duration;
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

#[derive(Debug, Clone)]
pub(crate) struct ActivePeers(Arc<RwLock<ActivePeersInner>>);

impl ActivePeers {
    pub fn new(channel_size: usize) -> Self {
        Self(Arc::new(RwLock::new(ActivePeersInner::new(channel_size))))
    }

    #[allow(unused)]
    pub fn subscribe(
        &self,
    ) -> (
        tokio::sync::broadcast::Receiver<crate::types::PeerEvent>,
        Vec<PeerId>,
    ) {
        self.0.read().unwrap().subscribe()
    }

    pub fn peers(&self) -> Vec<PeerId> {
        self.0.read().unwrap().peers()
    }

    pub fn get(&self, peer_id: &PeerId) -> Option<Connection> {
        self.0.read().unwrap().get(peer_id)
    }

    pub fn remove(&self, peer_id: &PeerId, reason: crate::types::DisconnectReason) {
        self.0.write().unwrap().remove(peer_id, reason)
    }

    pub fn remove_with_stable_id(
        &self,
        peer_id: PeerId,
        stable_id: usize,
        reason: crate::types::DisconnectReason,
    ) {
        self.0
            .write()
            .unwrap()
            .remove_with_stable_id(peer_id, stable_id, reason)
    }

    #[must_use]
    fn add(&self, own_peer_id: &PeerId, new_connection: NewConnection) -> Option<NewConnection> {
        self.0.write().unwrap().add(own_peer_id, new_connection)
    }
}

#[derive(Debug)]
struct ActivePeersInner {
    connections: HashMap<PeerId, Connection>,
    peer_event_sender: tokio::sync::broadcast::Sender<crate::types::PeerEvent>,
}

impl ActivePeersInner {
    fn new(channel_size: usize) -> Self {
        let (sender, _reciever) = tokio::sync::broadcast::channel(channel_size);
        Self {
            connections: Default::default(),
            peer_event_sender: sender,
        }
    }

    #[allow(unused)]
    fn subscribe(
        &self,
    ) -> (
        tokio::sync::broadcast::Receiver<crate::types::PeerEvent>,
        Vec<PeerId>,
    ) {
        let peers = self.peers();
        let reciever = self.peer_event_sender.subscribe();
        (reciever, peers)
    }

    fn peers(&self) -> Vec<PeerId> {
        self.connections.keys().copied().collect()
    }

    fn get(&self, peer_id: &PeerId) -> Option<Connection> {
        self.connections.get(peer_id).cloned()
    }

    fn remove(&mut self, peer_id: &PeerId, reason: crate::types::DisconnectReason) {
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
    fn add(
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

#[derive(Clone, Debug, Default)]
pub struct KnownPeers(Arc<RwLock<HashMap<PeerId, PeerInfo>>>);

impl KnownPeers {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn remove(&self, peer_id: &PeerId) -> Option<PeerInfo> {
        self.0.write().unwrap().remove(peer_id)
    }

    pub fn remove_all(&self) -> impl Iterator<Item = PeerInfo> {
        std::mem::take(&mut *self.0.write().unwrap()).into_values()
    }

    pub fn get(&self, peer_id: &PeerId) -> Option<PeerInfo> {
        self.0.read().unwrap().get(peer_id).cloned()
    }

    pub fn get_all(&self) -> Vec<PeerInfo> {
        self.0.read().unwrap().values().cloned().collect()
    }

    pub fn insert(&self, peer_info: PeerInfo) -> Option<PeerInfo> {
        self.0.write().unwrap().insert(peer_info.peer_id, peer_info)
    }
}
