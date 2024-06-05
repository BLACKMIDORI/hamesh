use crate::control_datagram::ControlDatagram;
use crate::get_ip_version::get_ip_version;
use crate::ip_version::IpVersion;
use crate::settings_models::{ClientDataSettings, PortSettings, Protocol};
use futures::TryFutureExt;
use log::{error, info};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use tokio::sync::Mutex;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub struct OutboundServer {
    port_settings: PortSettings,
}

impl OutboundServer {
    pub fn new(port_settings: PortSettings) -> OutboundServer {
        OutboundServer { port_settings }
    }

    pub async fn start(
        self,
        to_server_receiver: tokio::sync::mpsc::Receiver<ControlDatagram>,
        from_server_sender: tokio::sync::mpsc::Sender<ControlDatagram>,
        from_tunnel_ack_subscriber: tokio::sync::broadcast::Sender<String>,
    ) -> Result<(), String> {
        info!(
            "{}:{}/{}",
            self.port_settings.host_port,
            self.port_settings.remote_host_port,
            match self.port_settings.protocol {
                Protocol::Tcp => {
                    "tcp"
                }
                Protocol::Udp => {
                    "udp"
                }
            },
        );
        Self::start_socket(
            &self,
            to_server_receiver,
            from_server_sender,
            from_tunnel_ack_subscriber,
        )
        .await
        .map_err(|err| {
            error!("{}", err);
            err
        })?;
        Ok(())
    }

    async fn start_socket(
        &self,
        to_server_receiver: tokio::sync::mpsc::Receiver<ControlDatagram>,
        from_server_sender: tokio::sync::mpsc::Sender<ControlDatagram>,
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
        match self.port_settings.protocol {
            Protocol::Tcp => {
                let server = tokio::net::TcpListener::bind(address)
                    .await
                    .map_err(|e| format!("{:?}", e))?;
                info!(
                    "listening on {}/tcp",
                    server.local_addr().map_err(|e| format!("{:?}", e))?
                );
                Self::tcp_accept(
                    server,
                    to_server_receiver,
                    from_server_sender,
                    from_tunnel_ack_subscriber,
                )
                .await?;
            }
            Protocol::Udp => {
                let server = tokio::net::UdpSocket::bind(address)
                    .await
                    .map_err(|e| format!("{:?}", e))?;
                info!(
                    "listening on {}/udp",
                    server.local_addr().map_err(|e| format!("{:?}", e))?
                );
                Self::handle_udp(server, to_server_receiver, from_server_sender).await?;
            }
        };
        Ok(())
    }

