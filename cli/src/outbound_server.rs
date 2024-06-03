use crate::control_datagram::ControlDatagram;
use crate::get_ip_version::get_ip_version;
use crate::ip_version::IpVersion;
use crate::settings_models::{ClientDataSettings, PortSettings, Protocol};
use futures::TryFutureExt;
use log::{error, info};
use std::collections::HashMap;
use std::error;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpSocket, UdpSocket};
use tracing_subscriber::fmt::format;

pub struct OutboundServer {
    local_host_port: u16,
    pub remote_host_port: u16,
    pub remote_protocol: Protocol,
    pub to_server_thread_sender: tokio::sync::mpsc::Sender<ControlDatagram>,
}

impl OutboundServer {
    pub fn new(
        local_host_port: u16,
        remote_host_port: u16,
        remote_protocol: Protocol,
        to_server_thread_sender: tokio::sync::mpsc::Sender<ControlDatagram>,
    ) -> OutboundServer {
        OutboundServer {
            local_host_port,
            remote_host_port,
            remote_protocol,
            to_server_thread_sender,
        }
    }

    pub async fn start(
        port_settings: PortSettings,
        to_server_receiver: tokio::sync::mpsc::Receiver<ControlDatagram>,
        from_server_sender: tokio::sync::mpsc::Sender<ControlDatagram>,
        from_tunnel_ack_subscriber: tokio::sync::broadcast::Sender<String>,
    ) -> Result<(), String> {
        info!(
            "{}:{}/{}",
            port_settings.host_port,
            port_settings.remote_host_port,
            match port_settings.protocol {
                Protocol::Tcp => {
                    "tcp"
                }
                Protocol::Udp => {
                    "udp"
                }
            },
        );
        // hold at least one receiver in order to not close the channel
        let mut from_tunnel_ack_receiver = from_tunnel_ack_subscriber.subscribe();
        tokio::spawn(async move {
            loop {
                from_tunnel_ack_receiver
                    .recv()
                    .await
                    .map_err(|e| format!("{e}"))?;
            }
            Ok::<(), String>(())
        });
        Self::start_socket(
            port_settings,
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
        port_settings: PortSettings,
        to_server_receiver: tokio::sync::mpsc::Receiver<ControlDatagram>,
        from_server_sender: tokio::sync::mpsc::Sender<ControlDatagram>,
        from_tunnel_ack_subscriber: tokio::sync::broadcast::Sender<String>,
    ) -> Result<(), String> {
        let address = match get_ip_version().ok_or("failed get_ip_version()")? {
            IpVersion::Ipv4 => {
                SocketAddr::new(IpAddr::from(Ipv4Addr::LOCALHOST), port_settings.host_port)
            }
            IpVersion::Ipv6 => {
                SocketAddr::new(IpAddr::from(Ipv6Addr::LOCALHOST), port_settings.host_port)
            }
        };
        match port_settings.protocol {
            Protocol::Tcp => {
                let server = TcpListener::bind(address)
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
                let server = UdpSocket::bind(address)
                    .await
                    .map_err(|e| format!("{:?}", e))?;
                info!(
                    "listening on {}/udp",
                    server.local_addr().map_err(|e| format!("{:?}", e))?
                );
            }
        };
        Ok(())
    }

    async fn tcp_accept(
        server: TcpListener,
        mut to_server_receiver: tokio::sync::mpsc::Receiver<ControlDatagram>,
        from_server_sender: tokio::sync::mpsc::Sender<ControlDatagram>,
        mut from_tunnel_ack_subscriber: tokio::sync::broadcast::Sender<String>,
    ) -> Result<(), String> {
        let (to_local_clients_sender, _) = tokio::sync::broadcast::channel(1024);
        // let mut to_local_client_senders: HashMap<u16,tokio::sync::mpsc::Sender<ControlDatagram>> = HashMap::new();
        tokio::join!(
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
                    let (mut client_stream_reader, mut client_stream_writer) =
                        client_stream.into_split();

                    let from_server_sender_clone = from_server_sender.clone();
                    let mut to_local_clients_receiver = to_local_clients_sender.subscribe();
                    let from_tunnel_ack_subscriber_clone = from_tunnel_ack_subscriber.clone();
                    tokio::spawn(async move {
                        let mut stopped = AtomicBool::new(false);

                        tokio::join!(
                            async {
                                let mut sequence: u64 = 0;
                                let mut buff = Vec::with_capacity(65535);
                                loop {
                                    let size = client_stream_reader
                                        .read_buf(&mut buff)
                                        .await
                                        .map_err(|e| format!("{:?}", e))?;
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
                                    // The first tick completes immediately
                                    interval.tick().await;
                                    loop {
                                        if ack_received.is_finished() {
                                            break;
                                        }
                                        from_server_sender_clone
                                            .send(client_data_datagram.clone())
                                            .await
                                            .map_err(|e| format!("{:?}", e))?;
                                        interval.tick().await;
                                    }
                                    sequence += 1;
                                }
                                Ok::<(), String>(())
                            }
                            .map_err(|e| {
                                error!("{}", e);
                                e
                            }),
                            async {
                                loop {
                                    if stopped.load(Ordering::Relaxed) {
                                        break;
                                    }
                                    // TODO: use simple channel for each client instead broadcast
                                    let datagram = to_local_clients_receiver
                                        .recv()
                                        .await
                                        .map_err(|e| format!("{:?}", e))?;
                                    client_stream_writer
                                        .write_all(
                                            &*datagram.to_vec().map_err(|e| format!("{:?}", e))?,
                                        )
                                        .await
                                        .map_err(|e| format!("{:?}", e))?
                                }
                                Ok::<(), String>(())
                            }
                            .map_err(|e| {
                                error!("{}", e);
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
}
