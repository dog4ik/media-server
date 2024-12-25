use std::path::PathBuf;

use reqwest::Url;

use crate::{peers::BitField, Info};

#[derive(Debug, Clone)]
pub struct DownloadParams {
    pub bitfield: BitField,
    pub info: Info,
    pub trackers: Vec<Url>,
    pub files: Vec<crate::Priority>,
    pub save_location: PathBuf,
}

impl DownloadParams {
    pub fn empty(
        info: Info,
        tracker_list: Vec<Url>,
        files: Vec<crate::Priority>,
        save_location: PathBuf,
    ) -> Self {
        let bitfield = BitField::empty(info.pieces.len());
        Self {
            bitfield,
            info,
            trackers: tracker_list,
            files,
            save_location,
        }
    }

    pub fn new(
        bitfield: BitField,
        info: Info,
        tracker_list: Vec<Url>,
        files: Vec<crate::Priority>,
        save_location: PathBuf,
    ) -> Self {
        Self {
            bitfield,
            info,
            trackers: tracker_list,
            files,
            save_location,
        }
    }

    pub fn enabled_files_bitfield(&self) -> BitField {
        let total_files = self.info.files_amount();
        let mut bitfield = BitField::empty(total_files);
        for enabled_file in self
            .files
            .iter()
            .enumerate()
            .filter_map(|(i, f)| (!f.is_disabled()).then_some(i))
        {
            bitfield.add(enabled_file).unwrap();
        }
        bitfield
    }
}
