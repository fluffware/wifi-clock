use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::Ipv4Address;
use embassy_net::Stack;
use embassy_time::{Duration, Instant};
use smoltcp::wire::{
    DhcpMessageType, DhcpPacket, DhcpRepr, EthernetAddress, IpAddress, IpEndpoint,
    DHCP_CLIENT_PORT, DHCP_SERVER_PORT,
};
use cortex_m::singleton;



#[derive(Clone, Copy)]
struct Lease {
    end_time: Option<Instant>, // Time when this lease ends, None if no lease
    client_id: EthernetAddress,
    client_ip: Ipv4Address,
}


pub struct DhcpServerConfig {
    pub subnet_mask: Ipv4Address,
    pub router: Option<Ipv4Address>,
    pub lease_duration: u32,
}

/*
    lease!(192, 168, 1, 151),
    lease!(192, 168, 1, 152),
    lease!(192, 168, 1, 153),
lease!(192, 168, 1, 154),
*/
pub struct DhcpServer<const N_LEASES: usize> {
    leases: [Lease; N_LEASES],
    config: DhcpServerConfig,
}



impl<const N_LEASES: usize> DhcpServer<N_LEASES> {
    pub fn new(start: Ipv4Address, config: DhcpServerConfig) -> DhcpServer<N_LEASES> {
        let mut leases = [Lease {
            end_time: None,
            client_id: EthernetAddress([0, 0, 0, 0, 0, 0]),
            client_ip: Ipv4Address::UNSPECIFIED,
        }; N_LEASES];
	let mut addr = start.to_bits();
	for i in 0..N_LEASES {
	    leases[i].client_ip = Ipv4Address::from_bits(addr);
	    addr += 1;
	}
	DhcpServer{leases,config}
    }


