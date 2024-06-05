use crate::settings_models::{ClientDataSettings, PortSettings, Protocol, ServerDataSettings};
use serde::{Deserialize, Serialize};
use sha256::digest;
use std::collections::HashMap;
use std::error::Error;

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ControlDatagram {
    pub version: i32,
    pub r#type: String,
    pub content: HashMap<String, String>,
}

impl ControlDatagram {
    pub fn from(size: usize, buffer: &[u8]) -> Result<ControlDatagram, Box<dyn Error>> {
        let json_str = std::str::from_utf8(&buffer[..size])?;
        let obj = serde_json::from_str::<ControlDatagram>(json_str)?;
        Ok(obj)
    }

    pub fn to_vec(&self) -> Result<Vec<u8>, Box<dyn Error>> {
        Ok(serde_json::to_vec(self)?)
    }
    pub fn syn() -> ControlDatagram {
        ControlDatagram {
            version: 1,
            r#type: "SYN".to_string(),
            content: HashMap::new(),
        }
    }
    pub fn ack(datagram: &ControlDatagram) -> ControlDatagram {
        let mut content = HashMap::new();
        content.insert(
            "id".to_string(),
            datagram
                .content
                .get("id")
                .unwrap_or(&String::from(&datagram.r#type))
                .clone(),
        );
        ControlDatagram {
            version: 1,
            r#type: "ACK".to_string(),
            content,
        }
    }
    pub fn input_port(id: &str, settings: &PortSettings) -> ControlDatagram {
        let mut content = HashMap::new();
        content.insert("id".to_string(), id.to_string());
        content.insert(
            "protocol".to_string(),
            match settings.protocol {
                Protocol::Tcp => "tcp",
                Protocol::Udp => "udp",
            }
            .to_string(),
        );
        content.insert("hostPort".to_string(), settings.host_port.to_string());
        content.insert(
            "remoteHostPort".to_string(),
            settings.remote_host_port.to_string(),
        );
        ControlDatagram {
            version: 1,
            r#type: "input_port".to_string(),
            content,
        }
    }

    pub fn client_data(
        id: &str,
        sequence: u32,
        settings: ClientDataSettings,
        base64: &str,
    ) -> ControlDatagram {
        let mut content = HashMap::new();
        content.insert("id".to_string(), id.to_string());
        content.insert("sequence".to_string(), sequence.to_string());
        content.insert(
            "protocol".to_string(),
            match settings.protocol {
                Protocol::Tcp => "tcp",
                Protocol::Udp => "udp",
            }
            .to_string(),
        );
        content.insert("hostPort".to_string(), settings.host_port.to_string());
        content.insert(
            "hostClientPort".to_string(),
            settings.host_client_port.to_string(),
        );
        content.insert("base64".to_string(), base64.to_string());
        ControlDatagram {
            version: 1,
            r#type: "client_data".to_string(),
            content,
        }
    }

    pub fn server_data(
        id: &str,
        sequence: u32,
        settings: ServerDataSettings,
        base64: &str,
    ) -> ControlDatagram {
        let mut content = HashMap::new();
        content.insert("id".to_string(), id.to_string());
        content.insert("sequence".to_string(), sequence.to_string());
        content.insert(
            "protocol".to_string(),
            match settings.protocol {
                Protocol::Tcp => "tcp",
                Protocol::Udp => "udp",
            }
            .to_string(),
        );
        content.insert(
            "remoteHostPort".to_string(),
            settings.remote_host_port.to_string(),
        );
        content.insert(
            "remoteHostClientPort".to_string(),
            settings.remote_host_client_port.to_string(),
        );
        content.insert("base64".to_string(), base64.to_string());
        ControlDatagram {
            version: 1,
            r#type: "server_data".to_string(),
            content,
        }
    }

    fn fragment(
        digest: String,
        content_type: String,
        index: i32,
        length: i32,
        data: String,
    ) -> ControlDatagram {
        let mut content = HashMap::new();
        content.insert("digest".to_string(), digest);
        content.insert("contentType".to_string(), content_type);
        content.insert("index".to_string(), index.to_string());
        content.insert("length".to_string(), length.to_string());
        content.insert("data".to_string(), data);
        ControlDatagram {
            version: 1,
            r#type: "fragment".to_string(),
            content,
        }
    }
    pub fn fragments(datagram: ControlDatagram) -> Result<[ControlDatagram; 2], serde_json::Error> {
        let control_datagram_json = serde_json::to_string(&datagram)?;
        let digest = digest(control_datagram_json.to_string());
        let part_a = control_datagram_json[0..control_datagram_json.len() / 2].to_string();
        let part_b = control_datagram_json
            [control_datagram_json.len() / 2..control_datagram_json.len()]
            .to_string();
        Ok([
            ControlDatagram::fragment(
                digest.to_string(),
                "control_datagram".to_string(),
                0,
                2,
                part_a,
            ),
            ControlDatagram::fragment(
                digest.to_string(),
                "control_datagram".to_string(),
                1,
                2,
                part_b,
            ),
        ])
    }
}
