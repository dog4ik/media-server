use std::fmt::Display;

use anyhow::Context;
use reqwest::StatusCode;

use crate::{ssdp::NotificationType, templates::UpnpAgent};

mod subscribers_store;

#[derive(Debug)]
pub struct SubscriptionMessage<'a> {
    publisher_path: String,
    user_agent: UpnpAgent<'a>,
    host: String,
    callback: String,
    nt: NotificationType,
    timeout: usize,
    statevar: String,
}

impl Display for SubscriptionMessage<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "SUBSCRIBE {publisher_path} HTTP/1.1\r\n\
HOST: {host}\r\n\
USER-AGENT: {user_agent}\r\n\
CALLBACK: {callback}\r\n\
NT: {nt}\r\n
TIMEOUT: {timeout}\r\n
STATEVAR: {statevar}\r\n",
            publisher_path = self.publisher_path,
            host = self.host,
            user_agent = self.user_agent,
            callback = self.callback,
            nt = self.nt,
            timeout = self.timeout,
            statevar = self.statevar,
        )?;
        write!(f, "\r\n")
    }
}

#[derive(Debug)]
pub struct SubscribeResponse {
    user_agent: UpnpAgent<'static>,
    timeout: usize,
    accepted_statevar: String,
    sid: uuid::Uuid,
}

impl Display for SubscribeResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let now = time::OffsetDateTime::now_utc();
        let format = time::format_description::parse_borrowed::<2>("[weekday repr:short], [day padding:zero] [month repr:short] [year] [hour]:[minute]:[second] GMT").expect("infallible");
        let formatted_date = now.format(&format).expect("infallible");
        write!(
            f,
            "HTTP/1.1 OK\r\n\
DATE: {date}\r\n\
SERVER: {user_agent}\r\n\
SID: {sid}\r\n\
CONTENT-LENGTH: 0\r\n\
TIMEOUT: {timeout}\r\n\
ACCEPTED-STATEVAR: {statevar}\r\n",
            date = formatted_date,
            user_agent = self.user_agent,
            timeout = self.timeout,
            statevar = self.accepted_statevar,
            sid = self.sid,
        )?;
        write!(f, "\r\n")
    }
}

pub struct SubscriptionError(StatusCode);

impl SubscriptionError {
    /// An SID header field and one of NT or CALLBACK header fields are present.
    const INCOMPATIBLE_HEADER_FIELD: Self = Self(StatusCode::BAD_REQUEST);
    /// CALLBACK header field is missing or does not contain a valid HTTP URL;
    /// or the NT header field does not equal upnp:event.
    const PRECONDITION_FAILED: Self = Self(StatusCode::PRECONDITION_FAILED);
}

#[derive(Debug)]
pub enum EventMessage<'a> {
    Subscribe(SubscriptionMessage<'a>),
    Renew,
    Unsubscribe,
}

impl<'a> EventMessage<'a> {
    pub fn parse(s: &'a str) -> anyhow::Result<Self> {
        let mut lines = s.lines();
        let request_line = lines.next().context("request line")?;
        let (method, _) = request_line.split_once(' ').context("split request line")?;
        let headers = lines.filter_map(|l| l.split_once(": "));
        match method {
            "SUBSCRIBE" => {}
            "UNSUBSCRIBE" => {}
            _ => {}
        }
        todo!();
    }
}
