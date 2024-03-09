use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionViewResponse {
    pub value: SubscriptionViewResponseValue,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionViewResponseValue {
    pub peers: Vec<SubscriptionPeer>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionPeer {
    pub address: String,
    pub port: i32,
}
