mod subscription_response;
mod subscription_view_response;
mod control_datagram;
mod tunnel;
mod stun_client;
mod settings_models;
mod ip_version;
mod output_server;
mod input_client;
mod get_ip_version;

use std::{io};
use std::io::{Read};
use std::sync::Arc;
use std::net::{SocketAddr, SocketAddrV6};
use std::str::FromStr;
use std::time::Duration;
use regex::Regex;
use bytes::Buf;

use futures::{future};
// use structopt::StructOpt;
use log::{error, info, LevelFilter, warn};

use h3_quinn::quinn;
use http::Uri;
use quinn::Endpoint;
use simple_logger::SimpleLogger;
use tokio::time;
use crate::tunnel::establish_connection;
use crate::ip_version::IpVersion;
use crate::stun_client::subscribe_to_stun;
use crate::subscription_view_response::SubscriptionPeer;

static ALPN: &[u8] = b"h3";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    SimpleLogger::new()
        .with_level(LevelFilter::Info)
        .init().unwrap();
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        show_help_message();
        return Ok(());
    }
    let joined = format!("hamash {}", args[1..].join(" "));
    let regex = Regex::new(r"^hamash p2p (?:ipv4|ipv6)(?: --(?:input|output) \d+:\d+/(?:tcp|udp))*$").unwrap();
    if !regex.is_match(joined.as_str()) {
        show_help_message();
        return Ok(());
    }
    if !joined.contains("--input") && !joined.contains("--output") {
        print!("Provide at least one --input or --output argument");
        return Ok(());
    }
    let ip_version_str = &args[2];
    let ip_version = match ip_version_str.as_str() {
        "ipv4" => { IpVersion::Ipv4 }
        "ipv6" => { IpVersion::Ipv6 }
        _ => {
            error!("invalid ip version: {}",ip_version_str);
            return Ok(());
        }
    };
    let socket = &match ip_version {
        IpVersion::Ipv4 => { std::net::UdpSocket::bind("0.0.0.0:0") }
        IpVersion::Ipv6 => { std::net::UdpSocket::bind("[::]:0") }
    }.unwrap();
    let mut client_endpoint = match ip_version {
        IpVersion::Ipv4 => { Endpoint::client("0.0.0.0:0".parse().unwrap())? }
        IpVersion::Ipv6 => { Endpoint::client("[::]:0".parse().unwrap())? }
    };
    client_endpoint.rebind(socket.try_clone().unwrap()).expect("Could not rebind the QUIC connection to a existing UDP Socket");

    let subscription_result = subscribe_to_stun(socket).await;
    let input_result = tokio::spawn(read_peer_subscription());
    let subscription_id = subscription_result?;
    static mut ALREADY_CONNECTED: bool = false;
    let mut client_clone = client_endpoint.clone();
    tokio::join!(
         async{
            let result = async {
                  let mut interval = time::interval(Duration::from_secs(1));
            interval.tick().await;
        loop {
                    unsafe{
                    if ALREADY_CONNECTED{
                        return Ok::<bool, ()>(false);
                    }
                    }
            let subscription_view_response_str_result = http3get(&mut client_clone, format!("https://hamesh-stun.blackmidori.com/subscription/{subscription_id}").as_str()).await;

            match subscription_view_response_str_result {
                Ok(subscription_view_response_str) => {
                    let subscription_response = serde_json::from_str::<subscription_view_response::SubscriptionViewResponse>(&subscription_view_response_str).unwrap();
                    if subscription_response.value.peers.is_empty() {
                        break;
                    } else if subscription_response.value.peers.len() == 2 {
                        let peer_a = &subscription_response.value.peers[0];
                        let peer_b = &subscription_response.value.peers[1];
                                tokio::join!(connect_peers(socket,peer_a.clone(), peer_b.clone()));
                        return Ok(true);
                    }
                }
                Err(e) => {
                    error!("error polling subscription: {:?}", e)
                }
            }
            interval.tick().await;
        }
        info!("subscription expired!");
        return Ok(false);
            }.await;
            match result{
                Ok(connected)=>unsafe{
                    if connected{
                        ALREADY_CONNECTED = true
                    }
                },
                Err(..)=>{
                    error!("failed waiting peers to connect.")
                }
            }
         },
        async{

    let subscription_id = input_result.await.unwrap().unwrap();
            unsafe{

    if  ALREADY_CONNECTED {
        warn!("already connected!")
    } else {
                    ALREADY_CONNECTED = true;
        http3get(&mut client_endpoint, format!("https://hamesh-stun.blackmidori.com/join/{subscription_id}").as_str()).await.unwrap();
        let subscription_view_response_str = http3get(&mut client_endpoint, format!("https://hamesh-stun.blackmidori.com/subscription/{subscription_id}").as_str()).await.unwrap();

        let subscription_response = serde_json::from_str::<subscription_view_response::SubscriptionViewResponse>(&subscription_view_response_str).unwrap();

        if subscription_response.value.peers.len() < 2 {
            error!("after we joined, we didn't find 2 peers in this subscription. It may be expired.")
        } else {
            let peer_a = &subscription_response.value.peers[0];
            let peer_b = &subscription_response.value.peers[1];
            tokio::join!(connect_peers(socket, peer_b,peer_a));
        }
    }
            }
        }
    );

    Ok(())
}

