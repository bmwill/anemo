use super::request_handler::InboundRequestHandler;
use crate::{
    config::Config,
    connection::Connection,
    endpoint::{Connecting, Endpoint},
    types::{Address, DisconnectReason, PeerEvent, PeerInfo},
    ConnectionOrigin, PeerId, Request, Response, Result,
};
use bytes::Bytes;
use futures::FutureExt;
use std::{
    collections::{hash_map::Entry, HashMap},
    convert::Infallible,
    sync::{Arc, RwLock},
};
use tokio::{
    sync::{broadcast, mpsc, oneshot},
    task::JoinSet,
};
use tower::util::BoxCloneService;
use tracing::{debug, info, instrument, trace};

#[derive(Debug)]
pub enum ConnectionManagerRequest {
    ConnectRequest(Address, Option<PeerId>, oneshot::Sender<Result<PeerId>>),
}

struct ConnectingOutput {
    connecting_result: Result<Connection>,
    maybe_oneshot: Option<oneshot::Sender<Result<PeerId>>>,
    target_address: Option<Address>,
    target_peer_id: Option<PeerId>,
}

pub(crate) struct ConnectionManager {
    config: Arc<Config>,

    endpoint: Arc<Endpoint>,

    mailbox: mpsc::Receiver<ConnectionManagerRequest>,
    pending_connections: JoinSet<ConnectingOutput>,
    pending_dials: HashMap<PeerId, oneshot::Receiver<Result<PeerId>>>,
    dial_backoff_states: HashMap<PeerId, DialBackoffState>,

