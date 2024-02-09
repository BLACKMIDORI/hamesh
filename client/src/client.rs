use std::net::SocketAddr;
use std::task::ready;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::time;
use crate::control_datagram::ControlDatagram;

pub async fn establish_connection(socket_std: std::net::UdpSocket, source_peer: SocketAddr, dest_peer: SocketAddr) {
    print!(
        "\n{} <-> {}\n", source_peer.to_string(), dest_peer.to_string());
    let socket = UdpSocket::from_std(socket_std.try_clone().unwrap()).unwrap();
    socket.connect(dest_peer).await.expect("Failed to connect to remote UDP");
    static mut RECEIVED_ACK: bool = false;

    unsafe {
        tokio::join!(
            async {
                loop{
                socket_loop(UdpSocket::from_std(socket_std.try_clone().unwrap()).unwrap(), &mut RECEIVED_ACK).await
                }
            },
                async{
    let mut interval = time::interval(Duration::from_secs(1));
            loop{
            if !RECEIVED_ACK{
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

async fn socket_loop(socket: UdpSocket, received_ack: &mut bool) {
    let mut buff = [0; 4096];
    match socket.recv(&mut buff).await {
        Ok(size) => {
            match std::str::from_utf8(&buff[..size]) {
                Ok(data) => {
                    match serde_json::from_str::<ControlDatagram>(data) {
                        Ok(control_datagram) => {
                            print!("<- {}\n", control_datagram.r#type);
                            if control_datagram.version != 1 {
                                panic!("unrecognized version")
                            }
                            match control_datagram.r#type.as_str() {
                                "SYN" => {
                                    send_ack(socket).await
                                }
                                "ACK" => {
                                    if !*received_ack {
                                        *received_ack = true;
                                        print!("Connection established!\n")
                                    }
                                }
                                _ => {
                                    print!("Unknown control datagram: {}\n", data)
                                }
                            }
                        }
                        _ => {
                            print!("Unknown datagram: '{}'\n", data);
                            print!("reason: '{:?}'\n", serde_json::from_str::<ControlDatagram>(data));
                        }
                    }
                }
                Err(e) => {
                    print!("{}\n", e);
                }
            }
        }
        Err(e) => {
            print!("[ERROR] socket.recv: {}\n", e);
        }
    };
}

async fn send_syn(socket: UdpSocket) {
    let syn = serde_json::to_vec(&ControlDatagram { version: 1, r#type: "SYN".to_string() }).unwrap();
    print!("-> SYN\n");
    socket.send(syn.as_slice()).await.expect("Failed to send SYN");
}

async fn send_ack(socket: UdpSocket) {
    let ack = serde_json::to_vec(&ControlDatagram { version: 1, r#type: "ACK".to_string() }).unwrap();
    print!("-> ACK\n");
    socket.send(ack.as_slice()).await.expect("Failed to send ACK");
}