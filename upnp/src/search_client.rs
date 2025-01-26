use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};

use crate::service_client::{ScpdClient, ScpdService};
use anyhow::Context;
use tokio::{net::UdpSocket, task::JoinSet};

use crate::{
    device_description::DeviceDescription,
    ssdp::{Announce, AnnounceHandler, SearchMessage, UnicastAnnounce, SSDP_ADDR},
    templates::service_description::Scpd,
    FromXml,
};

#[derive(Debug)]
pub struct SearchOptions {
    timeout: Duration,
    take: Option<usize>,
}

impl SearchOptions {
    pub fn new() -> Self {
        Self {
            timeout: Duration::from_secs(3),
            take: Some(1),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn take(mut self, take: Option<usize>) -> Self {
        self.take = take;
        self
    }
}

#[derive(Debug)]
pub struct SearchClient {
    socket: UdpSocket,
    fetch_client: reqwest::Client,
}

impl SearchClient {
    pub async fn bind() -> anyhow::Result<Self> {
        let socket = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)).await?;
        let fetch_client = reqwest::Client::new();
        Ok(Self {
            socket,
            fetch_client,
        })
    }

    async fn recv_announce(&self, buf: &mut [u8]) -> anyhow::Result<Announce> {
        let read = self.socket.recv(buf).await?;
        let msg = std::str::from_utf8(&buf[..read]).context("convert response to str")?;
        let response = <UnicastAnnounce as AnnounceHandler>::parse_announce(msg)?;
        Ok(response)
    }

    async fn fetch_xml(
        client: &reqwest::Client,
        url: impl reqwest::IntoUrl,
    ) -> anyhow::Result<String> {
        let request = client.request(reqwest::Method::GET, url).build()?;
        let res = client.execute(request).await?;
        let text = res.text().await?;
        Ok(text)
    }

    async fn get_client<T: ScpdService>(
        str_urn: std::sync::Arc<String>,
        announce: Announce,
        client: reqwest::Client,
    ) -> anyhow::Result<ScpdClient<T>> {
        let device_description = Self::fetch_xml(&client, &announce.location).await?;
        let device_description =
            DeviceDescription::read_xml(&mut quick_xml::Reader::from_str(&device_description))?;
        let service = device_description
            .device
            .all_services()
            .find(|s| s.service_type == *str_urn)
            .context("Find requested service")?;
        let mut url = reqwest::Url::parse(&announce.location)?;
        url.set_path(&service.control_url);
        let control_url = url.to_string();
        url.set_path(&service.scpd_url);
        let service_scpd = Self::fetch_xml(&client, url).await?;

        let service_scpd = Scpd::read_xml(&mut quick_xml::Reader::from_str(&service_scpd))?;

        return Ok(ScpdClient::new(service_scpd, control_url));
    }

    pub async fn search_for<T: ScpdService>(
        &self,
        options: SearchOptions,
    ) -> anyhow::Result<Vec<ScpdClient<T>>> {
        let SearchOptions { timeout, take } = options;
        let urn = T::URN;
        let str_urn = std::sync::Arc::new(urn.to_string());
        let msg = SearchMessage {
            host: SSDP_ADDR,
            st: crate::ssdp::NotificationType::Urn(urn),
            mx: Some(options.timeout.as_secs() as usize),
            user_agent: None,
            tcp_port: None,
            cp_fn: None,
            cp_uuid: None,
        };
        self.socket
            .send_to(msg.to_string().as_bytes(), SSDP_ADDR)
            .await?;
        let mut out = Vec::new();
        let mut join_set = JoinSet::new();
        let mut buf = [0; 2048];

        let _ = tokio::time::timeout(timeout, async {
            loop {
                tokio::select! {
                    announce = self.recv_announce(&mut buf) => {
                        let Ok(announce) = announce else {
                            return;
                        };
                        let client = self.fetch_client.clone();
                        let str_urn = str_urn.clone();
                        join_set.spawn(Self::get_client(str_urn, announce, client));
                    }
                    Some(Ok(Ok(client))) = join_set.join_next() => {
                        out.push(client);
                        if take.is_some_and(|take| take == out.len()) {
                            return;
                        }
                    }
                }
            }
        })
        .await;
        Ok(out)
    }
}
