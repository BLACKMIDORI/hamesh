use crate::settings_models::{ClientDataSettings, PortSettings, Protocol, ServerDataSettings};
use serde::{Deserialize, Serialize};
use sha256::digest;
use std::collections::HashMap;
use std::error::Error;
use std::string::ToString;

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ControlDatagram {
    pub version: i32,
    pub r#type: String,
    pub content: HashMap<String, String>,
}

impl ControlDatagram {
    pub fn from(size: usize, buffer: &[u8]) -> Result<ControlDatagram, String> {
        let json_str = std::str::from_utf8(&buffer[..size]).map_err(|e|format!("{e}"))?;
        let obj = serde_json::from_str::<ControlDatagram>(json_str).map_err(|e|format!("{e}"))?;
        Ok(obj)
    }

    pub fn to_vec(&self) -> Result<Vec<u8>, String> {
        Ok(serde_json::to_vec(self).map_err(|e|format!("{e}"))?)
    }
    pub fn syn() -> ControlDatagram {
        ControlDatagram {
            version: 1,
            r#type: "SYN".to_string(),
            content: HashMap::new(),
        }
    }

    pub fn heart_beat() -> ControlDatagram {
        ControlDatagram {
            version: 1,
            r#type: "heart_beat".to_string(),
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
        index: u16,
        length: u16,
        data: String,
    ) -> ControlDatagram {
        let mut content = HashMap::new();
        content.insert("digest".to_string(), digest);
        content.insert("contentType".to_string(), content_type);
        content.insert("index".to_string(), pad_number(index, 5));
        content.insert("length".to_string(), pad_number(length, 5));
        content.insert("data".to_string(), data);
        ControlDatagram {
            version: 1,
            r#type: "fragment".to_string(),
            content,
        }
    }

    pub fn fragments(
        datagram: ControlDatagram,
        datagram_max_size: u16,
    ) -> Result<Vec<ControlDatagram>, serde_json::Error> {
        let empty_datagram_size: u16 = ControlDatagram::fragment(
            digest(""),
            "control_datagram".to_string(),
            0,
            0,
            "".to_string(),
        )
        .to_vec()
        .unwrap()
        .len()
        .try_into()
        .unwrap();
        let data_max_size = datagram_max_size.checked_sub(empty_datagram_size).unwrap() as usize;
        let mut control_datagram_json = serde_json::to_string(&datagram)?
            // add workaround
            .replace("\"", "\0\"");
        let digest = digest(control_datagram_json.to_string());
        let mut offset = 0;
        let mut parts = Vec::new();
        while offset < control_datagram_json.len() {
            if control_datagram_json.len() - offset >= data_max_size {
                parts.push(&control_datagram_json[offset..offset + data_max_size]);
                offset += data_max_size;
            } else {
                parts.push(&control_datagram_json[offset..]);
                offset += data_max_size;
            }
        }
        // panic!("{}+{} = {}",empty_datagram_size,parts[0].len(),empty_datagram_size+parts[0].len() as u16);
        let datagrams_length = parts.len();
        let mut datagrams = Vec::new();
        for (index, part) in parts.iter().enumerate() {
            datagrams.push(ControlDatagram::fragment(
                digest.to_string(),
                "control_datagram".to_string(),
                index as u16,
                datagrams_length as u16,
                // remove workaround
                part.replace("\0", ""),
            ));
        }
        Ok(datagrams)
    }
}

fn pad_number(number: u16, desired_length: usize) -> String {
    format!("{:0>width$}", number, width = desired_length)
}
