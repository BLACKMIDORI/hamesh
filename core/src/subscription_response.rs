use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionResponse {
    pub value: SubscriptionResponseValue,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionResponseValue {
    pub source_address: String,
    pub source_port: i32,
    pub subscription_id: String,
}
