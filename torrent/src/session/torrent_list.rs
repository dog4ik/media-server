use crate::download::Download;

#[derive(Debug)]
pub struct TorrentList {
    pub items: Vec<Download>,
}

impl Default for TorrentList {
    fn default() -> Self {
        Self::new()
    }
}

impl TorrentList {
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    pub fn find_mut(&mut self, info_hash: [u8; 20]) -> Option<&mut Download> {
        self.items.iter_mut().find(|t| t.info_hash == info_hash)
    }

    pub fn find(&self, info_hash: [u8; 20]) -> Option<&Download> {
        self.items.iter().find(|t| t.info_hash == info_hash)
    }

    pub fn add(&mut self, download: Download) {
        self.items.push(download);
    }

    pub fn remove(&mut self, info_hash: [u8; 20]) {
        self.items
            .iter()
            .position(|v| v.info_hash == info_hash)
            .map(|i| self.items.remove(i));
    }
}
