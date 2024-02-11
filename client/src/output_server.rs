use std::net::SocketAddr;
use std::sync::mpsc::{channel};
use log::{error, info};
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, UdpSocket};
use crate::control_datagram::ControlDatagram;

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
        tokio::spawn(self1.handle_from_remote_data());
        tokio::spawn(self2.handle_to_remote_data());
    }
    async fn handle_from_remote_data(self) {
        let mut receiver;
        let (sender, host_client_receiver) = channel();
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
                    info!("can also read: {:?}",datagram);
                    match sender.send(datagram) {
                        Ok(_) => {}
                        Err(error) => {
                            error!("could not send to local client the received datagram: {}",error)
                        }
                    };
                }
                Err(error) => {
                    error!("error receiving datagram for handling: {}",error)
                }
            }
        }
    }

    async fn handle_to_remote_data(self) {
        match self {
            OutputServer::Tcp(server) => {
                let server_socket_result = TcpListener::bind(server.address).await;
                match server_socket_result {
                    Ok(server_socket) => {
                        info!("listening on {}/tcp",server_socket.local_addr().unwrap());
                        let sender = server.writer_sender.clone();
                        loop {
                            let result = server_socket.accept().await;
                            match result {
                                Ok((mut tcp_stream, local_client_address)) => {
                                    let mut buff = [0; 4096];
                                    match tcp_stream.read(&mut buff).await {
                                        Ok(size) => {
                                            info!("parser not implemented yet, size = {}",size)
                                        }
                                        Err(error) => {
                                            error!("could not read local client data: {}", error)
                                        }
                                    }
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
                        let mut buffer = [0; 4096];
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