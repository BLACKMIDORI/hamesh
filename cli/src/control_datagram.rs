use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use crate::settings_models::{ClientDataSettings, PortSettings, Protocol, ServerDataSettings};

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ControlDatagram {
    pub version: i32,
    pub r#type: String,
    pub content: HashMap<String, String>,
}

impl ControlDatagram {
    pub fn syn() -> ControlDatagram {
        ControlDatagram { version: 1, r#type: "SYN".to_string(), content: HashMap::new() }
    }
    pub fn ack(id: &str) -> ControlDatagram {
        let mut content = HashMap::new();
        content.insert("id".to_string(), id.to_string());
        ControlDatagram { version: 1, r#type: "ACK".to_string(), content }
    }
    pub fn input_port(id: &str, settings: PortSettings) -> ControlDatagram {
        let mut content = HashMap::new();
        content.insert("id".to_string(), id.to_string());
        content.insert("protocol".to_string(), match settings.protocol {
            Protocol::Tcp => "tcp",
            Protocol::Udp => "udp"
        }.to_string());
        content.insert("hostPort".to_string(), settings.host_port.to_string());
        content.insert("remoteHostPort".to_string(), settings.remote_host_port.to_string());
        ControlDatagram {
            version: 1,
            r#type: "input_port".to_string(),
            content,
        }
    }

    pub fn client_data(id: &str, sequence: u64, settings: ClientDataSettings, base64: &str) -> ControlDatagram {
        let mut content = HashMap::new();
        content.insert("id".to_string(), id.to_string());
        content.insert("sequence".to_string(), sequence.to_string());
        content.insert("protocol".to_string(), match settings.protocol {
            Protocol::Tcp => "tcp",
            Protocol::Udp => "udp"
        }.to_string());
        content.insert("hostPort".to_string(), settings.host_port.to_string());
        content.insert("hostClientPort".to_string(), settings.host_client_port.to_string());
        content.insert("base64".to_string(), base64.to_string());
        ControlDatagram {
            version: 1,
            r#type: "client_data".to_string(),
            content,
        }
    }

    pub fn server_data(id: &str, sequence: u64,  settings: ServerDataSettings, base64: &str) -> ControlDatagram {
        let mut content = HashMap::new();
        content.insert("id".to_string(), id.to_string());
        content.insert("sequence".to_string(), sequence.to_string());
        content.insert("protocol".to_string(), match settings.protocol {
            Protocol::Tcp => "tcp",
            Protocol::Udp => "udp"
        }.to_string());
        content.insert("remoteHostPort".to_string(), settings.remote_host_port.to_string());
        content.insert("remoteHostClientPort".to_string(), settings.remote_host_client_port.to_string());
        content.insert("base64".to_string(), base64.to_string());
        ControlDatagram {
            version: 1,
            r#type: "server_data".to_string(),
            content,
        }
    }
}