use tokio::sync::mpsc;
use tracing::Instrument;

use crate::{
    ban_list::BanList,
    download::peer::Performance,
    metric,
    progress::{
        self,
        consumer::ProgressConsumer,
        events::{SessionEvent, TorrentTickEvents},
    },
    session::{
        session_message::{SessionHandle, SessionMessage, TorrentStateRequest},
        tick_context::TickContext,
    },
};

pub mod config;
pub mod session_message;
pub(crate) mod tick_context;
mod torrent_list;

#[derive(Debug)]
pub struct Session<P = ()> {
    torrent_list: torrent_list::TorrentList,
    ban_list: BanList,
    config: config::SessionConfiguration,
    tick_num: usize,
    metrics: Performance,
    running_performance: metric::RollingSpeedMeter,
    progress_consumer: P,
    peer_connections: u16,
}

impl<T: ProgressConsumer> Session<T> {
    pub fn spawn(config: config::SessionConfiguration, progress_consumer: T) -> SessionHandle {
        let (tx, rx) = mpsc::channel(10);
        let session = Self {
            torrent_list: torrent_list::TorrentList::new(),
            ban_list: BanList::new(),
            config,
            tick_num: 0,
            metrics: Performance::default(),
            running_performance: metric::RollingSpeedMeter::new(),
            progress_consumer,
            peer_connections: 0,
        };
        tokio::spawn(
            async move { session.work(rx).await }
                .instrument(tracing::info_span!("torrent_session")),
        );
        SessionHandle::new(tx)
    }

    async fn handle_command(&mut self, command: session_message::SessionMessage) {
        let mut tick_context = TickContext {
            allowed_connections: u8::MAX as usize,
            tick_interval: self.config.tick_interval,
            tick_start: std::time::Instant::now(),
            events: TorrentTickEvents::new(),
            tick_num: self.tick_num,
            ban_list: &self.ban_list,
        };
        let mut changed_torrents = Vec::new();
        match command {
            SessionMessage::AddTorrent(mut torrent) => {
                if self.torrent_list.find(torrent.info_hash).is_none() {
                    torrent.initial_tracker_announce();
                    let full_state = torrent.full_state(&mut tick_context);
                    tick_context
                        .events
                        .emit_session(SessionEvent::TorrentAdd(Box::new(full_state)));
                    self.torrent_list.add(*torrent);
                } else {
                    tracing::warn!("Attempt to add duplicate torrent");
                }
            }
            SessionMessage::SetStrategy { torrent, strategy } => {
                if let Some(torrent) = self.torrent_list.find_mut(torrent) {
                    torrent
                        .handle_command(
                            &mut tick_context,
                            crate::DownloadMessage::SetStrategy(strategy),
                        )
                        .await
                }
            }
            SessionMessage::SetFilesPriority {
                torrent,
                file_indexes,
                priority,
            } => {
                if let Some(torrent) = self.torrent_list.find_mut(torrent) {
                    for file_idx in file_indexes {
                        torrent
                            .handle_command(
                                &mut tick_context,
                                crate::DownloadMessage::SetFilePriority { file_idx, priority },
                            )
                            .await;
                    }
                    changed_torrents.push(torrent.construct_torrent_update(tick_context.events));
                }
            }
            SessionMessage::PostFullState {
                tx,
                request: TorrentStateRequest::Single(hash),
            } => {
                if let Some(torrent) = self.torrent_list.find_mut(hash) {
                    let _ = tx.send(progress::full::FullSessionState {
                        torrents: vec![torrent.full_state(&mut tick_context)],
                        session_stats: self.full_stats(),
                    });
                }
            }
            SessionMessage::PostFullState {
                tx,
                request: TorrentStateRequest::Ranged(range),
            } => {
                let _ = tx.send(progress::full::FullSessionState {
                    session_stats: self.full_stats(),
                    torrents: self.torrent_list.items[range]
                        .iter()
                        .map(|d| d.full_state(&mut tick_context))
                        .collect(),
                });
            }
            SessionMessage::PostFullState {
                tx,
                request: TorrentStateRequest::All,
            } => {
                let _ = tx.send(progress::full::FullSessionState {
                    session_stats: self.full_stats(),
                    torrents: self
                        .torrent_list
                        .items
                        .iter()
                        .map(|d| d.full_state(&mut tick_context))
                        .collect(),
                });
            }
            SessionMessage::PerformAction { torrents, action } => {
                for info_hash in torrents {
                    let mut tick_context = TickContext {
                        allowed_connections: u8::MAX as usize,
                        tick_interval: self.config.tick_interval,
                        tick_start: std::time::Instant::now(),
                        events: TorrentTickEvents::new(),
                        tick_num: self.tick_num,
                        ban_list: &self.ban_list,
                    };
                    if let Some(download) = self.torrent_list.find_mut(info_hash) {
                        let message = match action {
                            session_message::Action::Validate => crate::DownloadMessage::Validate,
                            session_message::Action::Abort => {
                                download
                                    .handle_command(
                                        &mut tick_context,
                                        crate::DownloadMessage::Abort,
                                    )
                                    .await;
                                tick_context
                                    .events
                                    .emit_session(SessionEvent::TorrentRemove { info_hash });
                                changed_torrents
                                    .push(download.construct_torrent_update(tick_context.events));
                                self.torrent_list.remove(info_hash);
                                continue;
                            }
                            session_message::Action::Resume => crate::DownloadMessage::Resume,
                            session_message::Action::Pause => crate::DownloadMessage::Pause,
                        };
                        download.handle_command(&mut tick_context, message).await;
                        changed_torrents
                            .push(download.construct_torrent_update(tick_context.events));
                    }
                }
            }
        };

        if !changed_torrents.is_empty() {
            self.progress_consumer.consume_progress(crate::Progress {
                session_update: None,
                changed_torrents,
                tick_num: self.tick_num,
            });
        }
    }

