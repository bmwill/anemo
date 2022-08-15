#[derive(Copy, Clone, Eq)]
pub struct PeerId(pub ed25519_dalek::PublicKey);

impl PeerId {
    pub fn short_display(&self, len: u8) -> impl std::fmt::Display + '_ {
        ShortPeerId(self, len)
    }
}

impl std::hash::Hash for PeerId {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.as_bytes().hash(state);
    }
}

impl std::cmp::PartialEq for PeerId {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_bytes() == other.0.as_bytes()
    }
}

impl std::cmp::PartialOrd for PeerId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.0.as_bytes().partial_cmp(other.0.as_bytes())
    }
}

impl std::cmp::Ord for PeerId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.as_bytes().cmp(other.0.as_bytes())
    }
}

impl std::fmt::Display for PeerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let len = f.precision().unwrap_or(ed25519_dalek::PUBLIC_KEY_LENGTH);
        for byte in self.0.as_bytes().iter().take(len) {
            write!(f, "{:02x}", byte)?;
        }
        Ok(())
    }
}

impl std::fmt::Debug for PeerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PeerId({})", self)
    }
}

struct ShortPeerId<'a>(&'a PeerId, u8);

impl<'a> std::fmt::Display for ShortPeerId<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:.len$}", self.0, len = self.1.into())
    }
}

/// Origin of how a Connection was established.
#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub enum ConnectionOrigin {
    /// `Inbound` indicates that we are the listener for this connection.
    Inbound,
    /// `Outbound` indicates that we are the dialer for this connection.
    Outbound,
}

impl ConnectionOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            ConnectionOrigin::Inbound => "inbound",
            ConnectionOrigin::Outbound => "outbound",
        }
    }
}

impl std::fmt::Debug for ConnectionOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ConnectionOrigin::")?;
        let direction = match self {
            ConnectionOrigin::Inbound => "Inbound",
            ConnectionOrigin::Outbound => "Outbound",
        };
        f.write_str(direction)
    }
}

impl std::fmt::Display for ConnectionOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod test {
    use super::{ConnectionOrigin, PeerId};

    #[test]
    fn connection_origin_debug() {
        let inbound = format!("{:?}", ConnectionOrigin::Inbound);
        let outbound = format!("{:?}", ConnectionOrigin::Outbound);

        assert_eq!(inbound, "ConnectionOrigin::Inbound");
        assert_eq!(outbound, "ConnectionOrigin::Outbound");
    }

    #[test]
    fn connection_origin_display() {
        let inbound = ConnectionOrigin::Inbound.to_string();
        let outbound = ConnectionOrigin::Outbound.to_string();

        assert_eq!(inbound, "inbound");
        assert_eq!(outbound, "outbound");
    }

    #[test]
    fn short_peer_id() {
        let mut rng = rand::thread_rng();
        let keypair = ed25519_dalek::Keypair::generate(&mut rng);
        let peer_id = PeerId(keypair.public);

        let num_bytes_to_display = 4;
        let num_hex_digits = 2 * num_bytes_to_display;

        let short_str = peer_id.short_display(num_bytes_to_display).to_string();
        assert_eq!(short_str.len(), num_hex_digits as usize);
    }
}
