use std::path::PathBuf;

use reqwest::Url;

use crate::{bitfield::BitField, Info};

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
}
