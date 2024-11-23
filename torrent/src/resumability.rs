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
