use crate::{http3get, subscription_response};
use log::info;
use quinn::Endpoint;

pub async fn subscribe_to_stun(
    http3client: &mut Endpoint,
) -> Result<String, Box<dyn std::error::Error>> {
    let subscription_response_str = http3get(
        http3client,
        "https://hamesh-stun.blackmidori.com/subscription",
    )
    .await?;
    let subscription_response =
        serde_json::from_str::<subscription_response::SubscriptionResponse>(
            &subscription_response_str,
        )
        .unwrap();

    let subscription_id = subscription_response.value.subscription_id;
    info!("subscribed to stun subscription_id={}", subscription_id);
    return Ok(subscription_id);
}
