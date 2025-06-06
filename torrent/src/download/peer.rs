use std::{
    collections::VecDeque,
    io::Write,
    net::SocketAddr,
    time::{Duration, Instant},
};

use anyhow::Context;
use bytes::{BufMut, Bytes};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    BitField,
    peers::{Peer, PeerCommandMessage},
    protocol::{
        extension::Extension,
        peer::{ExtensionHandshake, HandShake, PeerMessage},
        pex::PexHistory,
        ut_metadata::UtMessage,
    },
    scheduler,
};

#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub struct Performance {
    pub downloaded: u64,
    pub uploaded: u64,
}

impl Performance {
    pub fn new(downloaded: u64, uploaded: u64) -> Self {
        Self {
            downloaded,
            uploaded,
        }
    }

    /// download in bytes per measurement period
    pub fn download_speed(&self) -> u64 {
        self.downloaded
    }

    /// upload in bytes per measurement period
    pub fn upload_speed(&self) -> u64 {
        self.uploaded
    }
}

#[derive(Debug, Clone)]
pub struct PerformanceHistory {
    /// Contains data that represents how difference between two measurements changed
    history: VecDeque<Performance>,
    // Snapshot of latest measuremnts. Used to calculate new measurements
    snapshot: Performance,
}

impl PerformanceHistory {
    const MAX_CAPACITY: usize = 20;

    pub fn new() -> Self {
        Self {
            history: VecDeque::with_capacity(Self::MAX_CAPACITY),
            snapshot: Performance::default(),
        }
    }

    pub fn update(&mut self, new: Performance) {
        if self.history.len() == Self::MAX_CAPACITY {
            self.history.pop_back();
        }
        let perf = Performance::new(
            new.downloaded - self.snapshot.downloaded,
            new.uploaded - self.snapshot.uploaded,
        );
        self.snapshot = new;
        self.history.push_front(perf);
    }

    pub fn avg_down_speed(&self) -> u64 {
        if self.history.is_empty() {
            return 0;
        }
        let mut avg = 0;
        for measure in &self.history {
            avg += measure.download_speed();
        }
        avg / self.history.len() as u64
    }

    pub fn avg_down_speed_sec(&self, tick_duration: &Duration) -> u64 {
        let tick_secs = tick_duration.as_secs_f32();
        let download_speed = self.avg_down_speed() as f32 / tick_secs;
        download_speed as u64
    }

    pub fn avg_up_speed(&self) -> u64 {
        if self.history.is_empty() {
            return 0;
        }
        let mut avg = 0;
        for measure in &self.history {
            avg += measure.upload_speed();
        }
        avg / self.history.len() as u64
    }

    pub fn avg_up_speed_sec(&self, tick_duration: &Duration) -> u64 {
        let tick_secs = tick_duration.as_secs_f32();
        let upload_speed = self.avg_up_speed() as f32 / tick_secs;
        upload_speed as u64
    }
}

impl Default for PerformanceHistory {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct InterestedPieces {
    bf: BitField,
    interested_amount: usize,
}

impl InterestedPieces {
    pub fn new(piece_table: &Vec<scheduler::SchedulerPiece>, peer_bf: &BitField) -> Self {
        let bf = BitField::empty(piece_table.len());

        let mut this = Self {
            bf,
            interested_amount: 0,
        };
        this.recalculate(piece_table, peer_bf);
        this
    }

    pub fn amount(&self) -> usize {
        self.interested_amount
    }

    pub fn recalculate(&mut self, piece_table: &[scheduler::SchedulerPiece], peer_bf: &BitField) {
        self.interested_amount = 0;
        for (i, piece) in piece_table.iter().enumerate() {
            if !piece.is_finished && !piece.priority.is_disabled() && peer_bf.has(i) {
                self.interested_amount += 1;
                self.bf.add(i).unwrap();
            } else {
                self.bf.remove(i).unwrap();
            }
        }
    }

