#![feature(async_fn_in_trait)]
pub mod library;
pub mod movie_file;
pub mod posters;
pub mod process_file;
pub mod scan;
pub mod serve_content;
pub mod show_file;
pub mod test;

pub use process_file::get_metadata;
pub use scan::Library;
pub use show_file::ShowFile;
