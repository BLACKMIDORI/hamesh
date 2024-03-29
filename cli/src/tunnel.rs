use std::collections::{HashMap, VecDeque};
use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::broadcast::{channel, Sender};
use std::time::Duration;
use log::{error, info, warn};
use regex::Regex;
use tokio::time;
use crate::settings_models::{ClientDataSettings, PortSettings, Protocol};
use crate::control_datagram::ControlDatagram;
use crate::get_ip_version::get_ip_version;
use crate::input_client::InputClient;
use crate::ip_version::IpVersion;
use crate::output_server::{OutputServer, OutputServerConfigTcp, OutputServerConfigUdp};


pub async fn establish_connection(socket_std: std::net::UdpSocket, source_peer: SocketAddr, dest_peer: SocketAddr) {
    let socket = tokio::net::UdpSocket::from_std(socket_std.try_clone().unwrap()).unwrap();
    socket.connect(dest_peer).await.expect("Failed to connect to remote UDP");
    info!(
        "connected {} <-> {}", source_peer.to_string(), dest_peer.to_string());
    // begin: Prepare connection settings

    let args: Vec<String> = std::env::args().collect();
    let joined = args[1..].join(" ");
    let regex = Regex::new(r"--(?:inbound|outbound) \d+:\d+/(?:tcp|udp)").unwrap();

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
            "--inbound" => {
                input_settings.insert(format!("{host_port}:{remote_host_port}"), PortSettings {
                    protocol,
                    host_port,
                    remote_host_port,
                });
            }
            "--outbound" => {
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
    let input_settings_clone = input_settings.clone();

    let received_syn_ack = &AtomicBool::new(false);
    let inputs_finished = &AtomicBool::new(false);
    let (inputs_ack_tx, inputs_ack_rx) = std::sync::mpsc::channel();
    let (datagram_handler_sender, _) = channel(1024);
    let datagram_handler_sender_dest = datagram_handler_sender.clone();
    let (handler_to_writer_sender_to_clone, _) = channel(1024);
    let (input_clients_sender, receiver) = tokio::sync::mpsc::channel(1024);

    tokio::join!(
        async{
            tokio::spawn(datagram_handler(datagram_handler_sender_dest,handler_to_writer_sender_to_clone.clone(),input_clients_sender,output_settings));
            tokio::spawn(datagram_from_local_client_to_tunnel_wrapper(socket_std.try_clone().unwrap(), handler_to_writer_sender_to_clone.clone()));
            tokio::spawn(input_clients_handler(handler_to_writer_sender_to_clone.clone(),receiver, input_settings_clone));
            delivery_syn(socket_std.try_clone().unwrap(),received_syn_ack).await;
            delivery_input_ports(socket_std.try_clone().unwrap(), &input_settings, &inputs_ack_rx).await;
             inputs_finished.store(true, Ordering::Relaxed);
        },

        socket_read_loop(socket_std.try_clone().unwrap(),received_syn_ack,inputs_finished,&inputs_ack_tx,&datagram_handler_sender),
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

async fn delivery_input_ports(socket_std: std::net::UdpSocket, input_settings: &HashMap<String, PortSettings>, inputs_ack_rx: &std::sync::mpsc::Receiver<String>) {
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
                    let maybe_item = remaining_inputs.iter().find(|(key, _)| {
                        let local_id = format!("input_port_{}", key);
                        local_id == id
                    });
                    match maybe_item {
                        Some((key, _)) => {
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


async fn datagram_handler(datagram_handler_sender: Sender<ControlDatagram>, writer_sender: Sender<ControlDatagram>, input_clients_sender: tokio::sync::mpsc::Sender<ControlDatagram>, output_settings: HashMap<String, PortSettings>) {
    let mut receiver = datagram_handler_sender.subscribe();
    loop {
        match receiver.recv().await {
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
                        let output_settings_option = output_settings.iter().find(|(_, settings)| {
                            settings.protocol == input_settings.protocol && settings.remote_host_port == input_settings.host_port && settings.host_port == input_settings.remote_host_port
                        });
                        match output_settings_option {
                            Some((_, output_settings)) => {
                                match output_settings.protocol {
                                    Protocol::Tcp => {
                                        let port = output_settings.host_port;
                                        // if output_tcp_servers.contains_key(&port) {
                                        //     warn!("already open {port}/tcp");
                                        //     continue;
                                        // }
                                        let reader_sender_clone = datagram_handler_sender.clone();
                                        let writer_sender_clone = writer_sender.clone();
                                        tokio::spawn(async move {
                                            let address;
                                            // let tcp_listener_result;
                                            match get_ip_version().unwrap() {
                                                IpVersion::Ipv4 => {
                                                    address = SocketAddr::V4(format!("0.0.0.0:{port}").parse::<SocketAddrV4>().unwrap());
                                                    // tcp_listener_result = tokio::net::TcpListener::bind(address).await;
                                                }
                                                IpVersion::Ipv6 => {
                                                    address = SocketAddr::V6(format!("[::]:{port}").parse::<SocketAddrV6>().unwrap());
                                                    // tcp_listener_result = tokio::net::TcpListener::bind(address).await;
                                                }
                                            }

                                            let output_server = OutputServer::Tcp(OutputServerConfigTcp::new(reader_sender_clone, writer_sender_clone, address));
                                            output_server.start().await;
                                            // match tcp_listener_result {
                                            //     Ok(tcp_listener) => {
                                            //         let output_server = OutputServer::Tcp(OutputServerTcp::new(reader_sender_clone, writer_sender_clone, tcp_listener));
                                            //         output_server.start().await;
                                            //     }
                                            //     _ => return
                                            // }
                                        });
                                    }
                                    Protocol::Udp => {
                                        let port = output_settings.host_port;
                                        // if output_udp_servers.contains_key(&port) {
                                        //     warn!("already open {port}/udp");
                                        //     continue;
                                        // }
                                        let reader_sender_clone = datagram_handler_sender.clone();
                                        let writer_sender_clone = writer_sender.clone();
                                        tokio::spawn(async move {
                                            let address: SocketAddr;
                                            // let udp_socket_result;
                                            match get_ip_version().unwrap() {
                                                IpVersion::Ipv4 => {
                                                    address = SocketAddr::V4(format!("0.0.0.0:{port}").parse::<SocketAddrV4>().unwrap());
                                                    // udp_socket_result = tokio::net::UdpSocket::bind(address).await;
                                                }
                                                IpVersion::Ipv6 => {
                                                    address = SocketAddr::V6(format!("[::]:{port}").parse::<SocketAddrV6>().unwrap());
                                                    // udp_socket_result = tokio::net::UdpSocket::bind(address).await;
                                                }
                                            }
                                            let output_server = OutputServer::Udp(OutputServerConfigUdp::new(reader_sender_clone, writer_sender_clone, address));
                                            output_server.start().await;
                                            // match udp_socket_result {
                                            //     Ok(udp_socket) => {
                                            //         let output_server = OutputServer::Udp(OutputServerConfigTcp::new(reader_sender_clone,writer_sender_clone, address));
                                            //         output_server.start().await;
                                            //     }
                                            //     Err(error) => {
                                            //         error!("could not start a udp socket: {}",error)
                                            //     }
                                            // }
                                        });
                                    }
                                }
                            }
                            None => {
                                warn!("input settings from remote host does not match any output settings: {:?}",input_settings);
                            }
                        }
                    }
                    "server_data" => {
                        // it is going to be handled by output server
                    }
                    "ACK" => {
                        // it is going to be handled by output server

                        // send also to input clients
                        match input_clients_sender.send(datagram).await {
                            Ok(_) => {}
                            Err(error) => {
                                error!("could not send the received client data to broadcast: {}",error)
                            }
                        }
                    }
                    "client_data" => {
                        match input_clients_sender.send(datagram).await {
                            Ok(_) => {}
                            Err(error) => {
                                error!("could not send the received client data to broadcast: {}",error)
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

#[derive(Clone)]
struct InputClientData {
    input_settings: PortSettings,
    datagram: ControlDatagram,
}

async fn input_clients_handler(tunnel_writer_sender: Sender<ControlDatagram>, mut remote_client_data_receiver: tokio::sync::mpsc::Receiver<ControlDatagram>, input_settings: HashMap<String, PortSettings>) {
    let (sender, mut receiver) = tokio::sync::mpsc::channel::<InputClientData>(1024);

    tokio::spawn(async move {
        // TODO: improve clients handling, one thread to insert, another to remove(when client is closed).
        let mut clients = HashMap::new();
        // let mut to_remove = Vec::new();
        let mut interval = time::interval(Duration::from_secs(10));
        let should_perform_clients_scan = AtomicBool::new(false);
        tokio::join!(async {
            loop{
                interval.tick().await;
                should_perform_clients_scan.store(true, Ordering::Relaxed);
            }
        },
        async{
                loop{
                    match receiver.recv().await {
                        Some(data)=>{
                    let input_settings = data.input_settings;
                    let datagram = data.datagram;
                            match datagram.r#type.as_str(){
                                "client_data"=>{

                    let content = &datagram.content;
                    let protocol_str = datagram.content["protocol"].as_str();
                    let host_port_str = datagram.content["hostPort"].as_str();
                    let host_client_port_str = content["hostClientPort"].as_str();
                    let client_data_settings = ClientDataSettings {
                        protocol: match protocol_str {
                            "tcp" => Protocol::Tcp,
                            "udp" => Protocol::Udp,
                            _ => {
                                error!("invalid protocol {}", protocol_str);
                                continue;
                            }
                        },
                        host_port: host_port_str.parse().unwrap(),
                        host_client_port: host_client_port_str.parse().unwrap(),
                    };

                            let client_id = format!("{}_{}_{}", protocol_str, client_data_settings.host_port, client_data_settings.host_client_port);
                            let mut client_option = clients.get(client_id.as_str()).cloned();
                            if client_option.is_none(){
                                let (to_inside,_) = channel(1024);
                                let client = InputClient::new(client_data_settings,to_inside);
                                clients.insert(client_id.clone(), client.clone());
                                info!("client inserted: {}",client_id.as_str());
                                client_option = Some(client.clone());
                                client.start(input_settings,tunnel_writer_sender.clone()).await;
                            }
                            let client = client_option.unwrap();
                            match client.from_outside_tx.send(datagram){
                                Ok(_) => {}
                                Err(error) => {
                                    error!("could not send the received datagram to local server: {error}")
                                }
                            }
                                }
                                "ACK"=>{
                                    for (_,client) in clients.clone() {
                            match client.from_outside_tx.send(datagram.clone()){
                                Ok(_) => {}
                                Err(error) => {
                                    error!("could not send the received datagram to local server: {error}")
                                }
                            }
                                    }
                                }
                            _ => {}}
                }
                None => {
                    error!("could not receive data from input clients channel: None");
                }
            }
                     if should_perform_clients_scan.load(Ordering::Relaxed){
                        let mut  count = 0;
                        for (key,client) in clients.clone(){
                            if *client.is_closed.lock().await {
                            clients.remove(key.as_str());
                                count = count+1;
                            }
                        }
                        if count>0{
                    info!("clients removed: {count}");
                        }
                    }
        }
            });
    });
    loop {
        match remote_client_data_receiver.recv().await {
            Some(datagram) => {
                match datagram.r#type.as_str() {
                    "client_data" => {
                        let content = &datagram.content;
                        let protocol_str = datagram.content["protocol"].as_str();
                        let host_port_str = datagram.content["hostPort"].as_str();
                        let host_client_port_str = content["hostClientPort"].as_str();
                        let client_data_settings = ClientDataSettings {
                            protocol: match protocol_str {
                                "tcp" => Protocol::Tcp,
                                "udp" => Protocol::Udp,
                                _ => {
                                    error!("invalid protocol {}", protocol_str);
                                    continue;
                                }
                            },
                            host_port: host_port_str.parse().unwrap(),
                            host_client_port: host_client_port_str.parse().unwrap(),
                        };
                        let input_settings_option = input_settings.iter().find(|(_, settings)| {
                            settings.protocol == client_data_settings.protocol && settings.remote_host_port == client_data_settings.host_port
                        });

                        match input_settings_option {
                            Some((_, input_settings)) => {
                                match sender.send(InputClientData {
                                    input_settings: input_settings.clone(),
                                    datagram,
                                }).await {
                                    Ok(_) => {}
                                    Err(error) => {
                                        error!("could not send data to input clients channel: {error}");
                                    }
                                }
                            }
                            None => {
                                warn!("client data settings from remote host does not match any input settings: {:?}",input_settings);
                            }
                        }
                    }
                    "ACK" => {
                        // TODO: ACK does not have enough information about the target client
                        match sender.send(InputClientData {
                            input_settings: PortSettings {
                                protocol: Protocol::Tcp,
                                remote_host_port: 0,
                                host_port: 0,
                            },
                            datagram,
                        }).await {
                            Ok(_) => {}
                            Err(error) => {
                                error!("could not send data to input clients channel: {error}");
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

async fn datagram_from_local_client_to_tunnel_wrapper(socket_std: std::net::UdpSocket, writer_sender: Sender<ControlDatagram>) {
    let mut receiver = writer_sender.subscribe();
    let tunnel_socket = tokio::net::UdpSocket::from_std(socket_std.try_clone().unwrap()).unwrap();
    loop {
        match receiver.recv().await {
            Ok(datagram) => {
                send_datagram(&tunnel_socket, datagram).await;
            }
            Err(_) => {}
        }
    }
}


async fn socket_read_loop(socket_std: std::net::UdpSocket, received_syn_ack: &AtomicBool, inputs_finished: &AtomicBool, inputs_sender: &std::sync::mpsc::Sender<String>, datagram_handler_sender: &Sender<ControlDatagram>) {
    let mut fragment_datagrams:VecDeque<ControlDatagram> = VecDeque::new();
    loop{
        if fragment_datagrams.len()>100{
            fragment_datagrams.pop_back();
        }
        let socket = tokio::net::UdpSocket::from_std(socket_std.try_clone().unwrap()).unwrap();
        let mut buff = Vec::with_capacity(65535);
        match socket.recv_buf(&mut buff).await {
            Ok(size) => {
                match std::str::from_utf8(&buff[..size]) {
                    Ok(data) => {
                        match serde_json::from_str::<ControlDatagram>(data) {
                            Ok(maybe_fragment) => {
                                if maybe_fragment.version != 1 {
                                    error!("🔻 RECEIVED {} with unrecognized version", maybe_fragment.r#type);
                                    panic!("unrecognized version")
                                }
                                let mut actual_datagram = Some(maybe_fragment.clone());
                                if maybe_fragment.r#type == "fragment"{
                                    actual_datagram = None;
                                    if maybe_fragment.content["contentType"] == "control_datagram" {
                                        let index:i32 = maybe_fragment.content["index"].clone().parse().unwrap();
                                        let digest = maybe_fragment.content["digest"].clone();
                                        if !fragment_datagrams.iter().any(|fragment_datagram|{
                                            fragment_datagram.content["digest"] == digest && fragment_datagram.content["index"] == maybe_fragment.content["index"]
                                        }){
                                            fragment_datagrams.push_front(maybe_fragment.clone());
                                        }
                                        let length:i32 = maybe_fragment.content["length"].clone().parse().unwrap();
                                        if index == length-1{
                                            let mut fragments_data_list = Vec::with_capacity(length as usize);
                                            let mut to_remove = Vec::new();
                                            for _ in 0..length{
                                                fragments_data_list.push(None);
                                            }
                                            for i in 0..fragment_datagrams.len() {
                                                let fragment_datagram = fragment_datagrams[i].clone();
                                                if fragment_datagram.content["digest"] == digest{
                                                    let fragment_index: i32 = fragment_datagram.content["index"].parse().unwrap();
                                                    fragments_data_list[fragment_index as usize] = Some(fragment_datagram.content["data"].clone());
                                                    to_remove.push(i);
                                                }
                                            }
                                            if fragments_data_list.iter().all(|option|option.is_some()){
                                                let mut restored_data = String::from("");
                                                for fragment_data in fragments_data_list{
                                                    restored_data.push_str(fragment_data.unwrap().as_str())
                                                }
                                                let control_datagram_result = serde_json::from_str::<ControlDatagram>(restored_data.as_str());
                                                match control_datagram_result {
                                                    Ok(control_datagram) => {
                                                        actual_datagram = Some(control_datagram);
                                                        for i in to_remove {
                                                            fragment_datagrams.remove(i);
                                                        }
                                                    }
                                                    Err(error) => {
                                                        error!("could not parse control datagram from fragments: {error}")
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                match actual_datagram{
                                    Some(control_datagram) => {
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
                                                } else if !inputs_finished.load(Ordering::Relaxed) {
                                                    match inputs_sender.send(control_datagram.content["id"].to_string()) {
                                                        Ok(_) => {
                                                            info!("sent ACK for inputs handling");
                                                        }
                                                        Err(_) => {
                                                            error!("could not send ACK for handling: {:?}", control_datagram)
                                                        }
                                                    }
                                                } else {
                                                    match datagram_handler_sender.send(control_datagram.clone()) {
                                                        Ok(_) => {}
                                                        Err(_) => {
                                                            let r#type = control_datagram.r#type.as_str().to_string();
                                                            let id = control_datagram.content.get("id").cloned().unwrap_or("None".to_string());
                                                            error!("could not send datagram for handling: {:?}{{id:{}}}", r#type, id);
                                                        }
                                                    }
                                                }
                                            }
                                            _ => {
                                                let id_option = control_datagram.content.get("id");
                                                if control_datagram.r#type == "input_port" {
                                                    match id_option {
                                                        Some(id) => {
                                                            info!("🔻 RECEIVED {}{{id:{}}}", control_datagram.r#type,id);
                                                            send_ack(socket, id).await
                                                        }
                                                        None => {
                                                            warn!("🔻 RECEIVED {} without id. Cannot send ACK.", control_datagram.r#type);
                                                        }
                                                    }
                                                }
                                                let r#type = control_datagram.r#type.as_str().to_string();
                                                let id_option_str = id_option.and_then(|text| Some(text.to_string()));
                                                match datagram_handler_sender.send(control_datagram) {
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
                                    None => {
                                        info!("fragment received: [{}] {}",maybe_fragment.content.get("index").cloned().unwrap_or("index".to_string()), maybe_fragment.content.get("digest").cloned().unwrap_or("digest".to_string()));
                                    }
                                }
                            }
                            _ => {
                                warn!("malformed datagram: '{}'", data);
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
}

async fn send_syn(socket: tokio::net::UdpSocket) {
    let datagram = ControlDatagram::syn();
    let datagram_bytes = serde_json::to_vec(&datagram).unwrap();
    match socket.send(datagram_bytes.as_slice()).await{
        Ok(_) => {}
        Err(error) => {
            error!("Failed to send SYN: {error}")
        }
    }
    info!(" ⃤ SENT SYN");
}

async fn send_ack(socket: tokio::net::UdpSocket, id: &str) {
    let datagram = ControlDatagram::ack(id);
    let datagram_bytes = serde_json::to_vec(&datagram).unwrap();
    match socket.send(datagram_bytes.as_slice()).await{
        Ok(_) => {}
        Err(error) => {
            error!("Failed to send {}: {error}", datagram.r#type)
        }
    }
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
    match socket.send(datagram_bytes.as_slice()).await{
        Ok(_) => {}
        Err(error) => {
            error!("Failed to send {}: {error}", datagram.r#type)
        }
    }
    info!(" ⃤ SENT {}{{id:{}}}",datagram.r#type, datagram.content["id"]);
}

async fn send_datagram(socket: &tokio::net::UdpSocket, datagram: ControlDatagram) {
    let parts_result = ControlDatagram::fragments(datagram.clone());
    match parts_result{
        Ok(parts) => {
            let part_a = serde_json::to_vec(&parts[0]).unwrap();
            let part_b = serde_json::to_vec(&parts[1]).unwrap();
            let mut interval = time::interval(Duration::from_millis(10));
            for _ in 0..3 {
                match socket.send(part_a.as_slice()).await {
                    Ok(_) => {}
                    Err(error) => {
                        error!("Failed to send {} (fragment index 0): {error}", datagram.r#type);
                    }
                }
                match datagram.content.get("protocol"){
                    Some(protocol_str) => {
                        if protocol_str == "tcp"{
                            break;
                        }
                    }
                    None => {}
                }
                interval.tick().await;
            }
            for _ in 0..3{
                match socket.send(part_b.as_slice()).await{
                    Ok(_) => {}
                    Err(error) => {
                        error!("Failed to send {} (fragment index 1): {error}", datagram.r#type);
                    }
                }
                match datagram.content.get("protocol"){
                    Some(protocol_str) => {
                        if protocol_str == "tcp"{
                            break;
                        }
                    }
                    None => {}
                }
                interval.tick().await;
            }
            info!(" ⃤ SENT {}{{id:{}}}",datagram.r#type, datagram.content.get("id").unwrap_or(&"null".to_string()));
        }
        Err(error) => {
            error!("could not split datagram into fragments: {error}")
        }
    }
}