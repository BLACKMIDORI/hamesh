use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;
use log::{error, info, warn};
use tokio::net::UdpSocket;
use tokio::time;
use crate::control_datagram::ControlDatagram;

pub async fn establish_connection(socket_std: std::net::UdpSocket, source_peer: SocketAddr, dest_peer: SocketAddr) {
    info!(
        "connected {} <-> {}", source_peer.to_string(), dest_peer.to_string());
    let socket = UdpSocket::from_std(socket_std.try_clone().unwrap()).unwrap();
    socket.connect(dest_peer).await.expect("Failed to connect to remote UDP");
    static mut RECEIVED_SYN_ACK: bool = false;

    unsafe {
        tokio::join!(
            async {
                loop{
                socket_loop(UdpSocket::from_std(socket_std.try_clone().unwrap()).unwrap(), &mut RECEIVED_SYN_ACK).await
                }
            },
                async{
    let mut interval = time::interval(Duration::from_secs(1));
            loop{
            if !RECEIVED_SYN_ACK{
                send_syn(UdpSocket::from_std(socket_std.try_clone().unwrap()).unwrap()).await;
                        interval.tick().await;
            }else{
                        break;
                    }
            }
                }
            );
    }
}

async fn socket_loop(socket: UdpSocket, received_syn_ack: &mut bool) {
    let mut buff = [0; 4096];
    match socket.recv(&mut buff).await {
        Ok(size) => {
            match std::str::from_utf8(&buff[..size]) {
                Ok(data) => {
                    match serde_json::from_str::<ControlDatagram>(data) {
                        Ok(control_datagram) => {
                            if control_datagram.version != 1 {
                                error!("🔻 RECEIVED {} with unrecognized version", control_datagram.r#type);
                                panic!("unrecognized version")
                            }
                            match control_datagram.r#type.as_str() {
                                "SYN" => {
                                    info!("🔻 RECEIVED {}", control_datagram.r#type);
                                    send_ack(socket, "SYN").await
                                }
                                "ACK" => {
                                    info!("🔻 RECEIVED {}{{id:{}}}", control_datagram.r#type, control_datagram.content["id"]);
                                    if control_datagram.content["id"]  == "SYN" {
                                        if !*received_syn_ack {
                                            *received_syn_ack = true;
                                            info!("✅  connection established!")
                                        }
                                    }
                                }
                                _ => {
                                    warn!("unknown control datagram: {}", data)
                                }
                            }
                        }
                        _ => {
                            warn!("unknown datagram: '{}'", data);
                        }
                    }
                }
                Err(e) => {
                    error!("{}", e);
                }
            }
        }
        Err(e) => {
            error!("[ERROR] socket.recv: {}", e);
        }
    };
}

async fn send_syn(socket: UdpSocket) {
    let syn = serde_json::to_vec(&ControlDatagram { version: 1, r#type: "SYN".to_string(), content: HashMap::new() }).unwrap();
    socket.send(syn.as_slice()).await.expect("Failed to send SYN");
    info!(" ⃤ SENT SYN");
}

async fn send_ack(socket: UdpSocket, id:  &'static str) {
    let mut content = HashMap::new();
    content.insert("id".to_string(), id.to_string());
    let ack = serde_json::to_vec(&ControlDatagram { version: 1, r#type: "ACK".to_string(), content}).unwrap();
    socket.send(ack.as_slice()).await.expect("Failed to send ACK");
    info!(" ⃤ SENT ACK{{id:{}}}", id);
}