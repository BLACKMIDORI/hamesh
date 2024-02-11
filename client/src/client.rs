use std::collections::HashMap;
use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;
use log::{error, info, warn};
use regex::Regex;
use tokio::time;
use crate::port_settings::{PortSettings, Protocol};
use crate::control_datagram::ControlDatagram;
use crate::ip_version::IpVersion;

pub async fn establish_connection(socket_std: std::net::UdpSocket, source_peer: SocketAddr, dest_peer: SocketAddr) {
    let socket = tokio::net::UdpSocket::from_std(socket_std.try_clone().unwrap()).unwrap();
    socket.connect(dest_peer).await.expect("Failed to connect to remote UDP");
    info!(
        "connected {} <-> {}", source_peer.to_string(), dest_peer.to_string());
    // begin: Prepare connection settings

    let args: Vec<String> = std::env::args().collect();
    let joined = args[1..].join(" ");
    let regex = Regex::new(r"--(?:input|output) \d+:\d+/(?:tcp|udp)").unwrap();

    let mut input_settings = HashMap::new();
    let mut output_settings = HashMap::new();
    for r#match in regex.find_iter(joined.as_str()) {
        let values = &r#match.as_str();
        let parts: Vec<&str> = values.split(" ").collect();
        let input_or_output = parts[0];
        let ports_and_protocol: Vec<&str> = parts[1].split("/").collect();
        let ports: Vec<&str> = ports_and_protocol[0].split(":").collect();
        let protocol_str = ports_and_protocol[1];
        let host_port = ports[0].parse().unwrap();
        let remote_host_port = ports[1].parse().unwrap();
        let protocol = match protocol_str {
            "tcp" => Protocol::Tcp,
            "udp" => Protocol::Udp,
            _ => continue
        };
        match input_or_output {
            "--input" => {
                input_settings.insert(format!("{host_port}:{remote_host_port}"), PortSettings {
                    protocol,
                    host_port,
                    remote_host_port,
                });
            }
            "--output" => {
                output_settings.insert(format!("{host_port}:{remote_host_port}"), PortSettings {
                    protocol,
                    host_port,
                    remote_host_port,
                });
            }
            _ => continue
        }
    }
    // end: Prepare connection settings


    let received_syn_ack = &AtomicBool::new(false);
    let (inputs_ack_tx, inputs_ack_rx) = channel();
    let (tx, rx) = channel();

    tokio::join!(
        async{
            tokio::spawn(datagram_handler(socket_std.try_clone().unwrap(),output_settings,rx));
            delivery_syn(socket_std.try_clone().unwrap(),received_syn_ack).await;
            delivery_input_ports(socket_std.try_clone().unwrap(), &input_settings, &inputs_ack_rx).await;

        },
        socket_read_loop_wrapper(socket_std.try_clone().unwrap(),received_syn_ack,&inputs_ack_tx,&tx),
    );
}

async fn delivery_syn(socket_std: std::net::UdpSocket, received_syn_ack: &AtomicBool) {
    let mut interval = time::interval(Duration::from_secs(1));
    loop {
        if !received_syn_ack.load(Ordering::Relaxed) {
            send_syn(tokio::net::UdpSocket::from_std(socket_std.try_clone().unwrap()).unwrap()).await;
            interval.tick().await;
        } else {
            break;
        }
    }
}

