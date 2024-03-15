use crate::control_datagram::ControlDatagram;

struct OutboundHandler {
    receiver: tokio::sync::mpsc::Receiver<ControlDatagram>,
    sender: tokio::sync::mpsc::Sender<ControlDatagram>,
}
