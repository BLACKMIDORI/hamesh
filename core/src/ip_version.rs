use std::fmt::Formatter;
use crate::settings_models::Protocol;

pub enum IpVersion {
    Ipv4,
    Ipv6,
}

impl std::fmt::Display for IpVersion {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            fmt,
            "{}",
            match self {
                IpVersion::Ipv4 => {
                    "IPv4"
                }
                IpVersion::Ipv6 => {
                    "IPv6"
                }
            }
        )
    }
}