    pub fn add_piece(&mut self, piece: usize) {
        if !self.bf.has(piece) {
            self.interested_amount += 1;
            self.bf.add(piece).unwrap();
        }
    }

    pub fn remove_piece(&mut self, piece: usize) {
        if self.bf.has(piece) {
            self.interested_amount -= 1;
            self.bf.remove(piece).unwrap();
        }
    }
}

#[derive(Debug)]
pub struct ActivePeer {
    pub id: Uuid,
    pub ip: SocketAddr,
    pub message_tx: flume::Sender<PeerCommandMessage>,
    pub message_rx: flume::Receiver<PeerMessage>,
    pub bitfield: BitField,
    /// Our status towards peer
    pub out_status: Status,
    /// Peer's status towards us
    pub in_status: Status,
    /// Amount of bytes downloaded from peer
    pub downloaded: u64,
    /// Amount of bytes uploaded to peer
    pub uploaded: u64,
    /// Peer's performance history (holds diff rates) useful to say how peer is performing
    pub performance_history: PerformanceHistory,
    /// Current pointer to the relevant pex history
    pub pex_idx: usize,
    pub last_pex_message_time: Instant,
    pub cancellation_token: CancellationToken,
    pub interested_pieces: InterestedPieces,
    pub handshake: HandShake,
    pub extension_handshake: Option<Box<ExtensionHandshake>>,
    /// Amount of blocks that are in flight
    /// Note that this number is approximate and not 100% accurate because of the race between chokes and requests
    pub pending_blocks: usize,
}

impl ActivePeer {
    pub fn new(
        message_tx: flume::Sender<PeerCommandMessage>,
        message_rx: flume::Receiver<PeerMessage>,
        peer: &Peer,
        interested_pieces: InterestedPieces,
        pex_idx: usize,
        cancellation_token: CancellationToken,
    ) -> Self {
        Self {
            id: peer.uuid,
            message_tx,
            message_rx,
            ip: peer.ip(),
            bitfield: peer.bitfield.clone(),
            in_status: Status::default(),
            out_status: Status::default(),
            downloaded: 0,
            uploaded: 0,
            performance_history: PerformanceHistory::new(),
            pex_idx,
            last_pex_message_time: Instant::now(),
            cancellation_token,
            interested_pieces,
            handshake: peer.handshake.clone(),
            extension_handshake: peer.extension_handshake.clone(),
            pending_blocks: 0,
        }
    }

    pub fn set_out_choke(&mut self, force: bool) -> anyhow::Result<()> {
        debug_assert_ne!(self.out_status.is_choked(), force);
        tracing::debug!(ip = %self.ip, "Setting out peer choke status to {force:?}");
        match force {
            true => self.message_tx.try_send(PeerCommandMessage::Choke)?,
            false => self.message_tx.try_send(PeerCommandMessage::Unchoke)?,
        }
        self.out_status.set_choke(force);
        Ok(())
    }

    pub fn set_out_interest(&mut self, force: bool) -> anyhow::Result<()> {
        debug_assert_ne!(self.out_status.is_interested(), force);
        tracing::debug!(ip = %self.ip, "Setting out peer interested status to {force:?}");
        match force {
            true => self.message_tx.try_send(PeerCommandMessage::Interested)?,
            false => self
                .message_tx
                .try_send(PeerCommandMessage::NotInterested)?,
        }
        self.out_status.set_interest(force);
        Ok(())
    }

    pub fn send_extension_message<'e, T: Extension<'e>>(&self, msg: T) -> anyhow::Result<()> {
        let handshake = self
            .extension_handshake
            .as_ref()
            .context("peer doesn't not support extensions")?;
        let extension_id = *handshake
            .dict
            .get(T::NAME)
            .context("extension is not supported by peer")?;
        let extension_message = PeerCommandMessage::Extension {
            extension_id,
            payload: msg.into(),
        };
        self.message_tx.try_send(extension_message)?;
        Ok(())
    }