    active_peers: ActivePeers,
    known_peers: KnownPeers,

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
        service: BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>,
    ) -> (Self, mpsc::Sender<ConnectionManagerRequest>) {
        let (sender, reciever) = mpsc::channel(config.connection_manager_channel_capacity());
        (
            Self {
                config,
                endpoint,
                mailbox: reciever,
                pending_connections: JoinSet::new(),
                pending_dials: HashMap::default(),
                dial_backoff_states: HashMap::default(),
                active_peers,
                known_peers,
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

        // Add some jitter to the interval we perform connectivity checks in order to help reduce
        // the probability of simultaneous dials, especially in non-production environments where
        // most nodes are spun up around the same time.
        //
        // TODO maybe look into adding jitter directly onto the dials themsevles so that dials are
        // more smeared out over time to avoid spiky load / thundering herd issues where all dial
        // requests happen around the same time.
        let jitter = std::time::Duration::from_millis(1_000).mul_f64(rand::random::<f64>());
        let mut interval =
            tokio::time::interval(self.config.connectivity_check_interval() + jitter);

        loop {
            tokio::select! {
                now = interval.tick() => {
                    self.handle_connectivity_check(now.into_std());
                }
                maybe_request = self.mailbox.recv() => {
                    // Once all handles to the ConnectionManager's mailbox have been dropped this
                    // will yeild `None` and we can break out of the event loop and terminate the
                    // network
                    let request = if let Some(request) = maybe_request {
                        request
                    } else {
                        break;
                    };

                    match request {
                        ConnectionManagerRequest::ConnectRequest(address, peer_id, oneshot) => {
                            self.handle_connect_request(address, peer_id, oneshot);
                        }
                    }
                }
                connecting = self.endpoint.accept() => {
                    if let Some(connecting) = connecting {
                        self.handle_incoming(connecting);
                    }
                },
                Some(connecting_output) = self.pending_connections.join_next() => {
                    self.handle_connecting_result(connecting_output.unwrap());
                },
            }
        }

        info!("ConnectionManager ended");
    }

    fn add_peer(&mut self, new_connection: Connection) {
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
        oneshot: oneshot::Sender<Result<PeerId>>,
    ) {
        self.dial_peer(address, peer_id, oneshot);
    }

    fn handle_incoming(&mut self, connecting: Connecting) {
        trace!("recieved new incoming connection");
        let connecting = connecting.map(|connecting_result| ConnectingOutput {
            connecting_result,
            maybe_oneshot: None,
            target_address: None,
            target_peer_id: None,
        });
        self.pending_connections.spawn(connecting);
    }

    fn handle_connecting_result(
        &mut self,
        ConnectingOutput {
            connecting_result,
            maybe_oneshot,
            target_address,
            target_peer_id,
        }: ConnectingOutput,
    ) {
        match connecting_result {
            Ok(new_connection) => {
                let peer_id = new_connection.peer_id();
                debug!(peer_id =% peer_id, "new connection");
                self.add_peer(new_connection);
                if let Some(oneshot) = maybe_oneshot {
                    let _ = oneshot.send(Ok(peer_id));
                }
            }
            Err(e) => {
                debug!(
                    target_address = ?target_address,
                    target_peer_id = ?target_peer_id,
                    "connecting failed: {e}"
                );
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

                Err(oneshot::error::TryRecvError::Closed) => {
                    panic!("BUG: connection-manager never finished dialing a peer")
                }

                // Dialing is in progress
                Err(oneshot::error::TryRecvError::Empty) => true,
            });

        let eligible: Vec<_> = {
            let active_peers = self.active_peers.inner();
            let known_peers = self.known_peers.inner();

            known_peers
                .values()
                .filter(|peer_info| {
                    peer_info.peer_id != self.endpoint.peer_id() // We don't dial ourself
                    && !peer_info.address.is_empty() // The peer has an address we can dial
                    && !active_peers.contains(&peer_info.peer_id) // The node is not already connected.
                    && !self.pending_dials.contains_key(&peer_info.peer_id) // There is no pending dial to this node.
                    && self.dial_backoff_states  // check that `now` is after the backoff time, if it exists
                        .get(&peer_info.peer_id)
                        .map(|state| now > state.backoff)
                        .unwrap_or(true)
                })
                .cloned()
                .collect()
        };

        // Limit the number of outstanding connections attempting to be established
        let number_to_dial = std::cmp::min(
            eligible.len(),
            self.config
                .max_concurrent_outstanding_connecting_connections()
                .saturating_sub(self.pending_connections.len()),
        );

        for mut peer in eligible.into_iter().take(number_to_dial) {
            let (sender, reciever) = oneshot::channel();

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
        oneshot: oneshot::Sender<Result<PeerId>>,
    ) {
        let target_address = Some(address.clone());
        let connecting = if let Some(peer_id) = peer_id {
            self.endpoint
                .connect_with_expected_peer_id(address, peer_id)
        } else {
            self.endpoint.connect(address)
        };
        let connecting = async move {
            let connecting_result = match connecting {
                Ok(connecting) => connecting.await,
                Err(e) => Err(e),
            };
            ConnectingOutput {
                connecting_result,
                maybe_oneshot: Some(oneshot),
                target_address,
                target_peer_id: peer_id,
            }
        };
        self.pending_connections.spawn(connecting);
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

        let backoff_duration = std::cmp::min(
            max_backoff,
            backoff_step.saturating_mul(self.attempts.try_into().unwrap_or(u32::MAX)),
        );

        self.backoff = now + backoff_duration;
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ActivePeers(Arc<RwLock<ActivePeersInner>>);

impl ActivePeers {
    pub fn new(channel_size: usize) -> Self {
        Self(Arc::new(RwLock::new(ActivePeersInner::new(channel_size))))
    }

    #[allow(unused)]
    pub fn subscribe(&self) -> (broadcast::Receiver<PeerEvent>, Vec<PeerId>) {
        self.inner().subscribe()
    }

    pub fn peers(&self) -> Vec<PeerId> {
        self.inner().peers()
    }

    pub fn get(&self, peer_id: &PeerId) -> Option<Connection> {
        self.inner().get(peer_id)
    }

    pub fn remove(&self, peer_id: &PeerId, reason: DisconnectReason) {
        self.inner_mut().remove(peer_id, reason)
    }

    pub fn remove_with_stable_id(
        &self,
        peer_id: PeerId,
        stable_id: usize,
        reason: DisconnectReason,
    ) {
        self.inner_mut()
            .remove_with_stable_id(peer_id, stable_id, reason)
    }

    #[must_use]
    fn add(&self, own_peer_id: &PeerId, new_connection: Connection) -> Option<Connection> {
        self.inner_mut().add(own_peer_id, new_connection)
    }

    fn inner(&self) -> std::sync::RwLockReadGuard<'_, ActivePeersInner> {
        self.0.read().unwrap()
    }

    fn inner_mut(&self) -> std::sync::RwLockWriteGuard<'_, ActivePeersInner> {
        self.0.write().unwrap()
    }
}

#[derive(Debug)]
struct ActivePeersInner {
    connections: HashMap<PeerId, Connection>,
    peer_event_sender: broadcast::Sender<PeerEvent>,
}

impl ActivePeersInner {
    fn new(channel_size: usize) -> Self {
        let (sender, _reciever) = broadcast::channel(channel_size);
        Self {
            connections: Default::default(),
            peer_event_sender: sender,
        }
    }

    fn subscribe(&self) -> (broadcast::Receiver<PeerEvent>, Vec<PeerId>) {
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

    fn contains(&self, peer_id: &PeerId) -> bool {
        self.connections.contains_key(peer_id)
    }

    fn remove(&mut self, peer_id: &PeerId, reason: DisconnectReason) {
        if let Some(connection) = self.connections.remove(peer_id) {
            // maybe actually provide reason to other side?
            connection.close();

            self.send_event(PeerEvent::LostPeer(*peer_id, reason));
        }
    }

    fn remove_with_stable_id(
        &mut self,
        peer_id: PeerId,
        stable_id: usize,
        reason: DisconnectReason,
    ) {
        match self.connections.entry(peer_id) {
            Entry::Occupied(entry) => {
                // Only remove the entry if the stable id matches
                if entry.get().stable_id() == stable_id {
                    let (peer_id, connection) = entry.remove_entry();
                    // maybe actually provide reason to other side?
                    connection.close();

                    self.send_event(PeerEvent::LostPeer(peer_id, reason));
                }
            }
            Entry::Vacant(_) => {}
        }
    }

    fn send_event(&self, event: PeerEvent) {
        // We don't care if anyone is listening
        let _ = self.peer_event_sender.send(event);
    }

    #[must_use]
    fn add(&mut self, own_peer_id: &PeerId, new_connection: Connection) -> Option<Connection> {
        // TODO drop Connection if you've somehow connected out ourself

        let peer_id = new_connection.peer_id();
        match self.connections.entry(peer_id) {
            Entry::Occupied(mut entry) => {
                if Self::simultaneous_dial_tie_breaking(
                    own_peer_id,
                    &peer_id,
                    entry.get().origin(),
                    new_connection.origin(),
                ) {
                    debug!("closing old connection with {peer_id:?} to mitigate simultaneous dial");
                    let old_connection = entry.insert(new_connection.clone());
                    old_connection.close();
                    self.send_event(PeerEvent::LostPeer(peer_id, DisconnectReason::Requested));
                } else {
                    debug!("closing new connection with {peer_id:?} to mitigate simultaneous dial");
                    new_connection.close();
                    // Early return to avoid standing up Incoming Request handlers
                    return None;
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(new_connection.clone());
            }
        }

        self.send_event(PeerEvent::NewPeer(peer_id));

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
        self.inner_mut().remove(peer_id)
    }

    pub fn remove_all(&self) -> impl Iterator<Item = PeerInfo> {
        std::mem::take(&mut *self.inner_mut()).into_values()
    }

    pub fn get(&self, peer_id: &PeerId) -> Option<PeerInfo> {
        self.inner().get(peer_id).cloned()
    }

    pub fn get_all(&self) -> Vec<PeerInfo> {
        self.inner().values().cloned().collect()
    }

    pub fn insert(&self, peer_info: PeerInfo) -> Option<PeerInfo> {
        self.inner_mut().insert(peer_info.peer_id, peer_info)
    }

    fn inner(&self) -> std::sync::RwLockReadGuard<'_, HashMap<PeerId, PeerInfo>> {
        self.0.read().unwrap()
    }

    fn inner_mut(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<PeerId, PeerInfo>> {
        self.0.write().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::DialBackoffState;
    use std::time::{Duration, Instant};

    #[test]
    fn backoff() {
        // GIVEN
        let now = Instant::now();

        let back_off_step = Duration::from_secs(5);
        let max_back_off = Duration::from_secs(60);

        // WHEN
        let mut state = DialBackoffState::new(now, back_off_step, max_back_off);

        // THEN
        assert_eq!(state.attempts, 1);
        assert_eq!(state.backoff - now, back_off_step.saturating_mul(1));

        // WHEN
        for attempt in 2..=12 {
            state.update(now, back_off_step, max_back_off);

            // THEN
            assert_eq!(state.attempts, attempt);
            assert_eq!(
                state.backoff - now,
                back_off_step.saturating_mul(attempt as u32)
            );
        }

        for attempt in 13..=15 {
            // WHEN
            state.update(now, back_off_step, max_back_off);

            // THEN we should expect to get from now on only the max backoff
            assert_eq!(state.attempts, attempt);
            assert_eq!(state.backoff - now, max_back_off);
        }
    }
}
