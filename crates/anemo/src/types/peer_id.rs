/// Length of a PeerId, based on the length of an ed25519 public key
const PEER_ID_LENGTH: usize = 32;

#[derive(Copy, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct PeerId(pub [u8; PEER_ID_LENGTH]);

impl PeerId {
    pub fn short_display(&self, len: u8) -> impl std::fmt::Display + '_ {
        ShortPeerId(self, len)
    }

    #[cfg(test)]
    fn random() -> Self {
        let mut rng = rand::thread_rng();
        let mut bytes = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rng, &mut bytes[..]);
        Self(bytes)
    }
}

impl std::fmt::Display for PeerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let len = f.precision().unwrap_or(PEER_ID_LENGTH);
        for byte in self.0.iter().take(len) {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl std::fmt::Debug for PeerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PeerId({self})")
    }
}

impl<'de> serde::Deserialize<'de> for PeerId {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;

        if deserializer.is_human_readable() {
            let s = <String>::deserialize(deserializer)?;

            hex::FromHex::from_hex(s)
                .map_err(D::Error::custom)
                .map(Self)
        } else {
            <[u8; PEER_ID_LENGTH]>::deserialize(deserializer).map(Self)
        }
    }
}

impl serde::Serialize for PeerId {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if serializer.is_human_readable() {
            hex::encode(self.0).serialize(serializer)
        } else {
            self.0.serialize(serializer)
        }
    }
}

struct ShortPeerId<'a>(&'a PeerId, u8);

impl<'a> std::fmt::Display for ShortPeerId<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:.len$}", self.0, len = self.1.into())
    }
}

/// Direction of a network event.
#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub enum Direction {
    /// `Inbound` indicates that the remote side initiated the network event.
    Inbound,
    /// `Outbound` indicates that we initiated the network event.
    Outbound,
}

impl Direction {
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Inbound => "inbound",
            Direction::Outbound => "outbound",
        }
    }
}

impl std::fmt::Debug for Direction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Direction::")?;
        let direction = match self {
            Direction::Inbound => "Inbound",
            Direction::Outbound => "Outbound",
        };
        f.write_str(direction)
    }
}

impl std::fmt::Display for Direction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Origin of how a Connection was established.
///
/// `Inbound` indicates that we are the listener for this connection.
/// `Outbound` indicates that we are the dialer for this connection.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct ConnectionOrigin(Direction);

impl ConnectionOrigin {
    #[allow(non_upper_case_globals)]
    pub const Inbound: ConnectionOrigin = ConnectionOrigin(Direction::Inbound);
    #[allow(non_upper_case_globals)]
    pub const Outbound: ConnectionOrigin = ConnectionOrigin(Direction::Outbound);

    pub fn as_str(self) -> &'static str {
        self.0.as_str()
    }
}

impl std::fmt::Display for ConnectionOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod test {
    use super::{ConnectionOrigin, Direction, PeerId};

    #[test]
    fn direction_debug() {
        let inbound = format!("{:?}", Direction::Inbound);
        let outbound = format!("{:?}", Direction::Outbound);

        assert_eq!(inbound, "Direction::Inbound");
        assert_eq!(outbound, "Direction::Outbound");
    }

    #[test]
    fn direction_display() {
        let inbound = Direction::Inbound.to_string();
        let outbound = Direction::Outbound.to_string();

        assert_eq!(inbound, "inbound");
        assert_eq!(outbound, "outbound");
    }

    #[test]
    fn connection_origin_debug() {
        let inbound = format!("{:?}", ConnectionOrigin::Inbound);
        let outbound = format!("{:?}", ConnectionOrigin::Outbound);

        assert_eq!(inbound, "ConnectionOrigin(Direction::Inbound)");
        assert_eq!(outbound, "ConnectionOrigin(Direction::Outbound)");
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

    #[test]
    fn test_serde_json() {
        let peer_id = PeerId::random();
        let hex = hex::encode(peer_id.0);
        let json_hex = format!("\"{hex}\"");

        let json = serde_json::to_string(&peer_id).unwrap();
        let json_peer_id: PeerId = serde_json::from_str(&json_hex).unwrap();

        assert_eq!(json, json_hex);
        assert_eq!(peer_id, json_peer_id);
    }

    #[test]
    fn test_bincode() {
        let peer_id = PeerId::random();

        let bincode = bincode::serialize(&peer_id).unwrap();
        let bincode_peer_id: PeerId = bincode::deserialize(&peer_id.0).unwrap();

        assert_eq!(bincode, peer_id.0);
        assert_eq!(peer_id, bincode_peer_id);
    }
}
