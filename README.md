# hamesh
P2P TCP/UDP tunnel over UDP hole punching


### How to use
```bash
# Usage:
hamesh p2p <ip_version: ipv4|ipv6> [--inbound <host_port>:<remote_host_port>/<protocol: tcp|udp>...] [--outbound <host_port>:<remote_host_port>/<protocol: tcp|udp>...]

# Server example:
hamesh p2p ipv6 --inbound 8080:8081/tcp
# Client Example:
hamesh p2p ipv6 --outbound 8081:8080/tcp
```

### How does it work?
- peers connect to a STUN like server and receive a subscription_id
- one of them provide the other one's subscription_id to the cli
- through the STUN like server they share settings and establish connection