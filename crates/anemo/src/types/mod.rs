mod address;
mod peer_id;
pub mod request;
pub mod response;

pub use address::Address;
pub use peer_id::{ConnectionOrigin, Direction, PeerId};

pub use http::Extensions;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u16)]
pub enum Version {
    V1 = 1,
}

impl Version {
    pub fn new(version: u16) -> crate::Result<Self> {
        match version {
            1 => Ok(Version::V1),
            _ => Err(anyhow::anyhow!("invalid version {}", version)),
        }
    }

    pub fn to_u16(self) -> u16 {
        self as u16
    }
}

impl Default for Version {
    fn default() -> Self {
        Self::V1
    }
}

pub type HeaderMap = std::collections::HashMap<String, String>;

pub mod header {
    pub const CONTENT_TYPE: &str = "content-type";
    pub const STATUS_MESSAGE: &str = "status-message";
    /// Timeout in nanoseconds, encoded as an u64
    pub const TIMEOUT: &str = "timeout";
}

#[derive(Clone, Copy, Debug)]
pub enum PeerAffinity {
    /// Always attempt to maintain a connection with this Peer.
    High,
    /// Never attempt to maintain a connection with this Peer.
    ///
    /// Inbound connection requests from these Peers are rejected.
    Never,
}

#[derive(Clone, Debug)]
pub struct PeerInfo {
    pub peer_id: PeerId,
    pub affinity: PeerAffinity,
    pub address: Vec<Address>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerEvent {
    NewPeer(PeerId),
    LostPeer(PeerId, DisconnectReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisconnectReason {
    Requested,
    ConnectionLost,
}
