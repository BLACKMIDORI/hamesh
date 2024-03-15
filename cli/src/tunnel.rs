use crate::control_datagram::ControlDatagram;
use crate::fragment_handler::FragmentHandler;
use crate::inbound_client::InboundClient;
use crate::outbound_server::OutboundServer;
use crate::settings_models::{PortSettings, Protocol};
use log::{error, info, warn};
use regex::Regex;
use std::collections::HashMap;
use std::error::Error;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::thread::available_parallelism;
use std::time::{Duration, Instant};

pub struct Tunnel {
    socket: std::net::UdpSocket,
    source_peer: SocketAddr,
    dest_peer: SocketAddr,
}

impl Tunnel {
    pub fn new(
        socket: std::net::UdpSocket,
        source_peer: SocketAddr,
        dest_peer: SocketAddr,
    ) -> Tunnel {
        Tunnel {
            socket,
            source_peer,
            dest_peer,
        }
    }
    pub async fn start(&self) -> Result<(), Box<dyn Error>> {
        let tokio_socket = self.connect().await?;

        self.perform_handshake(&tokio_socket).await;
        let outbound_matches = self.exchange_port_settings(&tokio_socket).await;
        let mut last_outbound_match_handled = None;

        let default_parallelism_approx = available_parallelism().unwrap().get();
        let outbound_server_per_thread =
            outbound_matches.len().div_ceil(default_parallelism_approx);
        let (tunnel_sender, tunnel_receiver) = tokio::sync::mpsc::channel::<ControlDatagram>(1024);
        let mut senders = Vec::with_capacity(default_parallelism_approx);
        let mut servers = HashMap::new();
        let mut main_receiver = None;
        for i in 0..default_parallelism_approx {
            let (sender, receiver) = tokio::sync::mpsc::channel::<ControlDatagram>(1024);
            let outbound_server_sender = sender.clone();
            senders.push(sender);

            if i == 0 {
                main_receiver = Some(receiver);
            } else {
                let mut outbound_servers_port_settings = Vec::new();
                for i_match in 0..outbound_server_per_thread {
                    last_outbound_match_handled = Some(i * outbound_server_per_thread + i_match);
                    match outbound_matches.get(last_outbound_match_handled.unwrap()) {
                        None => {}
                        Some(port_settings) => {
                            let outbound_server = self.create_outbound_server(
                                port_settings,
                                outbound_server_sender.clone(),
                            );
                            let outbound_server_key = (
                                outbound_server.remote_host_port,
                                outbound_server.remote_protocol,
                            );
                            servers.insert(outbound_server_key, outbound_server);
                            outbound_servers_port_settings.push(*port_settings)
                        }
                    }
                }
                tokio::spawn(async move {
                    let mut channel_receiver = receiver;
                    info!("hello, {:?}", std::thread::current().id());
                    for port_settings in outbound_servers_port_settings {
                        OutboundServer::start(port_settings).await;
                    }
                    loop {
                        match channel_receiver.recv().await {
                            _ => {}
                        }
                    }
                });
            }
        }

        let outbound_match_start_index = last_outbound_match_handled
            .and_then(|i| Some(i + 1))
            .unwrap_or(0);
        let mut outbound_servers_port_settings = Vec::new();
        for i in outbound_match_start_index..outbound_matches.len() {
            let port_settings = &outbound_matches[i];
            let outbound_server =
                self.create_outbound_server(port_settings, senders.first().unwrap().clone());
            let outbound_server_key = (
                outbound_server.remote_host_port,
                outbound_server.remote_protocol,
            );
            servers.insert(outbound_server_key, outbound_server);
            outbound_servers_port_settings.push(*port_settings)
        }

        tokio::join!(
            async {
                Self::tunnel_handler(&tokio_socket, tunnel_receiver, &senders, servers).await;
            },
            async {
                let mut receiver = main_receiver.unwrap();
                info!("hello from main, {:?}", std::thread::current().id());
                for port_settings in outbound_servers_port_settings {
                    OutboundServer::start(port_settings).await;
                }
                loop {
                    match receiver.recv().await {
                        _ => {}
                    }
                }
            }
        );
        Ok(())
    }

