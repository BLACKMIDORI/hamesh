use std::net::{ SocketAddr};
use std::sync::{Arc};
use std::time::Duration;
use log::{error, info, warn};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::broadcast::Sender;
use tokio::sync::mpsc::channel;
use tokio::sync::Mutex;
use tokio::time;
use crate::control_datagram::ControlDatagram;
use crate::settings_models::{ClientDataSettings, Protocol, ServerDataSettings};

#[derive(Clone)]
pub enum OutputServer {
    Tcp(OutputServerConfigTcp),
    Udp(OutputServerConfigUdp),
}


#[derive(Clone)]
pub struct OutputServerConfigTcp {
    reader_sender: tokio::sync::broadcast::Sender<ControlDatagram>,
    writer_sender: tokio::sync::broadcast::Sender<ControlDatagram>,
    address: SocketAddr,
}

impl OutputServerConfigTcp {
    pub fn new(reader_sender: tokio::sync::broadcast::Sender<ControlDatagram>, writer_sender: tokio::sync::broadcast::Sender<ControlDatagram>, address: SocketAddr) -> OutputServerConfigTcp {
        OutputServerConfigTcp { reader_sender, writer_sender, address }
    }
}

#[derive(Clone)]
pub struct OutputServerConfigUdp {
    reader_sender: tokio::sync::broadcast::Sender<ControlDatagram>,
    writer_sender: tokio::sync::broadcast::Sender<ControlDatagram>,
    address: SocketAddr,
}

impl OutputServerConfigUdp {
    pub fn new(reader_sender: tokio::sync::broadcast::Sender<ControlDatagram>, writer_sender: tokio::sync::broadcast::Sender<ControlDatagram>, address: SocketAddr) -> OutputServerConfigUdp {
        OutputServerConfigUdp { reader_sender, writer_sender, address }
    }
}

impl OutputServer {
    pub async fn start(self) {
        let self1 = self.clone();
        let self2 = self.clone();
        let (sender, _) = tokio::sync::broadcast::channel(1024);
        tokio::spawn(self1.get_socket_broadcast(sender.clone()));
        tokio::spawn(self2.handle_from_remote_data(sender.clone()));
    }

