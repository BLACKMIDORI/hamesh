use crate::control_datagram::ControlDatagram;
use crate::fragment_handler::FragmentHandler;
use crate::inbound_client::InboundClient;
use crate::outbound_server::OutboundServer;
use crate::settings_models::{PortSettings, Protocol};
use futures::future::err;
use futures::{future, stream, TryFutureExt};
use log::{error, info, warn};
use regex::Regex;
use std::collections::HashMap;
use std::error::Error;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::thread;
use std::thread::available_parallelism;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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

        let (to_tunnel_sender, to_tunnel_receiver) =
            tokio::sync::mpsc::channel::<ControlDatagram>(1024);
        let (from_tunnel_ack_sender, _) = tokio::sync::broadcast::channel::<String>(1024);
        let (from_tunnel_client_data_sender, mut from_tunnel_client_data_receiver) =
            tokio::sync::mpsc::channel::<ControlDatagram>(1024);
        let (from_tunnel_server_data_sender, mut from_tunnel_server_data_receiver) =
            tokio::sync::mpsc::channel::<ControlDatagram>(1024);

        let mut to_outbound_senders = HashMap::new();
        let from_tunnel_ack_sender_clone = from_tunnel_ack_sender.clone();
        tokio::join!(
            async {
                // handle tunnel
                Self::tunnel_handler(
                    &tokio_socket,
                    to_tunnel_receiver,
                    from_tunnel_ack_sender,
                    from_tunnel_client_data_sender,
                    from_tunnel_server_data_sender,
                )
                .await;
                Ok::<(), String>(())
            }
            .map_err(|e| {
                error!("start.join.0: {e}");
                e
            }),
            async {
                // handle local client_data and remote server_data
                let mut server_futures = Vec::new();
                for port_settings in outbound_matches {
                    let (to_outbound_server_sender, to_outbound_server_receiver) =
                        tokio::sync::mpsc::channel::<ControlDatagram>(1024);
                    to_outbound_senders.insert(
                        (port_settings.host_port, port_settings.protocol),
                        to_outbound_server_sender,
                    );
                    server_futures.push(OutboundServer::new(port_settings).start(
                        to_outbound_server_receiver,
                        to_tunnel_sender.clone(),
                        from_tunnel_ack_sender_clone.clone(),
                    ));
                }
                tokio::join!(futures::future::join_all(server_futures), async {
                    loop {
                        let server_data = from_tunnel_server_data_receiver
                            .recv()
                            .await
                            .ok_or("failed from_tunnel_server_data_receiver.recv()")?;
                        let local_outbound_server_port: u16 =
                            server_data.content["remoteHostPort"].parse().map_err(|e|format!("{e}"))?;
                        let local_protocol: Protocol =
                            server_data.content["protocol"].parse()?;
                        let key =
                            (local_outbound_server_port, local_protocol);
                        match to_outbound_senders.get(&key) {
                            None => {
                                warn!("received a server_data for unknown outbound server: {local_outbound_server_port}/{local_protocol}")
                            }
                            Some(outbound_sender) => {
                                outbound_sender
                                .send(server_data)
                                .await
                                .map_err(|e|{format!("{e}")})?;
                            }
                        }
                    }
                    Ok::<(), String>(())
                });
                Ok::<(), String>(())
            }
            .map_err(|e| {
                error!("start.join.1: {e}");
                e
            }),
            async {
                // handle remote client_data and local server_data
                let mut inbound_clients_mutex = Mutex::new(HashMap::<
                    (u16, u16, Protocol),
                    (tokio::sync::mpsc::Sender<ControlDatagram>, AtomicU64),
                >::new());
                tokio::join!(
                    async {
                        const ONE_HOUR_IN_SECONDS : u64 = 3600;
                        const TWO_HOURS_IN_SECONDS : u64 = 2 * ONE_HOUR_IN_SECONDS;
                        let mut interval = tokio::time::interval(Duration::from_secs(ONE_HOUR_IN_SECONDS));
                        loop {
                            interval.tick().await;
                            let mut inbound_clients = inbound_clients_mutex.lock().unwrap();
                            let total_clients = inbound_clients.len();
                            if total_clients > 0 {
                                info!("Running stopped clients cleanup");
                                let now = SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .map_err(|e| format!("{e}"))?
                                    .as_secs();
                                const TIMEOUT_IN_SECONDS: u64 = TWO_HOURS_IN_SECONDS;
                                let mut to_remove = Vec::new();
                                for (key, (inbound_client_sender, last_download_activity)) in
                                    inbound_clients.iter()
                                {
                                    if now - last_download_activity.load(Ordering::Relaxed)
                                        > TIMEOUT_IN_SECONDS
                                    {
                                        let mut content = HashMap::new();
                                        content.insert(String::from("hostPort"), key.0.to_string());
                                        content.insert(
                                            String::from("hostClientPort"),
                                            key.1.to_string(),
                                        );
                                        content.insert(
                                            String::from("protocol"),
                                            match key.2 {
                                                Protocol::Tcp => "tcp".to_string(),
                                                Protocol::Udp => "udp".to_string(),
                                            },
                                        );
                                        inbound_client_sender
                                            .send(ControlDatagram {
                                                version: -1,
                                                r#type: "close".to_string(),
                                                content,
                                            })
                                            .await
                                            .map_err(|e| format!("{e}"))?;
                                        to_remove.push(key.clone());
                                    }
                                }
                                for key in &to_remove {
                                    inbound_clients.remove(key);
                                }
                                info!(
                                    "previous clients = {}; cleared = {}; current clients = {}",
                                    total_clients,
                                    to_remove.len(),
                                    inbound_clients.len()
                                )
                            }
                        }
                        Ok::<(), String>(())
                    }
                    .map_err(|e| {
                        error!("start.join.2.join.0: {e}");
                        e
                    }),
                    async {
                        let (inbound_port_settings,_)=Self::get_settings();
                        loop {
                            let datagram = from_tunnel_client_data_receiver
                                .recv()
                                .await
                                .ok_or("failed from_tunnel_client_data_receiver.recv()")?;
                            let remote_host_port: u16 = datagram.content["hostPort"]
                                .parse()
                                .map_err(|e| format!("{e}"))?;
                            let remote_client_port: u16 = datagram.content["hostClientPort"]
                                .parse()
                                .map_err(|e| format!("{e}"))?;
                            let remote_protocol: Protocol = datagram.content["protocol"]
                                .parse()
                                .map_err(|e| format!("{e}"))?;

                            let port_settings = match inbound_port_settings.iter().find(|settings|{settings.protocol == remote_protocol && settings.remote_host_port == remote_host_port}){
                                None => {
                                warn!(
                                        "[SKIPPED] Datagram does not match any inbound settings: {}/{}",
                                        remote_host_port,
                                        match remote_protocol{Protocol::Tcp => {"tcp"}Protocol::Udp => {"udp"}},
                                    );
                                continue;
                                }
                                Some(port_settings) => {port_settings}
                            };

                            let key = (remote_host_port, remote_client_port, remote_protocol);
                            let mut inbound_clients = inbound_clients_mutex.lock().unwrap();
                            if !inbound_clients.contains_key(&key) {
                                if datagram.content["sequence"] != "0" {
                                    warn!("[SKIPPED] Cannot find an existent inbound client and cannot create a new one since the datagram sequence is not 0.");
                                    continue;
                                }
                                let (to_inbound_client_sender, to_inbound_client_receiver) =
                                    tokio::sync::mpsc::channel::<ControlDatagram>(1024);
                                inbound_clients
                                    .insert(key, (to_inbound_client_sender, AtomicU64::new(0)));
                                tokio::spawn(
                                    InboundClient::new(port_settings.clone(),remote_client_port)
                                    .start(
                                        to_inbound_client_receiver,
                                        to_tunnel_sender.clone(),
                                        from_tunnel_ack_sender_clone.clone(),
                                    ).map_err(|e|{
                                        error!("InboundClient.start(): {}",e);
                                        e
                                    }),
                                );
                            }
                            let (inbound_client_sender, last_download_activity) =
                                &inbound_clients[&key];
                            last_download_activity.store(
                                SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .map_err(|e| format!("{e}"))?
                                    .as_secs(),
                                Ordering::Relaxed,
                            );
                            inbound_client_sender
                                .send(datagram)
                                .await
                                .map_err(|e| format!("{e}"))?;
                        }
                        Ok::<(), String>(())
                    }
                    .map_err(|e| {
                        error!("start.join.2.join.1: {e}");
                        e
                    })
                );
            }
        );
        Ok(())
    }

    async fn connect(&self) -> Result<tokio::net::UdpSocket, Box<dyn Error>> {
        let tokio_socket = tokio::net::UdpSocket::from_std(self.socket.try_clone()?)?;
        let source_peer = self.source_peer;
        let dest_peer = self.dest_peer;
        info!("connecting {source_peer} <-> {dest_peer}");
        tokio_socket.connect(self.dest_peer).await?;
        info!("connected!");
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
            }
            .map_err(|e| {
                error!("{e}");
                e
            }),
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
            .map_err(|e| {
                error!("{e}");
                e
            })
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
            }
            .map_err(|e| {
                error!("{e}");
                e
            }),
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
                            warn!("timed out(10s) waiting for outbound settings matches");
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
                                                    error!("Self::parse_remote_inbound_settings(&datagram): {error}");
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
            .map_err(|e| {
                error!("{e}");
                e
            })
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

    async fn tunnel_handler(
        tokio_socket: &tokio::net::UdpSocket,
        mut to_tunnel_receiver: tokio::sync::mpsc::Receiver<ControlDatagram>,
        from_tunnel_ack_sender: tokio::sync::broadcast::Sender<String>,
        from_tunnel_client_data_sender: tokio::sync::mpsc::Sender<ControlDatagram>,
        from_tunnel_server_data_sender: tokio::sync::mpsc::Sender<ControlDatagram>,
    ) {
        let (result1, result2) = tokio::join!(
        async {
        loop {
            match to_tunnel_receiver.recv().await {
                None => {
                    warn!("tunnel closed");
                    break;
                        }
                        Some(datagram) => {
                            let r#type = datagram.r#type.clone();
                            let id = datagram
                                .content
                                .get("id")
                                .and_then(|id| { Some(format!("{{id:{id}}}")) })
                                .unwrap_or(String::from(""));
                            let fragments = ControlDatagram::fragments(datagram)?;
                            Self::send_datagram(tokio_socket, &fragments[0]).await?;
                            Self::send_datagram(tokio_socket, &fragments[1]).await?;
                            info!(
                                "🎁 SENT IN FRAGMENTS {}{}",
                                r#type,
                                id,
                            );
                        },
                    }
                }
                Ok::<(), Box<dyn Error>>(())
            }
            .map_err(|e| {
                error!("tunnel_handler.join.0: {e}");
                e
            }),
            async {
                let mut buffer = Vec::with_capacity(65535);
                let mut fragment_handler = FragmentHandler::new();
                loop {
                    buffer.clear();
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
                                    if received_datagram.r#type == "fragment"{
                                        info!(
                                            "🔻 RECEIVED fragment({}/{})",
                                            received_datagram
                                            .content
                                            .get("index")
                                            .and_then(|index|Some(index.parse::<isize>().and_then(|i|Ok(i+1))))
                                            .unwrap_or(Ok(-1))?,
                                            received_datagram
                                            .content
                                            .get("length")
                                            .unwrap_or(&String::from(""))
                                        );
                                    }else{
                                        info!(
                                            "🔻 RECEIVED {}{}",
                                            received_datagram.r#type,
                                            received_datagram
                                            .content
                                            .get("id")
                                            .and_then(|id| { Some(format!("{{id:{id}}}")) })
                                            .unwrap_or(String::from(""))
                                        );
                                    }
                                    let datagram = match received_datagram.r#type.as_str() {
                                        "fragment" => {
                                            match fragment_handler
                                                .get_complete_datagram(received_datagram)
                                            {
                                                None => continue,
                                                Some(data) => {
                                                    info!("🧩 FROM FRAGMENTS {}{}",
                                                        data.r#type,
                                                        data
                                                        .content
                                                        .get("id")
                                                        .and_then(|id| { Some(format!("{{id:{id}}}")) })
                                                        .unwrap_or(String::from(""))
                                                    );
                                                    data
                                                },
                                            }
                                        }
                                        _ => received_datagram,
                                    };
                                    if datagram.r#type != "ACK" {
                                        Self::send_datagram(
                                            tokio_socket,
                                            &ControlDatagram::ack(&datagram),
                                        )
                                        .await?;
                                    }
                                    match datagram.r#type.as_str() {
                                        "ACK" => {
                                            match from_tunnel_ack_sender
                                                .send(datagram.content["id"].clone()).map_err(|e|format!("ACK: {e}")){
                                                Err(e) => {
                                                    error!("{e}");
                                                }
                                                Ok(_) => {}
                                            }
                                        }
                                        "client_data" => {
                                            from_tunnel_client_data_sender.send(datagram).await.map_err(|e|format!("client_data: {e}"))?;
                                        }
                                        "server_data" => {
                                            from_tunnel_server_data_sender.send(datagram).await.map_err(|e|format!("server_data: {e}"))?;
                                        }
                                        "input_port" => {
                                            let remote_inbound_settings_result =
                                                Self::parse_remote_inbound_settings(&datagram);
                                            let remote_inbound_settings;
                                            match remote_inbound_settings_result {
                                                Err(error) => {
                                                    error!("Self::parse_remote_inbound_settings(&datagram): {error}");
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
           .map_err(|e| {
                error!("tunnel_handler.join.1: {e}");
                e
            })
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
        if datagram.r#type == "fragment" {
            info!(
                " ⃤ SENT fragment({}/{})",
                datagram
                    .content
                    .get("index")
                    .and_then(|index| Some(index.parse::<isize>().and_then(|i| Ok(i + 1))))
                    .unwrap_or(Ok(-1))?,
                datagram.content.get("length").unwrap_or(&String::from(""))
            );
        } else {
            info!(
                " ⃤ SENT {}{}",
                datagram.r#type,
                datagram
                    .content
                    .get("id")
                    .and_then(|id| { Some(format!("{{id:{id}}}")) })
                    .unwrap_or(String::from(""))
            );
        }
        Ok(())
    }
}
