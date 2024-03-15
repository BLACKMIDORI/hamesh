use crate::control_datagram::ControlDatagram;
use crate::settings_models::{PortSettings, Protocol};

pub struct OutboundServer {
    local_host_port: u16,
    pub remote_host_port: u16,
    pub remote_protocol: Protocol,
    pub sender: tokio::sync::mpsc::Sender<ControlDatagram>,
}

impl OutboundServer {
    pub fn new(
        local_host_port: u16,
        remote_host_port: u16,
        remote_protocol: Protocol,
        sender: tokio::sync::mpsc::Sender<ControlDatagram>,
    ) -> OutboundServer {
        OutboundServer {
            local_host_port,
            remote_host_port,
            remote_protocol,
            sender,
        }
    }

    pub async fn start(port_settings: PortSettings) {}
}