    async fn work(
        mut self,
        mut commands_rx: mpsc::Receiver<session_message::SessionMessage>,
    ) -> anyhow::Result<()> {
        tracing::info!("Spawned torrent session");
        for download in &mut self.torrent_list.items {
            download.initial_tracker_announce();
        }
        let mut tick_interval = tokio::time::interval(self.config.tick_interval);
        tick_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            let mut connected_peers = 0;
            let mut changed_torrents = Vec::new();
            loop {
                tokio::select! {
                    _ = tick_interval.tick() => {
                        break;
                    }
                    Some(command) = commands_rx.recv() => self.handle_command(command).await,
                    _ = self.config.cancellation_token.cancelled() => {
                        // self.handle_shutdown().await;
                        return Ok(());
                    }
                }
            }

            let mut tick_context = TickContext {
                allowed_connections: u8::MAX as usize,
                tick_start: std::time::Instant::now(),
                events: TorrentTickEvents::new(),
                tick_num: self.tick_num,
                tick_interval: self.config.tick_interval,
                ban_list: &self.ban_list,
            };
            for download in &mut self.torrent_list.items {
                let downloaded_before = download.total_download();
                let uploaded_before = download.total_uploaded();
                download.tick(&mut tick_context);
                let downloaded_after = download.total_download();
                let uploaded_after = download.total_uploaded();
                self.metrics.downloaded += downloaded_after - downloaded_before;
                self.metrics.uploaded += uploaded_after - uploaded_before;
                let (download_speed, upload_speed) = download.performance().speed();
                let state = download.state();
                connected_peers += download.connections_count() as u16;

                if !tick_context.events.is_empty() {
                    let mut events = TorrentTickEvents::new();
                    std::mem::swap(&mut events, &mut tick_context.events);
                    changed_torrents.push(progress::TorrentUpdate {
                        events,
                        download_speed,
                        state,
                        total_downloaded: downloaded_after,
                        total_uploaded: uploaded_after,
                        upload_speed,
                        info_hash: download.info_hash,
                    })
                }
            }
            self.running_performance.update(
                tick_context.tick_start,
                Performance {
                    downloaded: self.metrics.downloaded,
                    uploaded: self.metrics.uploaded,
                },
            );

            if !changed_torrents.is_empty() {
                let (download_speed, upload_speed) = self.running_performance.speed();
                let session_update = Some(progress::SessionUpdate {
                    connected_peers,
                    download_speed,
                    upload_speed,
                });
                self.progress_consumer.consume_progress(progress::Progress {
                    changed_torrents,
                    session_update,
                    tick_num: self.tick_num,
                });
            }
            self.peer_connections = connected_peers;
            self.tick_num = self.tick_num.wrapping_add(1);
        }
    }

    pub fn full_stats(&self) -> progress::full::FullSessionStats {
        let mut download_speed = 0.0;
        let mut upload_speed = 0.0;
        let mut connected_peers = 0;
        for download in &self.torrent_list.items {
            let speed = download.performance().speed();
            download_speed += speed.0;
            upload_speed += speed.1;
            connected_peers += download.connections_count() as u16;
        }
        progress::full::FullSessionStats {
            download_speed,
            upload_speed,
            connected_peers,
        }
    }
}
