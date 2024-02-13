use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};
use std::str::Utf8Error;
use log::{error, info};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::broadcast::Sender;
use crate::control_datagram::ControlDatagram;
use crate::get_ip_version::get_ip_version;
use crate::ip_version::IpVersion;
use crate::settings_models::{ClientDataSettings, PortSettings, Protocol, ServerDataSettings};

#[derive(Clone)]
pub struct InputClient {
    pub settings: ClientDataSettings,
    pub from_outside_tx: Sender<Vec<u8>>,
}


impl InputClient {
    pub fn new(settings: ClientDataSettings,from_outside_tx: Sender<Vec<u8>>)->InputClient{
        InputClient{settings,from_outside_tx}
    }
    pub async fn start(self, input_settings: &PortSettings, tunnel_sender : Sender<ControlDatagram>) {
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

        match TcpStream::connect(address).await {
            Ok(stream) => {
                let client_address = stream.local_addr().unwrap();
                let client_port = client_address.port();
                let (mut read_stream, mut write_stream) = stream.into_split();
                let mut receiver = self.from_outside_tx.subscribe();
                tokio::spawn(async move{
                    loop{
                        match receiver.recv().await{
                            Ok(data)=>{
                                match write_stream.write(&data).await{
                                    Ok(_) => {}
                                    Err(error) => {
                                        error!("could not receive the data for local server on its client: {error}")
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                });
                tokio::spawn(async move{
                    let mut count: u64 = 0;
                    let mut received_zero_bytes = false;
                    loop{
                        let id = format!("tcp_server_{server_port}_{client_port}_{count}");
                        let mut buff = [0; 10240];
                        match read_stream.read(&mut buff).await {
                            Ok(size) => {
                                if size == 0 {
                                    if !received_zero_bytes {
                                        received_zero_bytes = true;
                                        continue;
                                    } else {
                                        info!("closed {}/tcp",client_address );
                                        break;
                                    }
                                }
                                received_zero_bytes = false;
                                let encoded_data = base64::encode(&buff[..size]);
                                let datagram = ControlDatagram::server_data(
                                    id.as_str(),
                                    ServerDataSettings {
                                        protocol: Protocol::Tcp,
                                        remote_host_port: self.settings.host_port,
                                        remote_host_client_port: self.settings.host_client_port,
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
                                error!("could not read local server data: {}", error)
                            }
                        }
                    }
                });
            },
            Err(error) => {
                error!("Failed to connect to local server({address}): {error}");
            }
        }
    }
}
