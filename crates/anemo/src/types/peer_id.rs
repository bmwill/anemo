/// Length of a PeerId, based on the length of an ed25519 public key
const PEER_ID_LENGTH: usize = 32;

#[derive(Copy, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct PeerId(pub [u8; PEER_ID_LENGTH]);

impl PeerId {
    pub fn short_display(&self, len: u8) -> impl std::fmt::Display + '_ {
        ShortPeerId(self, len)
    }
}

impl std::fmt::Display for PeerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let len = f.precision().unwrap_or(PEER_ID_LENGTH);
        for byte in self.0.iter().take(len) {
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
        let peer_id = PeerId([42; 32]);

        let num_bytes_to_display = 4;
        let num_hex_digits = 2 * num_bytes_to_display;

        let short_str = peer_id.short_display(num_bytes_to_display).to_string();
        assert_eq!(short_str.len(), num_hex_digits as usize);
    }
}