fn show_help_message() {
    print!("\
Usage: hamesh p2p <ip_version: ipv4|ipv6> [--input <host_port>:<remote_host_port>/<protocol: tcp|udp>...] [--output <host_port>:<remote_host_port>/<protocol: tcp|udp>...]\n\
\n\
Server Example: hamesh p2p ipv6 --input 25565:1234/tcp\n\
Client Example: hamesh p2p ipv6 --output 1234:25565/tcp\
")
}

async fn connect_peers(socket: &std::net::UdpSocket, source_peer: &SubscriptionPeer, dest_peer: &SubscriptionPeer) {
    let source_address;
    let dest_address;
    if source_peer.address.contains(".") {
        source_address = SocketAddr::from_str(format!("{}:{}", source_peer.address, source_peer.port).as_str()).unwrap()
    } else {
        source_address = SocketAddr::from_str(format!("[{}]:{}", source_peer.address, source_peer.port).as_str()).unwrap()
    }
    if dest_peer.address.contains(".") {
        dest_address = SocketAddr::from_str(format!("{}:{}", dest_peer.address, dest_peer.port).as_str()).unwrap()
    } else {
        dest_address = SocketAddr::from_str(format!("[{}]:{}", dest_peer.address, dest_peer.port).as_str()).unwrap()
    }
    establish_connection(socket.try_clone().unwrap(), source_address, dest_address).await;
}


async fn read_peer_subscription() -> Result<String, io::Error> {
    info!("enter a peer subscription id:");
    Ok(io::stdin().lines().next().unwrap().unwrap())
}


