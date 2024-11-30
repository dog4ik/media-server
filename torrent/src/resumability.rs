use std::path::PathBuf;

use reqwest::Url;

use crate::{peers::BitField, Info};

#[derive(Debug, Clone)]
pub struct DownloadParams {
    pub bitfield: BitField,
    pub info: Info,
    pub trackers: Vec<Url>,
    // Change it to bitfield?
    pub enabled_files: Vec<usize>,
    pub save_location: PathBuf,
}

impl DownloadParams {
    pub fn empty(
        info: Info,
        tracker_list: Vec<Url>,
        enabled_files: Vec<usize>,
        save_location: PathBuf,
    ) -> Self {
        let bitfield = BitField::empty(info.pieces.len());
        Self {
            bitfield,
            info,
            trackers: tracker_list,
            enabled_files,
            save_location,
        }
    }

    pub fn new(
        bitfield: BitField,
        info: Info,
        tracker_list: Vec<Url>,
        enabled_files: Vec<usize>,
        save_location: PathBuf,
    ) -> Self {
        Self {
            bitfield,
            info,
            trackers: tracker_list,
            enabled_files,
            save_location,
        }
    }

    pub fn enabled_files_bitfield(&self) -> BitField {
        let total_files = self.info.files_amount();
        let mut bitfield = BitField::empty(total_files);
        for enabled_file in &self.enabled_files {
            bitfield.add(*enabled_file).unwrap();
        }
        bitfield
    }
}
