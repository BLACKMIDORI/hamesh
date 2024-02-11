use std::collections::HashMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlDatagram {
    pub version: i32,
    pub r#type: String,
    pub content: HashMap<String, String>,
}