    async fn connect(&self) -> Result<tokio::net::UdpSocket, Box<dyn Error>> {
        let tokio_socket = tokio::net::UdpSocket::from_std(self.socket.try_clone()?)?;
        tokio_socket.connect(self.dest_peer).await?;
        let source_peer = self.source_peer;
        let dest_peer = self.dest_peer;
        info!("connected {source_peer} <-> {dest_peer}");
        Ok(tokio_socket)
    }
    async fn perform_handshake(&self, tokio_socket: &tokio::net::UdpSocket) {
        let syn_atomic = AtomicBool::new(false);
        let done_atomic = AtomicBool::new(false);
        let (result1, result2) = tokio::join!(
            async {
                let mut interval = tokio::time::interval(Duration::from_secs(1));
                loop {
                    let syn = syn_atomic.load(Ordering::Relaxed);
                    let done = done_atomic.load(Ordering::Relaxed);
                    if syn && done {
                        break;
                    }
                    if !syn {
                        Self::send_datagram(tokio_socket, &ControlDatagram::syn()).await?;
                    }
                    interval.tick().await;
                }
                Ok::<(), Box<dyn Error>>(())
            },
            async {
                loop {
                    let syn = syn_atomic.load(Ordering::Relaxed);
                    let done = done_atomic.load(Ordering::Relaxed);
                    if syn && done {
                        break;
                    }
                    let mut buffer = Vec::with_capacity(65535);
                    match tokio_socket.recv_buf(&mut buffer).await {
                        Err(error) => error!("perform_handshake: {error}"),
                        Ok(size) => {
                            let datagram_result = ControlDatagram::from(size, &buffer);
                            match datagram_result {
                                Err(error) => error!("perform_handshake: {error}"),
                                Ok(datagram) => {
                                    if datagram.version != 1 {
                                        error!(
                                            "🔻 RECEIVED {} with unrecognized version",
                                            datagram.r#type
                                        );
                                        panic!("unrecognized version")
                                    }
                                    info!(
                                        "🔻 RECEIVED {}{}",
                                        datagram.r#type,
                                        datagram
                                            .content
                                            .get("id")
                                            .and_then(|id| { Some(format!("{{id:{id}}}")) })
                                            .unwrap_or(String::from(""))
                                    );
                                    if datagram.r#type != "ACK" {
                                        Self::send_datagram(
                                            tokio_socket,
                                            &ControlDatagram::ack(&datagram),
                                        )
                                        .await?;
                                    }
                                    match datagram.r#type.as_str() {
                                        "ACK" => {
                                            if datagram.content["id"] == "SYN" {
                                                syn_atomic.store(true, Ordering::Relaxed);
                                            }
                                        }
                                        "SYN" => {
                                            done_atomic.store(true, Ordering::Relaxed);
                                        }
                                        _ => {
                                            warn!(
                                                "🔻 SKIPPED {}{}",
                                                datagram.r#type,
                                                datagram
                                                    .content
                                                    .get("id")
                                                    .and_then(|id| { Some(format!("{{id:{id}}}")) })
                                                    .unwrap_or(String::from(""))
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    };
                }
                Ok::<(), Box<dyn Error>>(())
            }
        );
        match (result1, result2) {
            (Err(error1), Err(error2)) => {
                error!("perform_handshake: {error1}");
                error!("perform_handshake: {error2}");
            }
            (Err(error), _) => {
                error!("perform_handshake: {error}");
            }
            (_, Err(error)) => {
                error!("perform_handshake: {error}");
            }
            (Ok(_), Ok(_)) => {
                info!("✅  connection established!")
            }
        }
    }

    async fn exchange_port_settings(
        &self,
        tokio_socket: &tokio::net::UdpSocket,
    ) -> Vec<PortSettings> {
        let (inbound, outbound) = Self::get_settings();
        let current_index_atomic = AtomicUsize::new(0);
        let receiver_stopped_atomic = AtomicBool::new(false);

        let (result1, outbound_matches_result) = tokio::join!(
            async {
                let mut interval = tokio::time::interval(Duration::from_secs(1));
                loop {
                    let current_index = current_index_atomic.load(Ordering::Relaxed);
                    if current_index >= inbound.len()
                        || receiver_stopped_atomic.load(Ordering::Relaxed)
                    {
                        break;
                    }
                    let port_settings = &inbound[current_index];
                    let datagram = ControlDatagram::input_port(
                        &format!("input_port_{}", port_settings),
                        port_settings,
                    );
                    Self::send_datagram(tokio_socket, &datagram).await?;
                    interval.tick().await;
                }
                Ok::<(), Box<dyn Error>>(())
            },
            async {
                let cloned_outbound = outbound.clone();
                let mut outbound_matches = Vec::new();
                let mut interval = tokio::time::interval(Duration::from_secs(1));
                let mut count_timeout = 0;
                loop {
                    let current_index = current_index_atomic.load(Ordering::Relaxed);
                    if current_index >= inbound.len() {
                        if &outbound_matches.len() >= &cloned_outbound.len() {
                            break;
                        } else if count_timeout >= 10 {
                            warn!("timed out waiting for outbound settings matches");
                            break;
                        }
                    }
                    let mut buffer = Vec::with_capacity(65535);
                    match tokio_socket.try_recv_buf(&mut buffer) {
                        Err(error) => match error.kind() {
                            std::io::ErrorKind::WouldBlock => {
                                count_timeout += 1;
                                interval.tick().await;
                            }
                            _ => error!("exchange_port_settings: {}", error.kind()),
                        },
                        Ok(size) => {
                            count_timeout = 0;
                            let datagram_result = ControlDatagram::from(size, &buffer);
                            match datagram_result {
                                Err(error) => error!("exchange_port_settings: {error}"),
                                Ok(datagram) => {
                                    if datagram.version != 1 {
                                        error!(
                                            "🔻 RECEIVED {} with unrecognized version",
                                            datagram.r#type
                                        );
                                        panic!("unrecognized version")
                                    }
                                    info!(
                                        "🔻 RECEIVED {}{}",
                                        datagram.r#type,
                                        datagram
                                            .content
                                            .get("id")
                                            .and_then(|id| { Some(format!("{{id:{id}}}")) })
                                            .unwrap_or(String::from(""))
                                    );
                                    if datagram.r#type != "ACK" {
                                        Self::send_datagram(
                                            tokio_socket,
                                            &ControlDatagram::ack(&datagram),
                                        )
                                        .await?;
                                    }
                                    match datagram.r#type.as_str() {
                                        "ACK" => {
                                            if current_index < inbound.len() {
                                                let port_settings = &inbound[current_index];
                                                if datagram.content["id"]
                                                    == format!("input_port_{}", port_settings)
                                                {
                                                    current_index_atomic
                                                        .fetch_add(1, Ordering::Relaxed);
                                                }
                                            }
                                        }
                                        "input_port" => {
                                            let remote_inbound_settings_result =
                                                Self::parse_remote_inbound_settings(&datagram);
                                            let remote_inbound_settings;
                                            match remote_inbound_settings_result {
                                                Err(error) => {
                                                    error!("{}", error);
                                                    continue;
                                                }
                                                Ok(data) => remote_inbound_settings = data,
                                            }
                                            let matched_option =
                                                &cloned_outbound.iter().find(|outbound_settings| {
                                                    (
                                                        remote_inbound_settings.protocol,
                                                        remote_inbound_settings.remote_host_port,
                                                        remote_inbound_settings.host_port,
                                                    ) == (
                                                        outbound_settings.protocol,
                                                        outbound_settings.host_port,
                                                        outbound_settings.remote_host_port,
                                                    )
                                                });
                                            match matched_option {
                                                None => {
                                                    warn!(
                                                        "UNMATCHED REMOTE INBOUND: {}",
                                                        remote_inbound_settings
                                                    );
                                                }
                                                Some(matched) => {
                                                    if !outbound_matches.contains(*matched) {
                                                        outbound_matches.push(**matched);
                                                    }
                                                }
                                            }
                                        }
                                        _ => {
                                            warn!(
                                                "🔻 SKIPPED {}{}",
                                                datagram.r#type,
                                                datagram
                                                    .content
                                                    .get("id")
                                                    .and_then(|id| { Some(format!("{{id:{id}}}")) })
                                                    .unwrap_or(String::from(""))
                                            );
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    };
                }
                receiver_stopped_atomic.store(true, Ordering::Relaxed);
                Ok::<Vec<PortSettings>, Box<dyn Error>>(outbound_matches)
            }
        );
        match (result1, outbound_matches_result) {
            (Err(error1), Err(error2)) => {
                error!("exchange_port_settings: {error1}");
                error!("exchange_port_settings: {error2}");
                Vec::new()
            }
            (Err(error), _) => {
                error!("exchange_port_settings: {error}");
                Vec::new()
            }
            (_, Err(error)) => {
                error!("exchange_port_settings: {error}");
                Vec::new()
            }
            (Ok(_), Ok(outbound_matches)) => {
                for port_settings in outbound.iter() {
                    let not_match = outbound_matches
                        .iter()
                        .find(|matched_settings| *port_settings == **matched_settings)
                        .is_none();
                    if not_match {
                        warn!("UNMATCHED OUTBOUND: {}", port_settings);
                    }
                }
                info!("🚪<->🚪  port settings done");
                outbound_matches
            }
        }
    }

    fn create_outbound_server(
        &self,
        port_settings: &PortSettings,
        threads_sender: tokio::sync::mpsc::Sender<ControlDatagram>,
    ) -> OutboundServer {
        OutboundServer::new(
            port_settings.host_port,
            port_settings.remote_host_port,
            port_settings.protocol,
            threads_sender,
        )
    }
    async fn tunnel_handler(
        tokio_socket: &tokio::net::UdpSocket,
        mut tunnel_receiver: tokio::sync::mpsc::Receiver<ControlDatagram>,
        threads_senders: &[tokio::sync::mpsc::Sender<ControlDatagram>],
        outbound_servers: HashMap<(u16, Protocol), OutboundServer>,
    ) {
        let senders_length = threads_senders.len();
        let mut next_thread_sender_index = (outbound_servers.len() + 1) % senders_length;
        let mut inbound_clients = HashMap::new();
        let fragment_handler = FragmentHandler::new();
        let (result1, result2) = tokio::join!(
            async {
                loop {
                    match tunnel_receiver.recv().await {
                        None => {
                            warn!("tunnel closed");
                            break;
                        }
                        Some(datagram) => Self::send_datagram(tokio_socket, &datagram).await?,
                    }
                }
                Ok::<(), Box<dyn Error>>(())
            },
            async {
                loop {
                    let mut buffer = Vec::with_capacity(65535);
                    match tokio_socket.recv_buf(&mut buffer).await {
                        Err(error) => error!("tunnel_handler: {error}"),
                        Ok(size) => {
                            let datagram_result = ControlDatagram::from(size, &buffer);
                            match datagram_result {
                                Err(error) => error!("tunnel_handler: {error}"),
                                Ok(received_datagram) => {
                                    if received_datagram.version != 1 {
                                        error!(
                                            "🔻 RECEIVED {} with unrecognized version",
                                            received_datagram.r#type
                                        );
                                        panic!("unrecognized version")
                                    }
                                    info!(
                                        "🔻 RECEIVED {}{}",
                                        received_datagram.r#type,
                                        received_datagram
                                            .content
                                            .get("id")
                                            .and_then(|id| { Some(format!("{{id:{id}}}")) })
                                            .unwrap_or(String::from(""))
                                    );
                                    if received_datagram.r#type != "ACK" {
                                        Self::send_datagram(
                                            tokio_socket,
                                            &ControlDatagram::ack(&received_datagram),
                                        )
                                        .await?;
                                    }
                                    let datagram = match received_datagram.r#type.as_str() {
                                        "fragment" => {
                                            match fragment_handler
                                                .get_complete_datagram(received_datagram)
                                            {
                                                None => continue,
                                                Some(data) => data,
                                            }
                                        }
                                        _ => received_datagram,
                                    };
                                    match datagram.r#type.as_str() {
                                        "ACK" => (),
                                        "client_data" => {
                                            let future = async {
                                                let remote_host_port: u16 =
                                                    datagram.content["hostClientPort"].parse()?;
                                                let remote_client_port: u16 =
                                                    datagram.content["hostClientPort"].parse()?;
                                                let remote_protocol: Protocol =
                                                    datagram.content["protocol"].parse()?;
                                                let key = (
                                                    remote_host_port,
                                                    remote_client_port,
                                                    remote_protocol,
                                                );
                                                if !inbound_clients.contains_key(&key) {
                                                    let sender = threads_senders
                                                        [next_thread_sender_index]
                                                        .clone();
                                                    next_thread_sender_index =
                                                        (next_thread_sender_index + 1)
                                                            % senders_length;
                                                    inbound_clients.insert(
                                                        key,
                                                        InboundClient::new(
                                                            remote_host_port,
                                                            remote_client_port,
                                                            remote_protocol,
                                                            sender,
                                                        ),
                                                    );
                                                }
                                                let inbound_client = &inbound_clients[&key];
                                                inbound_client.last_download_activity.store(
                                                    Instant::now().elapsed().as_secs(),
                                                    Ordering::Relaxed,
                                                );
                                                inbound_client.sender.send(datagram).await?;
                                                Ok::<(), Box<dyn Error>>(())
                                            };
                                            match future.await {
                                                Err(error) => {
                                                    error!("tunnel_handler(client_data): {error}");
                                                }
                                                _ => (),
                                            }
                                        }
                                        "server_data" => {
                                            let future = async {
                                                let local_client_port: u16 = datagram.content
                                                    ["remoteHostClientPort"]
                                                    .parse()?;
                                                let local_protocol: Protocol =
                                                    datagram.content["protocol"].parse()?;
                                                let key = (local_client_port, local_protocol);
                                                match outbound_servers.get(&key) {
                                                    None => {
                                                        warn!("received a server_data for unknown outbound server: {local_client_port}/{local_protocol}")
                                                    }
                                                    Some(outbound_server) => {
                                                        outbound_server
                                                            .sender
                                                            .send(datagram)
                                                            .await?;
                                                    }
                                                }
                                                Ok::<(), Box<dyn Error>>(())
                                            };
                                            match future.await {
                                                Err(error) => {
                                                    error!("tunnel_handler(server_data): {error}");
                                                }
                                                _ => (),
                                            }
                                        }
                                        "input_port" => {
                                            let remote_inbound_settings_result =
                                                Self::parse_remote_inbound_settings(&datagram);
                                            let remote_inbound_settings;
                                            match remote_inbound_settings_result {
                                                Err(error) => {
                                                    error!("{}", error);
                                                    continue;
                                                }
                                                Ok(data) => remote_inbound_settings = data,
                                            };
                                            warn!(
                                                "UNMATCHED REMOTE INBOUND: {}",
                                                remote_inbound_settings,
                                            );
                                        }
                                        _ => {
                                            warn!(
                                                "🔻 SKIPPED {}{}",
                                                datagram.r#type,
                                                datagram
                                                    .content
                                                    .get("id")
                                                    .and_then(|id| { Some(format!("{{id:{id}}}")) })
                                                    .unwrap_or(String::from(""))
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    };
                }
                Ok::<(), Box<dyn Error>>(())
            }
        );
        match result1 {
            Ok(_) => {}
            Err(error) => {
                error!("tunnel_handler: {error}");
            }
        };
        match result2 {
            Ok(_) => {}
            Err(error) => {
                error!("tunnel_handler: {error}");
            }
        };
        info!("tunnel handler stopped.")
    }

    fn get_settings() -> (Vec<PortSettings>, Vec<PortSettings>) {
        let args: Vec<String> = std::env::args().collect();
        let joined = args[1..].join(" ");
        let regex = Regex::new(r"--(?:inbound|outbound) \d+:\d+/(?:tcp|udp)").unwrap();

        let mut inbound_settings = Vec::new();
        let mut outbound_settings = Vec::new();
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
                _ => continue,
            };
            match input_or_output {
                "--inbound" => {
                    inbound_settings.push(PortSettings {
                        protocol,
                        host_port,
                        remote_host_port,
                    });
                }
                "--outbound" => {
                    outbound_settings.push(PortSettings {
                        protocol,
                        host_port,
                        remote_host_port,
                    });
                }
                _ => continue,
            }
        }

        (inbound_settings, outbound_settings)
    }

    fn parse_remote_inbound_settings(datagram: &ControlDatagram) -> Result<PortSettings, String> {
        let protocol_str = datagram.content["protocol"].as_str();
        let host_port_str = datagram.content["hostPort"].as_str();
        let remote_host_port_str = datagram.content["remoteHostPort"].as_str();
        Ok(PortSettings {
            protocol: match protocol_str {
                "tcp" => Protocol::Tcp,
                "udp" => Protocol::Udp,
                _ => {
                    return Err(format!("invalid protocol {}", protocol_str));
                }
            },
            host_port: host_port_str.parse().unwrap(),
            remote_host_port: remote_host_port_str.parse().unwrap(),
        })
    }

    async fn send_datagram(
        tokio_socket: &tokio::net::UdpSocket,
        datagram: &ControlDatagram,
    ) -> Result<(), Box<dyn Error>> {
        tokio_socket.send(&datagram.to_vec()?).await?;
        info!(
            " ⃤ SENT {}{}",
            datagram.r#type,
            datagram
                .content
                .get("id")
                .and_then(|id| { Some(format!("{{id:{id}}}")) })
                .unwrap_or(String::from(""))
        );
        Ok(())
    }
}
