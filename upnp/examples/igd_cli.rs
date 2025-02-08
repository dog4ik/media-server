use std::{net::Ipv4Addr, time::Duration};

use clap::{Parser, ValueEnum};
use upnp::{
    internet_gateway::PortMappingProtocol,
    search_client::{SearchClient, SearchOptions},
};

#[derive(ValueEnum, Debug, Clone, Copy)]
enum Proto {
    TCP,
    UDP,
}

impl From<Proto> for PortMappingProtocol {
    fn from(val: Proto) -> Self {
        match val {
            Proto::TCP => PortMappingProtocol::TCP,
            Proto::UDP => PortMappingProtocol::UDP,
        }
    }
}

#[derive(Parser, Debug)]
enum Command {
    /// Open a port
    Open {
        #[clap(value_enum, long)]
        protocol: Proto,
        #[clap(long, short)]
        local_host: Option<Ipv4Addr>,
        #[clap(long, short)]
        external_host: Option<Ipv4Addr>,
        #[clap(long, short)]
        /// External port to open
        port: u16,
        /// Internal port
        #[clap(long, short)]
        internal_port: Option<u16>,
        #[clap(long, short)]
        /// Description of the service
        description: String,
        #[clap(long, default_value = "1800")]
        /// Lease duration in seconds
        lease: u32,
        /// Use AddAnyPortMapping instead of AddPortMapping
        #[clap(long, short)]
        any: bool,
    },
    /// Close a port
    Close {
        /// Port to close
        #[clap(long, short)]
        port: u16,
        #[clap(value_enum, long)]
        protocol: Proto,
    },
    /// Print external ip address
    GetExternalIp,
    /// List all port mappings
    ListPortMappings {
        /// Start range of port
        #[clap(long, short, default_value = "0")]
        start: u16,
        /// End range of port
        #[clap(long, short, default_value = "65535")]
        end: u16,
        /// Protocol to list
        #[clap(long)]
        protocol: Proto,
        /// - If the NewManage argument is set to false, then this action returns a list of port mappings
        /// that have InternalClient value matching to the IP address of the control point between
        /// NewStartPort and NewEndPort
        /// - If the NewManage argument is set to true, then the gateway MUST return all port mappings
        /// between NewStartPort and NewEndPort
        #[clap(long, short, default_value = "true")]
        manage: bool,
        /// How many ports to list
        #[clap(long, short, default_value = "10")]
        take: u32,
    },
}

#[derive(Parser, Debug)]
struct Args {
    #[clap(subcommand)]
    action: Command,
}

fn resolve_local_addr() -> Ipv4Addr {
    use anyhow::Context;
    use std::net::{SocketAddr, SocketAddrV4, UdpSocket};

    let google = Ipv4Addr::new(8, 8, 8, 8);
    // NOTE: this feels wrong. Find the better solution
    let socket =
        UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))).unwrap();
    socket
        .connect(std::net::SocketAddr::V4(SocketAddrV4::new(google, 0)))
        .unwrap();
    let addr = socket.local_addr().context("get local addr").unwrap();
    match addr {
        SocketAddr::V4(addr) => *addr.ip(),
        SocketAddr::V6(_) => panic!("ipv6 not handled"),
    }
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let client = SearchClient::bind().await.unwrap();

    let options = SearchOptions::new()
        .take(Some(1))
        .with_timeout(Duration::from_secs(4));

    let client = match client
        .search_for(options)
        .await
        .map(|c| c.into_iter().next())
    {
        Ok(Some(c)) => c,
        Ok(None) => panic!("Requested client is not found"),
        Err(e) => panic!("Search failed: {e}"),
    };

    match args.action {
        Command::Open {
            local_host,
            external_host,
            port,
            internal_port,
            description,
            protocol,
            lease,
            any,
        } => {
            let new_port = match any {
                true => client
                    .add_any_port_mapping(
                        external_host,
                        port,
                        protocol.into(),
                        internal_port.unwrap_or(port),
                        local_host.unwrap_or_else(resolve_local_addr),
                        description,
                        lease,
                    )
                    .await
                    .unwrap(),
                false => {
                    client
                        .add_port_mapping(
                            external_host,
                            port,
                            protocol.into(),
                            internal_port.unwrap_or(port),
                            local_host.unwrap_or_else(resolve_local_addr),
                            description,
                            lease,
                        )
                        .await
                        .unwrap();
                    port
                }
            };
            println!("Added port {new_port}");
        }
        Command::Close { port, protocol } => {
            client
                .delete_port_mapping(protocol.into(), port)
                .await
                .unwrap();
            println!("Deleted port {port}");
        }
        Command::GetExternalIp => {
            let ip = client.get_external_ip_addr().await.unwrap();
            println!("{ip}");
        }
        Command::ListPortMappings {
            start,
            end,
            protocol,
            take,
            manage,
        } => {
            let listing = client
                .list_all_port_mappings(start, end, protocol.into(), manage, take)
                .await
                .unwrap();
            for entry in listing {
                println!(
                    "{local_addr} ({description}) {local_port}:{external_port}",
                    local_addr = entry.new_internal_client,
                    description = entry.new_description,
                    local_port = entry.new_internal_port,
                    external_port = entry.new_external_port,
                )
            }
        }
    }
}
