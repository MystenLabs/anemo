/// Representation of a network address that is dial-able in Anemo
#[derive(Clone, Debug)]
pub enum Address {
    /// A plain SocketAddr
    SocketAddr(std::net::SocketAddr),

    /// Host and Port where 'host' should be either a string representation of an IpAddr address or
    /// a host name that will be resolved via DNS.
    HostAndPort { host: Box<str>, port: u16 },

    /// A string representation of a SocketAddr or a string like `<host_name>:<port>` pair where
    /// `<port>` is a u16 value.
    AddressString(Box<str>),
}

impl Address {
    pub(crate) fn resolve(&self) -> std::io::Result<std::net::SocketAddr> {
        std::net::ToSocketAddrs::to_socket_addrs(self).and_then(|mut iter| {
            iter.next().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "unable to resolve host")
            })
        })
    }
}

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Address::SocketAddr(addr) => f.write_fmt(format_args!("{addr}")),
            Address::HostAndPort { host, port } => f.write_fmt(format_args!("{host}:{port}")),
            Address::AddressString(addr) => f.write_fmt(format_args!("{addr}")),
        }
    }
}

impl std::net::ToSocketAddrs for Address {
    type Iter = Box<dyn Iterator<Item = std::net::SocketAddr>>;

    fn to_socket_addrs(&self) -> std::io::Result<Self::Iter> {
        match self {
            Address::SocketAddr(addr) => Ok(Box::new(std::iter::once(*addr))),
            Address::HostAndPort { host, port } => (host.as_ref(), *port)
                .to_socket_addrs()
                .map(|iter| Box::new(iter) as Self::Iter),
            Address::AddressString(addr) => addr
                .to_socket_addrs()
                .map(|iter| Box::new(iter) as Self::Iter),
        }
    }
}

impl From<std::net::SocketAddr> for Address {
    fn from(addr: std::net::SocketAddr) -> Self {
        Self::SocketAddr(addr)
    }
}

impl From<std::net::SocketAddrV4> for Address {
    fn from(addr: std::net::SocketAddrV4) -> Self {
        std::net::SocketAddr::from(addr).into()
    }
}

impl From<std::net::SocketAddrV6> for Address {
    fn from(addr: std::net::SocketAddrV6) -> Self {
        std::net::SocketAddr::from(addr).into()
    }
}

impl From<(std::net::IpAddr, u16)> for Address {
    fn from(addr: (std::net::IpAddr, u16)) -> Self {
        std::net::SocketAddr::from(addr).into()
    }
}

impl From<(std::net::Ipv4Addr, u16)> for Address {
    fn from(addr: (std::net::Ipv4Addr, u16)) -> Self {
        std::net::SocketAddr::from(addr).into()
    }
}

impl From<(std::net::Ipv6Addr, u16)> for Address {
    fn from(addr: (std::net::Ipv6Addr, u16)) -> Self {
        std::net::SocketAddr::from(addr).into()
    }
}

impl<'a> From<(&'a str, u16)> for Address {
    fn from(addr: (&'a str, u16)) -> Self {
        Self::HostAndPort {
            host: addr.0.into(),
            port: addr.1,
        }
    }
}

impl From<(Box<str>, u16)> for Address {
    fn from(addr: (Box<str>, u16)) -> Self {
        Self::HostAndPort {
            host: addr.0,
            port: addr.1,
        }
    }
}

impl From<(String, u16)> for Address {
    fn from(addr: (String, u16)) -> Self {
        Self::HostAndPort {
            host: addr.0.into(),
            port: addr.1,
        }
    }
}

impl<'a> From<&'a str> for Address {
    fn from(addr: &'a str) -> Self {
        Self::AddressString(addr.into())
    }
}

impl From<String> for Address {
    fn from(addr: String) -> Self {
        Self::AddressString(addr.into())
    }
}

impl From<Box<str>> for Address {
    fn from(addr: Box<str>) -> Self {
        Self::AddressString(addr)
    }
}
