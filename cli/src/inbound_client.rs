use crate::control_datagram::ControlDatagram;
use crate::get_ip_version::get_ip_version;
use crate::ip_version::IpVersion;
use crate::settings_models::{PortSettings, Protocol, ServerDataSettings};
use futures::TryFutureExt;
use log::{error, info, warn};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpSocket, TcpStream, UdpSocket};

pub struct InboundClient {
    port_settings: PortSettings,
    remote_client_port: u16,
}

impl InboundClient {
    pub fn new(port_settings: PortSettings, remote_client_port: u16) -> InboundClient {
        InboundClient {
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
        let address = match get_ip_version().ok_or("failed get_ip_version()")? {
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
                let client = TcpStream::connect(address)
                    .await
                    .map_err(|e| format!("{:?}", e))?;
                info!("new client at {host_port} {}/tcp",client.local_addr().map_err(|e|format!("{e}"))?);
                let client_address = client.local_addr().unwrap();
                let host_client_port = client_address.port();
                let (mut client_stream_reader, mut client_stream_writer) = client.into_split();

                let mut stopped = AtomicBool::new(false);
                tokio::join!(
                    async {
                        let mut next_sequence: u32 = 0;
                        loop {
                            if stopped.load(Ordering::Relaxed) {
                                break;
                            }
                            let datagram = to_inbound_client_receiver
                                .recv()
                                .await
                                .ok_or("failed to_server_receiver.recv()")?;
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
                                .map_err(|e| format!("{:?}", e))?;
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
                                .map_err(|e| format!("{:?}", e))?;
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
                                sequence as u32,
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
                                        .map_err(|e| format!("{:?}", e))?;
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
                                .map_err(|e| format!("{:?}", e))?;
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
                let server = UdpSocket::bind(address)
                    .await
                    .map_err(|e| format!("{:?}", e))?;
                warn!("UDP unimplemented")
            }
        };
        Ok::<(), String>(())
    }
}
