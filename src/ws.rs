use std::{sync::Arc, time::Duration};

use anyhow::Context;
use axum::{
    extract::{
        State, WebSocketUpgrade,
        ws::{self, WebSocket},
    },
    response::Response,
};

use crate::{app_state::AppState, progress::Notification, torrent::TorrentProgress};

const SEND_TIMEOUT: Duration = Duration::from_secs(1);

/// Websockets connection input message
#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase", tag = "type")]
pub enum WsRequest {
    TorrentSubscribe,
    TorrentUnsubscribe,
}

/// Websockets connection output message
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase", tag = "type")]
pub enum WsMessage {
    AllTorrents {
        torrents: Vec<crate::torrent::TorrentState>,
    },
    TorrentProgress {
        #[schema(value_type = TorrentProgress)]
        progress: Arc<TorrentProgress>,
    },
    Progress {
        progress: Notification,
    },
    Connected,
    TorrentUnsubscribe,
}

#[derive(Debug)]
struct Connection {
    is_torrent_subscribed: bool,
    socket: WebSocket,
}

impl Connection {
    pub fn new(socket: WebSocket) -> Self {
        Self {
            socket,
            is_torrent_subscribed: false,
        }
    }

    pub async fn send(&mut self, msg: WsMessage) -> anyhow::Result<()> {
        let str = serde_json::to_string(&msg).expect("serialization is infallible");
        tokio::time::timeout(
            SEND_TIMEOUT,
            self.socket.send(ws::Message::Text(str.into())),
        )
        .await
        .context("send timed out")??;
        Ok(())
    }

    pub async fn recv(&mut self) -> anyhow::Result<Option<WsRequest>> {
        match self.socket.recv().await {
            Some(Ok(ws::Message::Text(text))) => Ok(serde_json::from_str(&text)?),
            Some(Ok(ws::Message::Close(_))) => Err(anyhow::anyhow!("peer closed the connection"))?,
            Some(Ok(_)) => Ok(None),
            Some(Err(e)) => Err(e)?,
            None => Err(anyhow::anyhow!("stream closed")),
        }
    }
}

/// Open websockets connection
#[utoipa::path(
    method(get, post, put, delete, patch),
    path = "/api/ws",
    responses(
        (status = 101, description = "Protocol upgrade"),
    ),
    tag = "Tasks",
)]
pub async fn ws(ws: WebSocketUpgrade, State(app_state): State<AppState>) -> Response {
    ws.on_upgrade(|socket| ws_handler(socket, app_state))
}

async fn ws_handler(socket: WebSocket, app_state: AppState) {
    tracing::debug!("Opened ws connection");
    if let Err(e) = ws_handler_inner(socket, app_state).await {
        tracing::debug!("Websocket connection closed: {e}");
    } else {
        tracing::debug!("Websocket connection closed");
    }
}

async fn ws_handler_inner(socket: WebSocket, app_state: AppState) -> anyhow::Result<()> {
    let mut progress = app_state.tasks.progress_channel.0.subscribe();
    let mut torrent_progress = app_state.torrent_client.progress_broadcast.subscribe();
    let mut connection = Connection::new(socket);

    connection.send(WsMessage::Connected).await?;

    loop {
        tokio::select! {
            msg = connection.recv() => {
                let msg = msg?;
                if let Some(msg) = msg {
                    handle_request(msg, &mut connection, &app_state).await?;
                }
            },
            progress = progress.recv() => {
                let progress = progress?;
                connection.send(WsMessage::Progress{ progress }).await?;
            }
            progress = torrent_progress.recv() => {
                let progress = progress?;
                handle_torrent_progress(&mut connection, progress).await?;
            }
        }
    }
}

async fn handle_request(
    request: WsRequest,
    connection_state: &mut Connection,
    app_state: &AppState,
) -> anyhow::Result<()> {
    match request {
        WsRequest::TorrentSubscribe => {
            tracing::debug!("Received torrents subscription");
            let torrents = app_state.torrent_client.all_downloads().await;
            connection_state
                .send(WsMessage::AllTorrents { torrents })
                .await?;
            connection_state.is_torrent_subscribed = true;
        }
        WsRequest::TorrentUnsubscribe => {
            tracing::debug!("Received unsubscribe from torrent progress");
            connection_state.is_torrent_subscribed = false;
        }
    }
    Ok(())
}

async fn handle_torrent_progress(
    connection: &mut Connection,
    progress: Arc<TorrentProgress>,
) -> anyhow::Result<()> {
    if connection.is_torrent_subscribed {
        connection
            .send(WsMessage::TorrentProgress { progress })
            .await?;
    }
    Ok(())
}