async fn http3get(client_endpoint: &mut Endpoint, url: &str) -> Result<String, Box<dyn std::error::Error>> {
    // DNS lookup
    let uri = url.parse::<http::Uri>()?;

    if uri.scheme() != Some(&http::uri::Scheme::HTTPS) {
        Err("uri scheme must be 'https'")?;
    }

    let auth = uri.authority().ok_or("uri must have a host")?.clone();

    let port = auth.port_u16().unwrap_or(443);

    let args: Vec<String> = std::env::args().collect();
    let ip_version_str = &args[2];
    let ip_version = match ip_version_str.as_str() {
        "ipv4" => { IpVersion::Ipv4 }
        "ipv6" => { IpVersion::Ipv6 }
        _ => {
            error!("invalid ip version: {}",ip_version_str);
            IpVersion::Ipv6
        }
    };
    let addresses =tokio::net::lookup_host((auth.host(), port))
        .await?;
    let mut address_option = None;
    for socket_address in addresses {
        match socket_address {
            SocketAddr::V4(_)=>{
                match ip_version{
                    IpVersion::Ipv4 => {
                        address_option = Some(socket_address);
                        break;
                    }
                    _ => {}
                }
            }
            SocketAddr::V6(_)=>{
                match ip_version{
                    IpVersion::Ipv6 => {
                        address_option = Some(socket_address);
                        break;
                    }
                    _ => {}
                }
            }
        }
    }
    let address = address_option.ok_or("dns found no addresses")?;

    // info!("DNS lookup for {:?}: {:?}", uri, addr);

    // create quinn client endpoint

    // load CA certificates stored in the system
    let mut roots = rustls::RootCertStore::empty();
    match rustls_native_certs::load_native_certs() {
        Ok(certs) => {
            for cert in certs {
                if let Err(e) = roots.add(&rustls::Certificate(cert.0)) {
                    error!("failed to parse trust anchor: {}", e);
                }
            }
        }
        Err(e) => {
            error!("couldn't load any default trust roots: {}", e);
        }
    };

    let mut tls_config = rustls::ClientConfig::builder()
        .with_safe_default_cipher_suites()
        .with_safe_default_kx_groups()
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_root_certificates(roots)
        .with_no_client_auth();

    tls_config.enable_early_data = true;
    tls_config.alpn_protocols = vec![ALPN.into()];


    let client_config = quinn::ClientConfig::new(Arc::new(tls_config));
    client_endpoint.set_default_client_config(client_config);

    let body = run_client(&client_endpoint, address, uri).await;

    // wait for the connection to be closed before exiting
    client_endpoint.wait_idle().await;

    body
}

async fn run_client(client_endpoint: &Endpoint, addr: SocketAddr, uri: Uri) -> Result<String, Box<dyn std::error::Error>> {
    let auth = uri.authority().ok_or("uri must have a host")?.clone();

    let conn = client_endpoint.connect(addr, auth.host())?.await?;

    // info!("QUIC connection established");

    // create h3 client

    // h3 is designed to work with different QUIC implementations via
    // a generic interface, that is, the [`quic::Connection`] trait.
    // h3_quinn implements the trait w/ quinn to make it work with h3.
    let quinn_conn = h3_quinn::Connection::new(conn);

    let (mut driver, mut send_request) = h3::client::new(quinn_conn).await?;

    let drive = async move {
        future::poll_fn(|cx| driver.poll_close(cx)).await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    };

    // In the following block, we want to take ownership of `send_request`:
    // the connection will be closed only when all `SendRequest`s instances
    // are dropped.
    //
    //             So we "move" it.
    //                  vvvv
    let request = async move {
        // info!("sending request ...");

        let req = http::Request::builder().uri(uri).header("Accept-Version","1").body(())?;

        // sending request results in a bidirectional stream,
        // which is also used for receiving response
        let mut stream = send_request.send_request(req).await?;

        // finish on the sending side
        stream.finish().await?;

        // info!("receiving response ...");

        let response = stream.recv_response().await?;

        // info!("headers: {:#?}", resp.headers());

        let mut buffer = [0; 1024];
        let mut buffer_size = 0;
        // `recv_data()` must be called after `recv_response()` for
        // receiving potential response body
        while let Some(mut chunk) = stream.recv_data().await? {
            let mut remaining = chunk.remaining();
            while remaining > 0 {
                buffer_size += remaining;
                let bytes = chunk.copy_to_bytes(remaining);
                bytes.reader().read(&mut buffer).unwrap();
                remaining = chunk.remaining();
            }
        }

        let body = std::str::from_utf8(&buffer[..buffer_size]).unwrap().to_string();


        let status = response.status();
        if !status.is_success() {
            warn!("response: {:?} {}", response.version(), response.status());
            warn!("body: {}", body);
        }

        Ok::<_, Box<dyn std::error::Error>>(body)
    };

    let (req_res, drive_res) = tokio::join!(request, drive);
    let body = req_res?;
    drive_res?;
    Ok(body)
}