    async fn handle_from_remote_data(self, broadcast_sender: Sender<ControlDatagram>) {
        let mut receiver;
        match self {
            OutputServer::Tcp(server) => {
                receiver = server.reader_sender.subscribe();
            }
            OutputServer::Udp(server) => {
                receiver = server.reader_sender.subscribe();
            }
        }
        loop {
            match receiver.recv().await {
                Ok(datagram) => {
                    match datagram.r#type.as_str() {
                        "server_data" => {
                            match broadcast_sender.send(datagram) {
                                Ok(_) => {}
                                Err(error) => {
                                    error!("could not send to local client the received datagram(server_data): {}",error)
                                }
                            };
                        }
                        "ACK" => {
                            match broadcast_sender.send(datagram) {
                                Ok(_) => {}
                                Err(error) => {
                                    error!("could not send to local client the received datagram(ACK): {}",error)
                                }
                            };
                        }
                        _ => {}
                    }
                }
                Err(error) => {
                    error!("error receiving datagram for handling: {}",error)
                }
            }
        }
    }


    async fn get_socket_broadcast(self, broadcast_sender: Sender<ControlDatagram>) {
        match self {
            OutputServer::Tcp(server) => {
                let server_socket_result = TcpListener::bind(server.address).await;
                match server_socket_result {
                    Ok(server_socket) => {
                        info!("listening on {}/tcp",server_socket.local_addr().unwrap());
                        let host_port = server_socket.local_addr().unwrap().port();
                        let tunnel_sender_to_clone = server.writer_sender.clone();
                        loop {
                            let mut receiver = broadcast_sender.subscribe();
                            let result = server_socket.accept().await;
                            match result {
                                Ok((tcp_stream, local_client_address)) => {
                                    let (mut read_stream, mut write_stream) = tcp_stream.into_split();
                                    let host_client_port = local_client_address.port();
                                    let tunnel_sender = tunnel_sender_to_clone.clone();
                                    let (pre_tunnel_sender, _) = tokio::sync::broadcast::channel(1024);
                                    let (ack_sender, _) = tokio::sync::broadcast::channel(1024);
                                    let is_closed1 = Arc::new(Mutex::new(false));
                                    let is_closed2 = is_closed1.clone();
                                    tokio::spawn(handle_retry(is_closed1, tunnel_sender.clone(), pre_tunnel_sender.clone(), ack_sender.clone()));
                                    let reader_handler = tokio::spawn(async move {
                                        let mut sequence: u64 = 0;
                                        let mut received_zero_bytes = false;
                                        loop {
                                            let id = format!("tcp_client_{host_port}_{host_client_port}_{sequence}");
                                            let mut buff = Vec::with_capacity(65535);
                                            match read_stream.read_buf(&mut buff).await {
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
                                                    let datagram = ControlDatagram::client_data(
                                                        id.as_str(),
                                                        sequence,
                                                        ClientDataSettings {
                                                            protocol: Protocol::Tcp,
                                                            host_port,
                                                            host_client_port,
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
                                                    error!("could not read local client data: {}", error);
                                                    break;
                                                }
                                            };
                                        }
                                    });
                                    let support_sender = broadcast_sender.clone();
                                    tokio::spawn(async move {
                                        let _ = reader_handler.await;
                                        // Gracefully free the write thread and close it
                                        support_sender.send(ControlDatagram::server_data(
                                            "close",
                                            0,
                                            ServerDataSettings {
                                                protocol: Protocol::Tcp,
                                                remote_host_port: host_port,
                                                remote_host_client_port: host_client_port,
                                            },
                                            "",
                                        )).expect("failed to signal the writer to close");
                                    });
                                    tokio::spawn(async move {
                                        let mut sequence = 0;
                                        loop {
                                            match receiver.recv().await {
                                                Ok(datagram) => {
                                                    match datagram.r#type.as_str() {
                                                        "server_data" => {
                                                            let protocol_str = datagram.content["protocol"].as_str();
                                                            let remote_host_port_str = datagram.content["remoteHostPort"].as_str();
                                                            let remote_host_client_port_str = datagram.content["remoteHostClientPort"].as_str();
                                                            let server_data_settings = ServerDataSettings {
                                                                protocol: match protocol_str {
                                                                    "tcp" => Protocol::Tcp,
                                                                    "udp" => Protocol::Udp,
                                                                    _ => {
                                                                        error!("invalid protocol {}", protocol_str);
                                                                        continue;
                                                                    }
                                                                },
                                                                remote_host_port: remote_host_port_str.parse().unwrap(),
                                                                remote_host_client_port: remote_host_client_port_str.parse().unwrap(),
                                                            };
                                                            if server_data_settings.protocol == Protocol::Tcp && server_data_settings.remote_host_port == host_port && server_data_settings.remote_host_client_port == host_client_port {
                                                                let sequence_str = datagram.content.get("sequence").cloned().unwrap_or("-1".to_string());
                                                                match sequence_str.parse::<u64>() {
                                                                    Ok(received_sequence) => {
                                                                        if received_sequence > sequence {
                                                                            // TODO: persist datagram instead of discarding
                                                                            // blocking ACK reply, in order to receive retransmission
                                                                            warn!("sequence out of order. discarded. expected: {sequence} received: {received_sequence}")
                                                                        } else {
                                                                            // if received_sequence <= sequence
                                                                            let id_option = datagram.content.get("id");
                                                                            match id_option {
                                                                                Some(id) => {
                                                                                    info!("🔻 RECEIVED {}{{id:{}}}", datagram.r#type,id);
                                                                                    match tunnel_sender.send(ControlDatagram::ack(id)) {
                                                                                        Ok(_) => {}
                                                                                        Err(error) => {
                                                                                            error!("failed to send the ACK of {}{{id:{}}}: {}",datagram.r#type,id,error)
                                                                                        }
                                                                                    }
                                                                                }
                                                                                None => {
                                                                                    warn!("🔻 RECEIVED {} without id. Cannot send ACK.", datagram.r#type);
                                                                                }
                                                                            }
                                                                            if received_sequence == sequence {
                                                                                sequence += 1;
                                                                                let data_base64 = datagram.content["base64"].as_str();
                                                                                let data_result = base64::decode(data_base64);
                                                                                match data_result {
                                                                                    Ok(data) => {
                                                                                        if data.is_empty() {
                                                                                            let id = datagram.content["id"].as_str();
                                                                                            if id == "close" {
                                                                                                break;
                                                                                            }
                                                                                        }

                                                                                        match write_stream.write(data.as_slice()).await {
                                                                                            Ok(_) => {}
                                                                                            Err(error) => {
                                                                                                error!("could not send the received data to local client: {error}")
                                                                                            }
                                                                                        }
                                                                                    }
                                                                                    Err(error) => {
                                                                                        error!("could not decode remote server data: {error}")
                                                                                    }
                                                                                }
                                                                            }
                                                                        }
                                                                    }
                                                                    Err(error) => {
                                                                        error!("invalid sequence: {error}");
                                                                    }
                                                                }
                                                            }
                                                        }
                                                        "ACK" => {
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
                                                    error!("could not received data on the local client: {}",error)
                                                }
                                            }
                                        }
                                        info!("closed {}/tcp",local_client_address );
                                        let mut is_closed_mutex = is_closed2.lock().await;
                                        *is_closed_mutex = true;
                                    });
                                }
                                Err(error) => {
                                    error!("could not accept a tcp client: {}",error)
                                }
                            }
                        }
                    }
                    Err(error) => {
                        error!("could not start a tcp server socket: {}",error)
                    }
                }
            }
            OutputServer::Udp(server) => {
                let server_socket_result = UdpSocket::bind(server.address).await;
                match server_socket_result {
                    Ok(server_socket) => {
                        info!("listening on {}/udp",server.address);
                        let host_port = server_socket.local_addr().unwrap().port();
                        let tunnel_sender = server.writer_sender.clone();
                        let mut receiver = broadcast_sender.subscribe();
                        let (server_socket_sender, mut server_socket_sender_receiver) = channel::<(Vec<u8>,SocketAddr)>(1024);
                        let (server_socket_receiver_sender, mut server_socket_receiver) = channel::<(Vec<u8>,usize,SocketAddr)>(1024);
                        tokio::spawn(async move{
                            tokio::join!(
                                async{
                                    loop{
                                        let mut buff = Vec::with_capacity(65535);
                                        match server_socket.recv_buf_from(&mut buff).await{
                                            Ok((size,address)) => {
                                                let _=server_socket_receiver_sender.send((buff,size, address)).await;
                                            }
                                            Err(_) => {}
                                        }
                                    }
                                },
                                async{
                                    loop{
                                        match server_socket_sender_receiver.recv().await{
                                            Some((bytes,address)) => {
                                                let _= server_socket.send_to(bytes.as_slice(),address).await;
                                            }
                                            None => {}
                                        }
                                    }
                                }
                            )
                        });
                        tokio::spawn(async move {
                            let mut sequence: u64 = 0;
                            loop {
                                match server_socket_receiver.recv().await {
                                    Some((buff,size, udp_client)) => {
                                        let host_client_port = udp_client.port();
                                        let id = format!("udp_client_{host_port}_{host_client_port}_{sequence}");
                                        if size == 0 {
                                            info!("received 0 bytes from {}", udp_client);
                                        }
                                        let encoded_data = base64::encode(&buff[..size]);
                                        let datagram = ControlDatagram::client_data(
                                            id.as_str(),
                                            sequence,
                                            ClientDataSettings {
                                                protocol: Protocol::Udp,
                                                host_port,
                                                host_client_port,
                                            },
                                            encoded_data.as_str(),
                                        );
                                        match tunnel_sender.send(datagram.clone()) {
                                            Ok(_) => {}
                                            Err(error) => {
                                                error!("could not send local client data: {}", error);
                                            }
                                        }
                                        sequence += 1;
                                    }
                                    None => {
                                        error!("could not read local client data: None");
                                        break;
                                    }
                                };
                            }
                        });
                        tokio::spawn(async move {
                            loop {
                                match receiver.recv().await {
                                    Ok(datagram) => {
                                        match datagram.r#type.as_str() {
                                            "server_data" => {
                                                let id_option = datagram.content.get("id");
                                                match id_option {
                                                    Some(id) => {
                                                        info!("🔻 RECEIVED {}{{id:{}}}", datagram.r#type,id);
                                                    }
                                                    None => {
                                                        warn!("🔻 RECEIVED {} without id", datagram.r#type);
                                                    }
                                                }
                                                let protocol_str = datagram.content["protocol"].as_str();
                                                let remote_host_port_str = datagram.content["remoteHostPort"].as_str();
                                                let remote_host_client_port_str = datagram.content["remoteHostClientPort"].as_str();
                                                let server_data_settings = ServerDataSettings {
                                                    protocol: match protocol_str {
                                                        "tcp" => Protocol::Tcp,
                                                        "udp" => Protocol::Udp,
                                                        _ => {
                                                            error!("invalid protocol {}", protocol_str);
                                                            continue;
                                                        }
                                                    },
                                                    remote_host_port: remote_host_port_str.parse().unwrap(),
                                                    remote_host_client_port: remote_host_client_port_str.parse().unwrap(),
                                                };
                                                if server_data_settings.protocol == Protocol::Udp {
                                                    let data_base64 = datagram.content["base64"].as_str();
                                                    let data_result = base64::decode(data_base64);
                                                    let dest_address;
                                                    match server.address {
                                                        SocketAddr::V4(_) => {
                                                            dest_address = SocketAddr::V4(format!("127.0.0.1:{}", server_data_settings.remote_host_client_port).parse().unwrap());
                                                        }
                                                        SocketAddr::V6(_) => {
                                                            dest_address = SocketAddr::V6(format!("[::1]:{}", server_data_settings.remote_host_client_port).parse().unwrap());
                                                        }
                                                    }
                                                    match data_result {
                                                        Ok(data) => {
                                                            if data.is_empty() {
                                                                warn!("received 0 bytes from server to {}",dest_address);
                                                            }

                                                            match server_socket_sender.send((data, dest_address)).await {
                                                                Ok(_) => {}
                                                                Err(error) => {
                                                                    error!("could not send the received data to local client: {error}")
                                                                }
                                                            }
                                                        }
                                                        Err(error) => {
                                                            error!("could not decode remote server data: {error}")
                                                        }
                                                    }
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                    Err(error) => {
                                        error!("could not received data on the local client: {}",error)
                                    }
                                }
                            }
                        });
                    }
                    Err(error) => {
                        error!("could not start a udp server socket: {}",error)
                    }
                }
            }
        }
    }
}


// TODO: improve memory usage
async fn handle_retry(is_closed: Arc<Mutex<bool>>, tunnel_sender: Sender<ControlDatagram>, pre_tunnel_sender: Sender<ControlDatagram>, ack_sender: Sender<ControlDatagram>) {
    let mut pre_tunnel_receiver = pre_tunnel_sender.subscribe();
    loop {
        match pre_tunnel_receiver.recv().await {
            Ok(datagram) => {
                let datagram_id_option = datagram.content.get("id").cloned();
                match datagram_id_option {
                    Some(datagram_id) => {
                        let received_ack1 = Arc::new(Mutex::new(false));
                        let received_ack2 = received_ack1.clone();
                        let tunnel_sender_clone = tunnel_sender.clone();
                        let is_closed = is_closed.clone();
                        let mut ack_receiver = ack_sender.subscribe();
                        let ack_handler = tokio::spawn(async move {
                            loop {
                                match ack_receiver.recv().await {
                                    Ok(datagram) => {
                                        match datagram.r#type.as_str() {
                                            "ACK" => {
                                                let id = datagram.content["id"].clone();
                                                if id == datagram_id {
                                                    let mut received_ack_mutex = received_ack1.lock().await;
                                                    *received_ack_mutex = true;
                                                }
                                            }
                                            _ => {
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
                                    ack_handler.abort();
                                    break;
                                }
                            }
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