    async fn tcp_accept(
        server: tokio::net::TcpListener,
        mut to_server_receiver: tokio::sync::mpsc::Receiver<ControlDatagram>,
        from_server_sender: tokio::sync::mpsc::Sender<ControlDatagram>,
        from_tunnel_ack_subscriber: tokio::sync::broadcast::Sender<String>,
    ) -> Result<(), String> {
        let (to_local_clients_sender, _) = tokio::sync::broadcast::channel(1024);
        // let mut to_local_client_senders: HashMap<u16,tokio::sync::mpsc::Sender<ControlDatagram>> = HashMap::new();
        _ = tokio::join!(
            async {
                loop {
                    let datagram = to_server_receiver
                        .recv()
                        .await
                        .ok_or("failed to_server_receiver.recv()")?;
                    to_local_clients_sender
                        .send(datagram)
                        .map_err(|e| format!("{:?}", e))?;
                }
                Ok::<(), String>(())
            }
            .map_err(|e| {
                error!("{}", e);
                e
            }),
            async {
                loop {
                    let server_address = server.local_addr().map_err(|e| format!("{:?}", e))?;
                    let host_port = server_address.port();
                    let (client_stream, client_address) =
                        server.accept().await.map_err(|e| format!("{:?}", e))?;
                    let local_client_port = client_address.port();
                    let (mut client_stream_reader, mut client_stream_writer) =
                        client_stream.into_split();

                    let from_server_sender_clone = from_server_sender.clone();
                    let mut to_local_clients_receiver = to_local_clients_sender.subscribe();
                    let from_tunnel_ack_subscriber_clone = from_tunnel_ack_subscriber.clone();
                    tokio::spawn(async move {
                        let stopped = AtomicBool::new(false);
                        _ = tokio::join!(
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
                                        info!(
                                            "disconnected from {}: {}",
                                            server_address, client_address
                                        );
                                        break;
                                    }
                                    let host_client_port = client_address.port();
                                    let id = format!(
                                        "tcp_client_{host_port}_{host_client_port}_{sequence}"
                                    );
                                    let encoded_data = base64::encode(&buff[..size]);
                                    buff.clear();
                                    let client_data_datagram = ControlDatagram::client_data(
                                        id.as_str(),
                                        sequence,
                                        ClientDataSettings {
                                            protocol: Protocol::Tcp,
                                            host_port,
                                            host_client_port,
                                        },
                                        encoded_data.as_str(),
                                    );
                                    let mut ack_receiver =
                                        from_tunnel_ack_subscriber_clone.subscribe();
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
                                        from_server_sender_clone
                                            .send(client_data_datagram.clone())
                                            .await
                                            .map_err(|e| format!("{:?}", e))?;
                                    }
                                    (sequence,_) = sequence.overflowing_add(1);
                                }
                                Ok::<(), String>(())
                            }
                            .map_err(|e| {
                                stopped.store(true, Ordering::Relaxed);
                                error!("OutboundServer.tcp_accept().join.0: {e}");
                                e
                            }),
                            async {
                                let mut next_sequence: u32 = 0;
                                loop {
                                    if stopped.load(Ordering::Relaxed) {
                                        break;
                                    }
                                    let datagram = to_local_clients_receiver
                                        .recv()
                                        .await
                                        .map_err(|e| format!("{:?}", e))?;
                                    // TODO: use simple channel for each client instead broadcast
                                    let server_remote_host_client_port = datagram.content.get("remoteHostClientPort").ok_or("invalid datagram.content.get(\"remoteHostClientPort\")")?.parse::<u16>().map_err(|e|format!("{e}"))?;
                                    if server_remote_host_client_port != local_client_port {
                                        continue;
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
                                error!("OutboundServer.tcp_accept().join.1: {e}");
                                e
                            }),
                        );
                    });
                }
                Ok::<(), String>(())
            }
            .map_err(|e| {
                error!("{}", e);
                e
            })
        );
        Ok(())
    }

    async fn handle_udp(
        server_socket: tokio::net::UdpSocket,
        mut to_server_receiver: tokio::sync::mpsc::Receiver<ControlDatagram>,
        from_server_sender: tokio::sync::mpsc::Sender<ControlDatagram>,
    ) -> Result<(), String> {
        let server_address = server_socket.local_addr().map_err(|e| format!("{:?}", e))?;
        let host_port = server_address.port();
        let mut buff = Vec::with_capacity(65535);
        let clients_mutex = Mutex::new(HashMap::new());
        _ = tokio::join!(
            async {
                loop {
                    let (size, local_client_socket_address) = server_socket
                        .recv_buf_from(&mut buff)
                        .await
                        .map_err(|e| format!("{e}"))?;
                    let host_client_port = local_client_socket_address.port();
                    let mut clients= clients_mutex.lock().await;
                    if !clients.contains_key(&host_client_port) {
                        clients.insert(host_client_port, (local_client_socket_address,AtomicU32::new(0),AtomicU32::new(0)));
                    }
                    info!("👀 read {}b", size);
                    if size == 0 {
                        clients.remove(&host_client_port);
                        info!("disconnected from {}: {}", server_address, host_client_port);
                        break;
                    }
                    let (_,sequence_atomic,_) = clients.get(&host_client_port).ok_or("invalid clients.get(&local_client_socket)")?;
                    let sequence = sequence_atomic.load(Ordering::Relaxed);
                    let id = format!("udp_client_{host_port}_{host_client_port}_{sequence}");
                    let encoded_data = base64::encode(&buff[..size]);
                    buff.clear();
                    let client_data_datagram = ControlDatagram::client_data(
                        id.as_str(),
                        sequence,
                        ClientDataSettings {
                            protocol: Protocol::Udp,
                            host_port,
                            host_client_port,
                        },
                        encoded_data.as_str(),
                    );
                    from_server_sender
                    .send(client_data_datagram.clone())
                    .await
                    .map_err(|e| format!("{:?}", e))?;
                    let (new_sequence, _) = sequence.overflowing_add(1);
                    sequence_atomic.store(new_sequence,Ordering::Relaxed);
                }
                Ok::<(),String>(())
            }.map_err(|e|{
                format!("OutboundServer.handle_up.join.0: {e}");
                e
            }),
            async {
                loop{
                    let datagram = to_server_receiver
                    .recv()
                    .await
                    .ok_or("invalid to_server_receiver.recv()")?;
                    // TODO: use simple channel for each client instead broadcast
                    let server_remote_host_client_port = datagram.content.get("remoteHostClientPort").ok_or("invalid datagram.content.get(\"remoteHostClientPort\")")?.parse::<u16>().map_err(|e|format!("{e}"))?;

                    let clients= clients_mutex.lock().await;
                    let found = clients.get(&server_remote_host_client_port);
                    let (local_client_socket_address,_, next_sequence_atomic) = match found {
                        None => {
                            continue;
                        }
                        Some(atomic) => {atomic}
                    };
                    let next_sequence = next_sequence_atomic.load(Ordering::Relaxed);
                    // let received_sequence = datagram.content.get("sequence").ok_or("invalid datagram.content.get(\"sequence\")")?.parse::<u32>().map_err(|e|format!("{e}"))?;
                    // sequence doesn't matter for udp
                    // and there is no resend mechanism in the other side
                    // if received_sequence != next_sequence {
                    //     continue;
                    // }
                    let data_base64 = &datagram.content["base64"];
                    let data = base64::decode(data_base64).map_err(|e|format!("{e}"))?;
                    server_socket
                    .send_to(&data, local_client_socket_address)
                    .await
                    .map_err(|e| format!("{:?}", e))?;
                    info!("📝 wrote {}b (sequence = {next_sequence})",data.len());
                    let (new_next_sequence, _) = next_sequence.overflowing_add(1);
                    next_sequence_atomic.store(new_next_sequence,Ordering::Relaxed);
                }
                Ok::<(),String>(())
            }
            .map_err(|e|{
                format!("OutboundServer.handle_up.join.0: {e}");
                e
            })
        );
        Ok::<(),String>(())
    }
}
