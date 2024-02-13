#[derive(Debug,Clone)]
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
#[derive(Debug,Clone)]
pub struct PortSettings{
    pub protocol: Protocol,
    pub host_port: u16,
    pub remote_host_port: u16,
}

#[derive(Debug,Clone)]
pub struct ClientDataSettings{
    pub protocol: Protocol,
    pub host_port: u16,
    pub host_client_port: u16,
}
#[derive(Debug)]
pub struct ServerDataSettings{
    pub protocol: Protocol,
    pub remote_host_port: u16,
    pub remote_host_client_port: u16,
}