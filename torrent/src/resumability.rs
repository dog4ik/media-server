use std::path::PathBuf;

use reqwest::Url;

use crate::{peers::BitField, Info};

#[derive(Debug)]
pub struct ResumeData {
    pub bitfield: BitField,
    pub info: Info,
    pub trackers: Vec<Url>,
    pub enabled_files: Vec<usize>,
    pub save_location: PathBuf,
}

impl ResumeData {
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
}
