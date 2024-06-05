use std::fmt::Formatter;
use std::hash::{Hash};
use std::str::FromStr;

#[derive(Debug, Clone, Copy, Eq, Hash)]
pub enum Protocol {
    Tcp,
    Udp,
}
impl FromStr for Protocol {
    type Err = String;

    fn from_str(protocol: &str) -> Result<Self, Self::Err> {
        match protocol {
            "tcp" => Ok(Protocol::Tcp),
            "udp" => Ok(Protocol::Udp),
            _ => Err("Invalid protocol. Must be tcp or udp".to_string()),
        }
    }
}

impl PartialEq for Protocol {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Tcp, Self::Tcp) | (Self::Udp, Self::Udp) => true,
            _ => false,
        }
    }
}
#[derive(Debug, Clone, Copy)]
pub struct PortSettings {
    pub protocol: Protocol,
    pub host_port: u16,
    pub remote_host_port: u16,
}
impl PartialEq for PortSettings {
    fn eq(&self, other: &Self) -> bool {
        if self.protocol != other.protocol {
            return false;
        }
        if self.host_port != other.host_port {
            return false;
        }
        if self.remote_host_port != other.remote_host_port {
            return false;
        }
        true
    }
}

impl std::fmt::Display for PortSettings {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            fmt,
            "{}:{}/{}",
            self.host_port, self.remote_host_port, self.protocol,
        )
    }
}

impl std::fmt::Display for Protocol {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            fmt,
            "{}",
            match self {
                Protocol::Tcp => {
                    "tcp"
                }
                Protocol::Udp => {
                    "udp"
                }
            }
        )
    }
}

#[derive(Debug, Clone)]
pub struct ClientDataSettings {
    pub protocol: Protocol,
    pub host_port: u16,
    pub host_client_port: u16,
}
#[derive(Debug)]
pub struct ServerDataSettings {
    pub protocol: Protocol,
    pub remote_host_port: u16,
    pub remote_host_client_port: u16,
}
