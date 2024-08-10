use hamesh::ip_version::IpVersion;
use log::error;

pub fn get_ip_version() -> Option<IpVersion> {
    let args: Vec<String> = std::env::args().collect();
    let ip_version_str = &args[2];
    let ip_version = match ip_version_str.as_str() {
        "ipv4" => IpVersion::Ipv4,
        "ipv6" => IpVersion::Ipv6,
        _ => {
            error!("invalid ip version: {}", ip_version_str);
            return None;
        }
    };
    Some(ip_version)
}
