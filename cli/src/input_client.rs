
use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};
use std::sync::Arc;
use std::time::Duration;
use log::{error, info};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::broadcast::Sender;
use tokio::sync::Mutex;
use tokio::time;
use crate::control_datagram::ControlDatagram;
use crate::get_ip_version::get_ip_version;
use crate::ip_version::IpVersion;
use crate::settings_models::{ClientDataSettings, PortSettings, Protocol, ServerDataSettings};

#[derive(Clone, Debug)]
pub struct InputClient {
    pub settings: ClientDataSettings,
    pub from_outside_tx: Sender<ControlDatagram>,
    pub is_closed: Arc<Mutex<bool>>,
}


impl InputClient {
    pub fn new(settings: ClientDataSettings, from_outside_tx: Sender<ControlDatagram>) -> InputClient {
        InputClient { settings, from_outside_tx, is_closed: Arc::new(Mutex::new(false))}
    }
    pub async fn start(self, input_settings: PortSettings, tunnel_sender: Sender<ControlDatagram>) {
        let server_port = input_settings.host_port;
        let address;
        match get_ip_version().unwrap() {
            IpVersion::Ipv4 => {
                address = SocketAddr::V4(format!("127.0.0.1:{server_port}").parse::<SocketAddrV4>().unwrap());
                // tcp_listener_result = tokio::net::TcpListener::bind(address).await;
            }
            IpVersion::Ipv6 => {
                address = SocketAddr::V6(format!("[::1]:{server_port}").parse::<SocketAddrV6>().unwrap());
                // tcp_listener_result = tokio::net::TcpListener::bind(address).await;
            }
        }

        match self.settings.protocol{
            Protocol::Tcp => {
                match TcpStream::connect(address).await {
                    Ok(stream) => {
                        let client_address = stream.local_addr().unwrap();
                        let client_port = client_address.port();
                        let (pre_tunnel_sender, _) = tokio::sync::broadcast::channel(1024);
                        let (ack_sender, _) = tokio::sync::broadcast::channel(1024);
                        let (mut read_stream, mut write_stream) = stream.into_split();
                        let mut receiver = self.from_outside_tx.subscribe();
                        tokio::spawn(self.clone().handle_retry( tunnel_sender, pre_tunnel_sender.clone(), ack_sender.clone()));
                        let reader_handler = tokio::spawn(async move {
                            let mut sequence: u64 = 0;
                            let mut received_zero_bytes = false;
                            loop {
                                let id = format!("tcp_server_{server_port}_{client_port}_{sequence}");
                                let mut buff = [0; 10240];
                                match read_stream.read(&mut buff).await {
                                    Ok(size) => {
                                        if size == 0 {
                                            if !received_zero_bytes {
                                                received_zero_bytes = true;
                                                continue;
                                            } else {
                                                break;
                                            }
                                        }
                                        received_zero_bytes = false;
                                        let encoded_data = base64::encode(&buff[..size]);
                                        let datagram = ControlDatagram::server_data(
                                            id.as_str(),
                                            sequence,
                                            ServerDataSettings {
                                                protocol: Protocol::Tcp,
                                                remote_host_port: self.settings.host_port,
                                                remote_host_client_port: self.settings.host_client_port,
                                            },
                                            encoded_data.as_str(),
                                        );
                                        match pre_tunnel_sender.send(datagram.clone()) {
                                            Ok(_) => {}
                                            Err(error) => {
                                                error!("could not send local client data: {}", error);
                                            }
                                        }
                                        sequence += 1;
                                    }
                                    Err(error) => {
                                        error!("could not read local server data({address}): {}", error);
                                        break;
                                    }
                                }
                            }
                        });
                        let support_sender = self.from_outside_tx.clone();
                        tokio::spawn(async move{
                            let _ = reader_handler.await;
                            // Gracefully free the write thread and close it
                            support_sender.send(ControlDatagram::client_data(
                                "close",
                                0,
                                ClientDataSettings {
                                    protocol: Protocol::Tcp,
                                    host_port: 0,
                                    host_client_port: 0,
                                },
                                "",
                            )).expect("failed to signal the writer to close");
                        });
                        tokio::spawn(async  move{
                            loop {
                                match receiver.recv().await {
                                    Ok(datagram) => {
                                        match datagram.r#type.as_str() {
                                            "client_data"=>{
                                                let data_base64 = datagram.content["base64"].as_str();
                                                let data_result = base64::decode(data_base64);
                                                match data_result{
                                                    Ok(data) => {
                                                        if data.is_empty() {
                                                            let id = datagram.content["id"].as_str();
                                                            if id == "close" {
                                                                break;
                                                            }
                                                        }
                                                        match write_stream.write(&data).await {
                                                            Ok(_) => {}
                                                            Err(error) => {
                                                                error!("could not receive the data for local server on its client: {error}")
                                                            }
                                                        }
                                                    }
                                                    Err(error) => {
                                                        error!("could not decode remote client data: {error}")
                                                    }
                                                }
                                            }
                                            "ACK"=>{
                                                match ack_sender.send(datagram) {
                                                    Ok(_) => {}
                                                    Err(error) => {
                                                        error!("error to handle ACK: {error}");
                                                    }
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                    Err(error) => {
                                        error!("could not receive data from clients channel: {error}");
                                    }
                                }
                            }
                            info!("closed {}/tcp",client_address );
                            let mut is_close_mutex = self.is_closed.lock().await;
                            *is_close_mutex = true;
                        });
                    }
                    Err(error) => {
                        error!("Failed to connect to local server({address}): {error}");
                    }
                }
            }
            Protocol::Udp => {
                // TODO: implement UDP
                let client_address;
                match get_ip_version().unwrap() {
                    IpVersion::Ipv4 => {
                        client_address = SocketAddr::V4("0.0.0.0:0".parse::<SocketAddrV4>().unwrap());
                        // tcp_listener_result = tokio::net::TcpListener::bind(address).await;
                    }
                    IpVersion::Ipv6 => {
                        client_address = SocketAddr::V6("[::]:0".parse::<SocketAddrV6>().unwrap());
                        // tcp_listener_result = tokio::net::TcpListener::bind(address).await;
                    }
                }
                let client_socket_result = UdpSocket::bind(client_address).await;
                match client_socket_result {
                    Ok(client_socket) => {
                        match client_socket.connect(address).await{
                            Ok(_) => {
                                let _client_address = client_socket.local_addr().unwrap();
                                // let client_port = client_address.port();
                                let mut buffer = [0; 10240];
                                loop {
                                    match client_socket.recv_from(&mut buffer).await {
                                        Ok((size, _local_client_address)) => {
                                            info!("UDP parser not implemented yet, size = {}",size)
                                        }
                                        Err(error) => {
                                            error!("could not read local server data({address}): {}", error);
                                            break;
                                        }
                                    }
                                }
                            }
                            Err(error) => {
                                error!("Failed to connect to local server({address}): {error}");
                            }
                        }
                    }
                    Err(error) => {
                        error!("could not start a udp client socket: {}",error)
                    }
                }
            }
        }
    }


    // TODO: improve memory usage
    async fn handle_retry(self, tunnel_sender: Sender<ControlDatagram>, pre_tunnel_sender: Sender<ControlDatagram>, ack_sender: Sender<ControlDatagram>) {
        let mut pre_tunnel_receiver = pre_tunnel_sender.subscribe();
        loop {
            // Send 10 datagrams a time
            for _ in 0..10 {
                match pre_tunnel_receiver.recv().await {
                    Ok(datagram) => {
                        let datagram_id_option = datagram.content.get("id").cloned();
                        match datagram_id_option {
                            Some(datagram_id) => {
                                let received_ack1 = Arc::new(Mutex::new(false));
                                let received_ack2 = received_ack1.clone();
                                let tunnel_sender_clone = tunnel_sender.clone();
                                let is_closed = self.is_closed.clone();
                                let mut ack_receiver = ack_sender.subscribe();
                                let ack_handler = tokio::spawn(async move {
                                    loop{
                                        match ack_receiver.recv().await{
                                            Ok(datagram) => {
                                                match datagram.r#type.as_str(){
                                                    "ACK"=>{
                                                        let id = datagram.content["id"].clone();
                                                        if id == datagram_id{
                                                            let mut received_ack_mutex = received_ack1.lock().await;
                                                            *received_ack_mutex = true;
                                                        }
                                                    }
                                                    _=>{
                                                        error!("datagram is not ACK: {}", datagram.r#type);
                                                    }
                                                }
                                            }
                                            Err(error) => {
                                                error!("[recv]could not receive ACK for retry handling: {}", error);
                                            }
                                        }
                                    }
                                });
                                tokio::spawn(async move {
                                    // TODO: use the max latency as interval
                                    let mut interval = time::interval(Duration::from_millis(20));

                                    const MAX_RETRIES: i32 = 500;
                                    for _ in 0..MAX_RETRIES {
                                        match tunnel_sender_clone.send(datagram.clone()) {
                                            Ok(_) => {}
                                            Err(error) => {
                                                error!("[send]could not send local client data to tunnel: {}", error);
                                            }
                                        }
                                        interval.tick().await;
                                        if *received_ack2.lock().await || *is_closed.lock().await {
                                            break;
                                        }
                                    }
                                    ack_handler.abort();
                                });
                            }
                            None => {
                                error! {"cannot handle retry for datagram without id"}
                                let _ = tunnel_sender.clone().send(datagram.clone());
                            }
                        }
                    }
                    Err(error) => {
                        error!("[recv]could not send local client data to tunnel: {}", error);
                    }
                }
            }
        }
    }
}
