use crate::control_datagram::ControlDatagram;
use crate::ip_version::IpVersion;
use crate::settings_models::{PortSettings, Protocol, ServerDataSettings};
use futures::TryFutureExt;
use log::{error, info};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};

pub struct InboundClient {
    ip_version: IpVersion,
    port_settings: PortSettings,
    remote_client_port: u16,
}

impl InboundClient {
    pub fn new(ip_version:IpVersion,port_settings: PortSettings, remote_client_port: u16) -> InboundClient {
        InboundClient {
            ip_version,
            port_settings,
            remote_client_port,
        }
    }
    pub async fn start(
        self,
        mut to_inbound_client_receiver: tokio::sync::mpsc::Receiver<ControlDatagram>,
        from_inbound_client_sender: tokio::sync::mpsc::Sender<ControlDatagram>,
        from_tunnel_ack_subscriber: tokio::sync::broadcast::Sender<String>,
    ) -> Result<(), String> {
        let local_server_address = match self.ip_version {
            IpVersion::Ipv4 => SocketAddr::new(
                IpAddr::from(Ipv4Addr::LOCALHOST),
                self.port_settings.host_port,
            ),
            IpVersion::Ipv6 => SocketAddr::new(
                IpAddr::from(Ipv6Addr::LOCALHOST),
                self.port_settings.host_port,
            ),
        };
        let host_port = self.port_settings.host_port;
        let remote_host_port = self.port_settings.remote_host_port;
        let remote_host_client_port = self.remote_client_port;

        match self.port_settings.protocol {
            Protocol::Tcp => {
                let client = TcpStream::connect(local_server_address)
                    .await
                    .map_err(|e| format!("{e}"))?;
                info!(
                    "new client at {host_port} {}/tcp",
                    client.local_addr().map_err(|e| format!("{e}"))?
                );
                let client_address = client.local_addr().unwrap();
                let host_client_port = client_address.port();
                let (mut client_stream_reader, mut client_stream_writer) = client.into_split();

                let stopped = AtomicBool::new(false);
                _ = tokio::join!(
                    async {
                        let mut next_sequence: u32 = 0;
                        loop {
                            if stopped.load(Ordering::Relaxed) {
                                break;
                            }
                            let datagram = to_inbound_client_receiver
                                .recv()
                                .await
                                .ok_or("failed  to_inbound_client_receiver.recv()")?;
                            if datagram.r#type == "close" {
                                stopped.store(true, Ordering::Relaxed);
                                break;
                            }
                            let received_sequence = datagram.content.get("sequence").ok_or("invalid datagram.content.get(\"sequence\")")?.parse::<u32>().map_err(|e|format!("{e}"))?;
                            if received_sequence != next_sequence {
                                continue;
                            }
                            let data_base64 = &datagram.content["base64"];
                            let data = base64::decode(data_base64).map_err(|e|format!("{e}"))?;
                            client_stream_writer
                                .write_all(&data)
                                .await
                                .map_err(|e| format!("{e}"))?;
                            info!("📝 wrote {}b (sequence = {next_sequence})",data.len());
                            (next_sequence,_) = next_sequence.overflowing_add(1);
                        }
                        Ok::<(), String>(())
                    }
                    .map_err(|e| {
                        stopped.store(true, Ordering::Relaxed);
                        error!("InboundClient writer: {}", e);
                        e
                    }),
                    async {
                        let mut sequence: u32 = 0;
                        let mut buff = Vec::with_capacity(65535);
                        loop {
                            if stopped.load(Ordering::Relaxed) {
                                break;
                            }
                            let size = client_stream_reader
                                .read_buf(&mut buff)
                                .await
                                .map_err(|e| format!("{e}"))?;
                            info!("👀 read {}b",size);
                            if size == 0 {
                                stopped.store(true, Ordering::Relaxed);
                                break;
                            }
                            let id =
                                format!("tcp_server_{host_port}_{host_client_port}_{remote_host_port}_{remote_host_client_port}_{sequence}");
                            let encoded_data = base64::encode(&buff[..size]);
                            buff.clear();
                            let server_data_datagram = ControlDatagram::server_data(
                                id.as_str(),
                                sequence,
                                ServerDataSettings {
                                    protocol: Protocol::Tcp,
                                    remote_host_port,
                                    remote_host_client_port,
                                },
                                encoded_data.as_str(),
                            );

                            let mut ack_receiver = from_tunnel_ack_subscriber.subscribe();
                            let ack_received = tokio::spawn(
                                async move {
                                    loop {
                                        let ack = ack_receiver
                                        .recv()
                                        .await
                                        .map_err(|e| format!("{e}"))?;
                                        if ack == id {
                                            break;
                                        }
                                    }
                                    Ok::<(), String>(())
                                }
                                .map_err(|e| {
                                    error!("{}", e);
                                    e
                                }),
                            );
                            let mut interval =
                            tokio::time::interval(Duration::from_millis(50));
                            loop {
                                // The first tick completes immediately
                                interval.tick().await;
                                if ack_received.is_finished() {
                                    break;
                                }
                                from_inbound_client_sender
                                .send(server_data_datagram.clone())
                                .await
                                .map_err(|e| format!("{e}"))?;
                            }
                            (sequence,_) = sequence.overflowing_add(1);
                        }
                        Ok::<(), String>(())
                    }
                    .map_err(|e| {
                        stopped.store(true, Ordering::Relaxed);
                        error!("InboundClient reader: {}", e);
                        e
                    })
                );
            }
            Protocol::Udp => {
                let local_client_address =
                    match self.ip_version {
                        IpVersion::Ipv4 => SocketAddr::new(IpAddr::from(Ipv4Addr::LOCALHOST), 0),
                        IpVersion::Ipv6 => SocketAddr::new(IpAddr::from(Ipv6Addr::LOCALHOST), 0),
                    };
                let local_server_client_socket = UdpSocket::bind(local_client_address)
                    .await
                    .map_err(|e| format!("{e}"))?;
                local_server_client_socket
                    .connect(local_server_address)
                    .await
                    .map_err(|e| format!("{e}"))?;
                let local_client_address = local_server_client_socket
                    .local_addr()
                    .map_err(|e| format!("{e}"))?;
                info!("new client at {host_port} {}/udp", local_client_address);
                let host_client_port = local_client_address.port();
                let stopped = AtomicBool::new(false);
                _ = tokio::join!(
                    async {
                        let mut next_sequence: u32 = 0;
                        loop {
                            if stopped.load(Ordering::Relaxed) {
                                break;
                            }
                            let datagram = to_inbound_client_receiver
                                .recv()
                                .await
                                .ok_or("failed to_inbound_client_receiver.recv()")?;
                            if datagram.r#type == "close" {
                                stopped.store(true, Ordering::Relaxed);
                                break;
                            }
                            // let received_sequence = datagram.content.get("sequence").ok_or("invalid datagram.content.get(\"sequence\")")?.parse::<u32>().map_err(|e|format!("{e}"))?;
                            // sequence doesn't matter for udp
                            // and there is no resend mechanism in the other side
                            // if received_sequence != next_sequence {
                            //     continue;
                            // }
                            let data_base64 = &datagram.content["base64"];
                            let data = base64::decode(data_base64).map_err(|e|format!("{e}"))?;
                            local_server_client_socket
                                .send(&data)
                                .await
                                .map_err(|e| format!("{e}"))?;
                            info!("📝 wrote {}b (sequence = {next_sequence})",data.len());
                            (next_sequence,_) = next_sequence.overflowing_add(1);
                        }
                        Ok::<(), String>(())
                    }
                    .map_err(|e| {
                        stopped.store(true, Ordering::Relaxed);
                        error!("InboundClient writer: {}", e);
                        e
                    }),
                    async {
                        let mut sequence: u32 = 0;
                        let mut buff = Vec::with_capacity(65535);
                        loop {
                            if stopped.load(Ordering::Relaxed) {
                                break;
                            }
                            let size = local_server_client_socket
                                .recv_buf(&mut buff)
                                .await
                                .map_err(|e| format!("{e}"))?;
                            info!("👀 read {}b",size);
                            if size == 0 {
                                stopped.store(true, Ordering::Relaxed);
                                break;
                            }
                            let id =
                                format!("udp_server_{host_port}_{host_client_port}_{remote_host_port}_{remote_host_client_port}_{sequence}");
                            let encoded_data = base64::encode(&buff[..size]);
                            buff.clear();
                            let server_data_datagram = ControlDatagram::server_data(
                                id.as_str(),
                                sequence,
                                ServerDataSettings {
                                    protocol: Protocol::Udp,
                                    remote_host_port,
                                    remote_host_client_port,
                                },
                                encoded_data.as_str(),
                            );

                            from_inbound_client_sender
                            .send(server_data_datagram.clone())
                            .await
                            .map_err(|e| format!("{e}"))?;

                            (sequence,_) = sequence.overflowing_add(1);
                        }
                        Ok::<(), String>(())
                    }
                    .map_err(|e| {
                        stopped.store(true, Ordering::Relaxed);
                        error!("InboundClient reader: {}", e);
                        e
                    })
                );
            }
        };
        Ok::<(), String>(())
    }
}
