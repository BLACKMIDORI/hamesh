
pub mod get_ip_version;
use regex::Regex;
use std::error::Error;
use std::io;
use std::io::Read;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use log::{error, info, warn, LevelFilter};

use crate::get_ip_version::get_ip_version;
use hamesh::ip_version::IpVersion;
use hamesh::{connect_peers, get_peers, get_socket, get_subscription, read_peer_subscription_from_stdin};
use hamesh::stun_client::subscribe_to_stun;
use hamesh::subscription_view_response::SubscriptionPeer;
use hamesh::tunnel::Tunnel;
use hamesh::version::VERSION;
use simple_logger::SimpleLogger;
use tokio::sync::Mutex;
use tokio::time;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    SimpleLogger::new()
        .with_level(LevelFilter::Info)
        .init()
        .unwrap();
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        show_help_message();
        return Ok(());
    }
    let joined = format!("hamash {}", args[1..].join(" "));
    let regex =
        Regex::new(r"^hamash p2p (?:ipv4|ipv6)(?: --(?:inbound|outbound) \d+:\d+/(?:tcp|udp))*$")
            .unwrap();
    if !regex.is_match(joined.as_str()) {
        show_help_message();
        return Ok(());
    }
    if !joined.contains("--inbound") && !joined.contains("--outbound") {
        print!("Provide at least one --inbound or --outbound argument");
        return Ok(());
    }
    info!("v{}",VERSION);
    let ip_version = get_ip_version().ok_or("failed get_ip_version()")?;
    let socket = get_socket(ip_version).await;
    let peers_result = {
        let (peer_subscription_id_sender,peer_subscription_id_receiver) = tokio::sync::mpsc::channel(1024);
        let peer_subscription_id_sender_clone = peer_subscription_id_sender.clone();
        let handler= tokio::spawn(async move{
            let peer_subscription_id_result = read_peer_subscription_from_stdin().await;
            match peer_subscription_id_result{
                Ok(peer_subscription_id) => {
                    _=peer_subscription_id_sender_clone.send(Some(peer_subscription_id)).await;
                }
                Err(_) => {}
            }
        });
        let (client_endpoint, subscription_id) = get_subscription(&socket).await?;
        let result = get_peers(subscription_id,client_endpoint, peer_subscription_id_receiver).await;
        _=peer_subscription_id_sender.send(None).await;
        handler.abort();
        result
    };

    match peers_result {
        Err(error) => {
            error!("{}",error);
        }
        Ok(peers) => {
            connect_peers(socket, &peers.0, &peers.1).await?
        }
    }
    Ok(())
}

fn show_help_message() {
    print!("\
Usage: hamesh p2p <ip_version: ipv4|ipv6> [--inbound <host_port>:<remote_host_port>/<protocol: tcp|udp>...] [--outbound <host_port>:<remote_host_port>/<protocol: tcp|udp>...]\n\
\n\
Server Example: hamesh p2p ipv6 --inbound 25565:1234/tcp\n\
Client Example: hamesh p2p ipv6 --outbound 1234:25565/tcp\
")
}