    fn find_lease(
	leases: &[Lease],
	client_id: EthernetAddress,
	requested_ip: Option<Ipv4Address>,
) -> Option<usize> {
	let now = Instant::now();
    let mut first_free = None;
    for (index, lease) in leases.iter().enumerate() {
        // If it's the same client, then reuse the lease
        if lease.client_id == client_id {
            return Some(index);
        }
        // Check if the lease is unused or expired
        let available = lease.end_time.map_or(true, |end_time| end_time < now);
        // Use requested address if available, otherwise use first available
        if available
            && (requested_ip.map_or(false, |ip| ip == lease.client_ip) || first_free.is_none())
        {
            first_free = Some(index);
        }
    }
    first_free
}
fn build_offer(msg: &mut DhcpRepr, addr: Ipv4Address, config: &DhcpServerConfig) {
    msg.message_type = DhcpMessageType::Offer;
    msg.your_ip = addr;
    msg.lease_duration = Some(config.lease_duration);
    msg.subnet_mask = Some(config.subnet_mask);
    msg.router = config.router;
}

fn build_ack(
    msg: &mut DhcpRepr,
    addr: Ipv4Address,
    config: &DhcpServerConfig,
    include_lease_duration: bool,
) {
    msg.message_type = DhcpMessageType::Ack;
    msg.your_ip = addr;
    msg.lease_duration = if include_lease_duration {
        Some(config.lease_duration)
    } else {
        None
    };
    msg.subnet_mask = Some(config.subnet_mask);
    msg.router = config.router;
}

fn build_nak(msg: &mut DhcpRepr) {
    msg.message_type = DhcpMessageType::Nak;
    msg.your_ip = Ipv4Address::UNSPECIFIED;
    msg.client_ip = Ipv4Address::UNSPECIFIED;
    msg.server_ip = Ipv4Address::UNSPECIFIED;
    msg.lease_duration = None;
    msg.subnet_mask = None;
    msg.router = None;
}

fn handle_msg(&mut self, msg: &mut DhcpRepr) {
    match msg.message_type {
        DhcpMessageType::Discover => {
            let client_id = msg
                .client_identifier
                .unwrap_or_else(|| msg.client_hardware_address);
            if let Some(index) = Self::find_lease(&self.leases, client_id, msg.requested_ip)
            {
                Self::build_offer(
                    msg,
                    self.leases[index].client_ip,
                    &self.config,
                );
            } else {
		Self::build_nak(msg);
            };
        }
        
        DhcpMessageType::Request => {
            let client_id = msg
                .client_identifier
                .unwrap_or_else(|| msg.client_hardware_address);

            if let Some(index) = Self::find_lease(&self.leases, client_id, msg.requested_ip)
            {
                self.leases[index].end_time = Some(
                    Instant::now()
                        + Duration::from_secs(self.config.lease_duration as u64),
                );
                self.leases[index].client_id = client_id;
                Self::build_ack(msg, self.leases[index].client_ip, &self.config, true);
            } else {
		Self::build_nak(msg);
            };
        }
        DhcpMessageType::Decline => {}
        DhcpMessageType::Release => {}
        DhcpMessageType::Inform => {}
        _ => {
            //debug!("Unhandled message type: {}", msg.message_type);
            return;
        }
    }
    msg.secs = 0;
    msg.requested_ip = None;
    msg.parameter_request_list = None;
    msg.dns_servers = None;
    msg.client_identifier = None;
    msg.max_size = None;
    msg.renew_duration = None;
    msg.rebind_duration = None;
}

pub async fn run(&mut self, stack: Stack<'_>)
{
    let rx = singleton!(: [u8; 4096] = [0; 4096]).unwrap();
    let rx_meta  = singleton!(: [PacketMetadata; 2] = [PacketMetadata::EMPTY; 2]).unwrap();
    let tx = singleton!(:[u8; 4096] = [0u8; 4096]).unwrap();
    let tx_meta = singleton!(:[PacketMetadata; 2] = [PacketMetadata::EMPTY; 2]).unwrap();
    let rx_buf = singleton!(: [u8; 1024] = [0u8; 1024]).unwrap();
    let mut tx_buf = singleton!(: [u8; 1024] = [0u8; 1024]).unwrap();
    let server_ip = if let Some(conf) = stack.config_v4() {
        conf.address.address()
    } else {
        Ipv4Address::UNSPECIFIED
    };
    loop {
        let mut socket = UdpSocket::new(stack, rx_meta, rx, tx_meta, tx);
        socket.bind(DHCP_SERVER_PORT).unwrap();
        loop {
            let (n, _ep) = socket.recv_from(rx_buf).await.unwrap();
            if let Ok(pkt) = DhcpPacket::new_checked(&rx_buf[..n]) {
                if let Ok(mut msg) = DhcpRepr::parse(&pkt) {
                    {
                        let mut reply_buf = DhcpPacket::new_unchecked(&mut tx_buf);
                        msg.server_ip = server_ip;
                        self.handle_msg(&mut msg);
                        if msg.emit(&mut reply_buf).is_err() {
                            continue;
                        }
                    }
                    let (client_addr, client_port) =
                        if msg.relay_agent_ip != smoltcp::wire::Ipv4Address::UNSPECIFIED {
                            (msg.relay_agent_ip, DHCP_SERVER_PORT)
                        } else if msg.client_ip != smoltcp::wire::Ipv4Address::UNSPECIFIED
                            && msg.message_type != DhcpMessageType::Nak
                        {
                            (msg.client_ip, DHCP_CLIENT_PORT)
                        } else {
                            (smoltcp::wire::Ipv4Address::BROADCAST, DHCP_CLIENT_PORT)
                        };
                    let ep = IpEndpoint {
                        addr: IpAddress::Ipv4(client_addr),
                        port: client_port,
                    };
                    let _ = socket.send_to(tx_buf, ep).await;
                }
            }
            //info!("rxd from {}: {}", ep, n);
        }
    }
}
}
