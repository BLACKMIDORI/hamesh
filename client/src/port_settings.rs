#[derive(Debug)]
pub enum Protocol {
    Tcp,
    Udp
}
impl PartialEq for Protocol {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Tcp, Self::Tcp) | (Self::Udp, Self::Udp) => true,
            _ => false,
        }
    }
}
#[derive(Debug)]
pub struct PortSettings{
    pub protocol: Protocol,
    pub host_port: u16,
    pub remote_host_port: u16,
}