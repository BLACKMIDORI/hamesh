use crate::control_datagram::ControlDatagram;
use crate::settings_models::Protocol;
use std::sync::atomic::AtomicU64;

pub struct InboundClient {
    remote_host_port: u16,
    remote_client_port: u16,
    remote_protocol: Protocol,
    pub sender: tokio::sync::mpsc::Sender<ControlDatagram>,
    pub last_download_activity: AtomicU64,
}

impl InboundClient {
    pub fn new(
        remote_host_port: u16,
        remote_client_port: u16,
        remote_protocol: Protocol,
        sender: tokio::sync::mpsc::Sender<ControlDatagram>,
    ) -> InboundClient {
        InboundClient {
            remote_host_port,
            remote_client_port,
            remote_protocol,
            sender,
            last_download_activity: AtomicU64::new(0),
        }
    }
}
