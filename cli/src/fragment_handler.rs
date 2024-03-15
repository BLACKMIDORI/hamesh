use crate::control_datagram::ControlDatagram;
use std::collections::HashMap;

pub struct FragmentHandler {
    fragments: HashMap<String, Vec<ControlDatagram>>,
}

impl FragmentHandler {
    pub fn new() -> FragmentHandler {
        FragmentHandler {
            fragments: HashMap::new(),
        }
    }
    pub fn get_complete_datagram(&self, fragment: ControlDatagram) -> Option<ControlDatagram> {
        None
    }
}