async fn delivery_input_ports(socket_std: std::net::UdpSocket, input_settings: &HashMap<String, PortSettings>, inputs_ack_rx: &Receiver<String>) {
    let mut interval = time::interval(Duration::from_secs(1));
    let mut remaining_inputs = HashMap::new();
    remaining_inputs.extend(input_settings.into_iter());
    loop {
        if !remaining_inputs.is_empty() {
            for (key, value) in &remaining_inputs {
                let id = format!("input_port_{}", key);
                send_input_port(tokio::net::UdpSocket::from_std(socket_std.try_clone().unwrap()).unwrap(), id.as_str(), value).await;
            }
            interval.tick().await;

            match inputs_ack_rx.try_recv() {
                Ok(id) => {
                    let maybe_item = remaining_inputs.iter().find(|(key, value)| {
                        let local_id = format!("input_port_{}", key);
                        local_id == id
                    });
                    match maybe_item {
                        Some((key, entry)) => {
                            remaining_inputs.remove(*key);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        } else {
            break;
        }
    }
}


async fn datagram_handler(socket_std: std::net::UdpSocket, output_settings: HashMap<String, PortSettings>, receiver: Receiver<ControlDatagram>) {
    let args: Vec<String> = std::env::args().collect();
    let ip_version_str = &args[2];
    let ip_version = match ip_version_str.as_str() {
        "ipv4" => { IpVersion::Ipv4 }
        "ipv6" => { IpVersion::Ipv6 }
        _ => {
            error!("invalid ip version: {}",ip_version_str);
            return;
        }
    };
    let mut output_tcp_sockets = HashMap::new();
    let mut output_udp_sockets = HashMap::new();
    loop {
        let result = receiver.recv();
        match result {
            Ok(datagram) => {
                match datagram.r#type.as_str() {
                    "input_port" => {
                        let protocol_str = datagram.content["protocol"].as_str();
                        let host_port_str = datagram.content["hostPort"].as_str();
                        let remote_host_port_str = datagram.content["remoteHostPort"].as_str();
                        let input_settings = PortSettings {
                            protocol: match protocol_str {
                                "tcp" => Protocol::Tcp,
                                "udp" => Protocol::Udp,
                                _ => {
                                    error!("invalid protocol {}", protocol_str);
                                    continue;
                                }
                            },
                            host_port: host_port_str.parse().unwrap(),
                            remote_host_port: remote_host_port_str.parse().unwrap(),
                        };
                        let output_settings_option = output_settings.iter().find(|(key, settings)| {
                            settings.protocol == input_settings.protocol && settings.remote_host_port == input_settings.host_port && settings.host_port == input_settings.remote_host_port
                        });
                        match output_settings_option {
                            Some((key, output_settings)) => {
                                match output_settings.protocol {
                                    Protocol::Tcp => {
                                        let port = output_settings.host_port;
                                        if output_tcp_sockets.contains_key(&port) {
                                            warn!("already open {port}/tcp");
                                            continue;
                                        }
                                        let address;
                                        let tcp_socket_result;
                                        match ip_version {
                                            IpVersion::Ipv4 => {
                                                address = format!("0.0.0.0:{port}").parse().unwrap();
                                                tcp_socket_result = tokio::net::TcpSocket::new_v4();
                                            }
                                            IpVersion::Ipv6 => {
                                                address = format!("[::]:{port}").parse().unwrap();
                                                tcp_socket_result = tokio::net::TcpSocket::new_v6();
                                            }
                                        }
                                        match tcp_socket_result {
                                            Ok(tcp_socket) => {
                                                let result = tcp_socket.bind(address);
                                                match result {
                                                    Ok(_) => {
                                                        output_tcp_sockets.insert(port, tcp_socket);
                                                        info!("listening on {address}/tcp")
                                                    }
                                                    Err(error) => {
                                                        error!("could not start a tcp socket: {}",error)
                                                    }
                                                }
                                            }
                                            _ => continue
                                        }
                                    }
                                    Protocol::Udp => {
                                        let port = output_settings.host_port;
                                        if output_udp_sockets.contains_key(&port) {
                                            warn!("already open {port}/udp");
                                            continue;
                                        }
                                        let address: SocketAddr;
                                        let udp_socket_result;
                                        match ip_version {
                                            IpVersion::Ipv4 => {
                                                address = SocketAddr::V4(format!("0.0.0.0:{port}").parse::<SocketAddrV4>().unwrap());
                                                udp_socket_result = tokio::net::UdpSocket::bind(address).await;
                                            }
                                            IpVersion::Ipv6 => {
                                                address = SocketAddr::V6(format!("[::]:{port}").parse::<SocketAddrV6>().unwrap());
                                                udp_socket_result = tokio::net::UdpSocket::bind(address).await;
                                            }
                                        }
                                        match udp_socket_result {
                                            Ok(udp_socket) => {
                                                output_udp_sockets.insert(port, udp_socket);
                                                info!("listening on {address}/udp")
                                            }
                                            Err(error) => {
                                                error!("could not start a udp socket: {}",error)
                                            }
                                        }
                                    }
                                }
                            }
                            None => {
                                warn!("input settings from remote host does not match any output settings: {:?}",input_settings);
                            }
                        }
                    }
                    _ => {
                        warn!("unknown control datagram: {:?}", datagram)
                    }
                }
            }
            Err(error) => {
                error!("receiver.recv: {}",error)
            }
        }
    }
}

async fn socket_read_loop_wrapper(socket_std: std::net::UdpSocket, received_syn_ack: &AtomicBool, inputs_sender: &Sender<String>, sender: &Sender<ControlDatagram>) {
    loop {
        socket_read_loop(tokio::net::UdpSocket::from_std(socket_std.try_clone().unwrap()).unwrap(), received_syn_ack, inputs_sender, sender).await
    }
}

async fn socket_read_loop(socket: tokio::net::UdpSocket, received_syn_ack: &AtomicBool, inputs_sender: &Sender<String>, sender: &Sender<ControlDatagram>) {
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
                                    if control_datagram.content["id"] == "SYN" {
                                        if !received_syn_ack.load(Ordering::Relaxed) {
                                            received_syn_ack.store(true, Ordering::Relaxed);
                                            info!("✅  connection established!")
                                        }
                                    } else {
                                        match inputs_sender.send(control_datagram.content["id"].to_string()) {
                                            Ok(_) => {}
                                            Err(_) => {
                                                error!("could not send ACK for handling: {:?}", control_datagram)
                                            }
                                        }
                                    }
                                }
                                _ => {
                                    let id_option = control_datagram.content.get("id");
                                    match id_option {
                                        Some(id) => {
                                            info!("🔻 RECEIVED {}{{id:{}}}", control_datagram.r#type,id);
                                            send_ack(socket, id).await
                                        }
                                        None => {
                                            warn!("🔻 RECEIVED {} without id. Can not send ACK.", control_datagram.r#type);
                                        }
                                    }
                                    let r#type = control_datagram.r#type.as_str().to_string();
                                    let id_option_str = id_option.and_then(|text| Some(text.to_string()));
                                    match sender.send(control_datagram) {
                                        Ok(_) => {}
                                        Err(_) => {
                                            match id_option_str {
                                                Some(id) => {
                                                    error!("could not send datagram for handling: {:?}{{id:{}}}", r#type, id);
                                                }
                                                None => {
                                                    error!("could not send datagram for handling: {:?}",r#type);
                                                }
                                            }
                                        }
                                    }
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
            error!("socket.recv: {}", e);
        }
    };
}

async fn send_syn(socket: tokio::net::UdpSocket) {
    let datagram = ControlDatagram::syn();
    let datagram_bytes = serde_json::to_vec(&datagram).unwrap();
    socket.send(datagram_bytes.as_slice()).await.expect("Failed to send SYN");
    info!(" ⃤ SENT SYN");
}

async fn send_ack(socket: tokio::net::UdpSocket, id: &str) {
    let datagram = ControlDatagram::ack(id);
    let datagram_bytes = serde_json::to_vec(&datagram).unwrap();
    socket.send(datagram_bytes.as_slice()).await.expect(format!("Failed to send {}", datagram.r#type).as_str());
    info!(" ⃤ SENT ACK{{id:{}}}", id);
}

async fn send_input_port(socket: tokio::net::UdpSocket, id: &str, port_settings: &PortSettings) {
    let datagram = ControlDatagram::input_port(id, PortSettings {
        protocol: match port_settings.protocol {
            Protocol::Tcp => { Protocol::Tcp }
            Protocol::Udp => { Protocol::Udp }
        },
        host_port: port_settings.host_port,
        remote_host_port: port_settings.remote_host_port,
    });
    let datagram_bytes = serde_json::to_vec(&datagram).unwrap();
    socket.send(datagram_bytes.as_slice()).await.expect(format!("Failed to send {}", datagram.r#type).as_str());
    info!(" ⃤ SENT {}{{id:{}}}",datagram.r#type, datagram.content["id"]);
}