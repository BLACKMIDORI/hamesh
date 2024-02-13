use std::future::Future;
use std::net::SocketAddr;
use std::str::Utf8Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel};
use base64::DecodeError;
use log::{error, info};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::broadcast::Sender;
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
                        "server_data"=>{
                            match broadcast_sender.send(datagram) {
                                Ok(_) => {}
                                Err(error) => {
                                    error!("could not send to local client the received datagram: {}",error)
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


    async fn get_socket_broadcast(self, broadcast_sender: Sender<ControlDatagram>)  {
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
                                Ok((mut tcp_stream, local_client_address)) => {
                                    let (mut read_stream, mut write_stream) = tcp_stream.into_split();
                                    let host_client_port = local_client_address.port();
                                    let tunnel_sender = tunnel_sender_to_clone.clone();
                                    let reader_handler = tokio::spawn(async move {
                                        let mut count: u64 = 0;
                                        let mut received_zero_bytes = false;
                                        loop {
                                            let id = format!("tcp_client_{host_port}_{host_client_port}_{count}");
                                            let mut buff = [0; 10240];
                                            match read_stream.read(&mut buff).await {
                                                Ok(size) => {
                                                    if size == 0 {
                                                        if !received_zero_bytes{
                                                            received_zero_bytes = true;
                                                            continue;
                                                        }else{
                                                            break;
                                                        }
                                                    }
                                                    received_zero_bytes = false;
                                                    let encoded_data = base64::encode(&buff[..size]);
                                                    let datagram = ControlDatagram::client_data(
                                                        id.as_str(),
                                                        ClientDataSettings {
                                                            protocol: Protocol::Tcp,
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
                                                }
                                                Err(error) => {
                                                    error!("could not read local client data: {}", error);
                                                    break;
                                                }
                                            };
                                            count += 1;
                                        }
                                    });
                                    let support_sender = broadcast_sender.clone();
                                    tokio::spawn(async move{
                                        let _ = reader_handler.await;
                                        // Gracefully free the write thread and close it
                                        support_sender.send(ControlDatagram::server_data(
                                            "close",
                                            ServerDataSettings{
                                                protocol: Protocol::Tcp,
                                                remote_host_port: host_port,
                                                remote_host_client_port: host_client_port
                                            },
                                            ""
                                        )).expect("failed to signal the writer to close");
                                    });
                                    tokio::spawn(async move {
                                        loop {
                                            match receiver.recv().await {
                                                Ok(datagram) => {
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
                                                    if server_data_settings.protocol == Protocol::Tcp && server_data_settings.remote_host_port == host_port && server_data_settings.remote_host_client_port == host_client_port{
                                                        let data_base64 = datagram.content["base64"].as_str();
                                                        let data_result = base64::decode(data_base64);
                                                        match data_result{
                                                            Ok(data) => {
                                                                if data.is_empty(){
                                                                    let id = datagram.content["id"].as_str();
                                                                    if id == "close"{
                                                                        break;
                                                                    }
                                                                }

                                                                match  write_stream.write(data.as_slice()).await{
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
                                                Err(error) => {
                                                    error!("could not received data on the local client: {}",error)
                                                }
                                            }
                                        }
                                        info!("closed {}/tcp",local_client_address );
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
                        let sender = server.writer_sender.clone();
                        let mut buffer = [0; 10240];
                        loop {
                            match server_socket.recv_from(&mut buffer).await {
                                Ok((size, local_client_address)) => {
                                    info!("parser not implemented yet, size = {}",size)
                                }
                                Err(error) => {
                                    error!("could not read local client data: {}", error)
                                }
                            }
                        }
                    }
                    Err(error) => {
                        error!("could not start a udp server socket: {}",error)
                    }
                }
            }
        }
    }
}
