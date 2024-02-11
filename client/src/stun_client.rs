use log::{ info};
use quinn::Endpoint;
use crate::{http3get, subscription_response};

pub async fn subscribe_to_stun(socket: &std::net::UdpSocket)->Result<String,Box<dyn std::error::Error>>{
    let socket_copy = socket.try_clone().unwrap();
    let mut client_endpoint = Endpoint::client("[::]:0".parse().unwrap())?;
    client_endpoint.rebind(socket_copy).expect("Could not rebind the QUIC connection to a existing UDP Socket");

    let subscription_response_str = http3get(&mut client_endpoint, "https://hamesh-stun.blackmidori.com/subscription?version=1").await?;
    let subscription_response = serde_json::from_str::<subscription_response::SubscriptionResponse>(&subscription_response_str).unwrap();

    let subscription_id = subscription_response.value.subscription_id;
    info!("subscribed to stun subscription_id={}", subscription_id);
    return Ok(subscription_id);
}