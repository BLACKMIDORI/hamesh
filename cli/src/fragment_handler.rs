use crate::control_datagram::ControlDatagram;
use log::{info, warn};
use std::collections::HashMap;
use std::sync::Mutex;

pub struct FragmentHandler {
    fragments: HashMap<(i32, String), ControlDatagram>,
}

impl FragmentHandler {
    pub fn new() -> FragmentHandler {
        FragmentHandler {
            fragments: HashMap::new(),
        }
    }
    pub fn get_complete_datagram(&mut self, fragment: ControlDatagram) -> Option<ControlDatagram> {
        let content_type = &fragment.content["contentType"];
        if content_type != "control_datagram" {
            warn!("cannot handle fragment content type = {content_type}");
        } else {
            let index = fragment.content["index"].parse::<i32>().ok()?;
            let length = fragment.content["length"].parse::<i32>().ok()?;
            let digest = fragment.content["digest"].clone();
            let key = (index, digest.clone());
            self.fragments.insert(key, fragment);
            if self.fragments.len() > 512 {
                self.fragments.clear();
                warn!("There were more than 512 fragments in memory. Cleared!")
            }
            let mut full_data = "".to_string();
            for i in 0..length {
                let fragment_option = self.fragments.get(&(i, digest.clone()));
                if fragment_option.is_none() {
                    return None;
                }
                full_data.push_str(&fragment_option.unwrap().content["data"]);
            }
            let control_datagram = serde_json::from_str::<ControlDatagram>(&full_data).ok()?;
            return Some(control_datagram);
        }
        None
    }
}