    pub fn send_pex_message(&mut self, history: &PexHistory) {
        tracing::info!("Sending pex message to the peer");
        let message = history.pex_message(self.pex_idx);
        if self.send_extension_message(message).is_ok() {
            self.last_pex_message_time = Instant::now();
            self.pex_idx = history.tip();
        };
    }

    pub fn send_ut_metadata_block(
        &self,
        ut_message: UtMessage,
        piece: Bytes,
    ) -> anyhow::Result<()> {
        // TODO: avoid copying
        // parsing extension on tcp framing step will solve this issue
        // So it will be used like
        // self.message_tx.try_send(PeerMessage::UtExtension {
        //   extension_id,
        //   ut_message,
        //   piece,
        // })?;
        let extension_id = self
            .extension_handshake
            .as_ref()
            .and_then(|h| h.ut_metadata_id())
            .context("get ut_metadata extension id from handshake")?;
        let msg = ut_message.as_bytes();
        let payload = bytes::BytesMut::zeroed(msg.len() + piece.len());
        let mut writer = payload.writer();
        writer.write_all(&msg)?;
        writer.write_all(&piece)?;

        self.message_tx.try_send(PeerCommandMessage::Extension {
            extension_id,
            payload: writer.into_inner().freeze(),
        })?;
        Ok(())
    }

    /// Send cancel signal to the peer.
    /// It will force peer handle to join
    pub fn cancel_peer(&self) {
        self.cancellation_token.cancel();
    }

    pub fn add_interested(&mut self, piece: usize) {
        self.interested_pieces.add_piece(piece);
        if self.interested_pieces.amount() > 1 && !self.out_status.is_interested() {
            let _ = self.set_out_interest(true);
        }
    }

    pub fn remove_interested(&mut self, piece: usize) {
        self.interested_pieces.remove_piece(piece);
        if self.interested_pieces.amount() == 0 && self.out_status.is_interested() {
            let _ = self.set_out_interest(false);
        }
    }

    pub fn recalculate_interested_amount(&mut self, table: &[scheduler::SchedulerPiece]) {
        self.interested_pieces.recalculate(table, &self.bitfield);
        let amount = self.interested_pieces.amount();
        if amount == 0 && self.out_status.is_interested() {
            let _ = self.set_out_interest(false);
        }
        if amount > 0 && !self.out_status.is_interested() {
            let _ = self.set_out_interest(true);
        }
    }

    pub fn client_name(&self) -> &str {
        self.extension_handshake
            .as_ref()
            .and_then(|h| h.client_name())
            .unwrap_or_else(|| self.handshake.peer_id.client_name())
    }

    /// Is other peer better to choke?
    pub fn cmp_to_choke(&self, other: &Self) -> bool {
        other.performance_history.avg_down_speed() < self.performance_history.avg_down_speed()
    }

    /// Is other peer better to unchoke?
    pub fn cmp_to_unchoke(&self, other: &Self) -> bool {
        other.performance_history.avg_down_speed() > self.performance_history.avg_down_speed()
    }

    /// Canonical priority (BEP 40)
    #[allow(unused)]
    pub fn canonical_priority(&self, my_ip: SocketAddr) -> u32 {
        crate::protocol::peer::canonical_peer_priority(my_ip, self.ip)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Status {
    choked: bool,
    choked_time: Instant,
    interested: bool,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            choked: true,
            choked_time: Instant::now(),
            interested: false,
        }
    }
}

impl Status {
    pub fn set_choke(&mut self, force: bool) {
        self.choked_time = Instant::now();
        self.choked = force;
    }

    pub fn is_choked(&self) -> bool {
        self.choked
    }

    pub fn set_interest(&mut self, force: bool) {
        self.interested = force;
    }

    pub fn is_interested(&self) -> bool {
        self.interested
    }

    /// Duration since the last choke state change
    pub fn choke_duration(&self) -> Duration {
        self.choked_time.elapsed()
    }
}
