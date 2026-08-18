use std::{
    ffi::OsStr,
    path::{Component, Path, PathBuf},
    time::Instant,
};

use crate::{
    library::{
        EXTRAS_FOLDERS,
        media::{Video, container::VideoContainer},
    },
    parser::{
        movie::{MovieIdent, MovieIdentifier},
        show::{ShowIdent, ShowIdentifier},
        tokenizer::Tokenizer,
    },
};

pub mod attributes;
/// Identification for movie file names
pub mod movie;
/// Identification for show file names
pub mod show;
pub mod symbol;
pub mod tokenizer;

pub(super) const SPECIAL_CHARS: [char; 3] = [',', '_', ' '];

#[derive(Debug, Clone)]
pub struct Parser<T> {
    inner: T,
}

pub trait Parseable {
    fn parse_parent(&mut self, folder_tokens: Tokenizer<'_>);
    fn parse_name(&mut self, name_tokens: Tokenizer<'_>);
}

impl<T: Parseable> Parser<T> {
    /// Creates new parser returning result in Err if given path is a file
    pub fn new(parsable: T) -> Parser<T> {
        Self { inner: parsable }
    }

    pub fn apply_dir_path(&mut self, dir_path: &Path) {
        let mut path = dir_path.components();
        loop {
            match path.next() {
                Some(Component::Normal(comp)) => {
                    self.feed_directory(comp);
                }
                None => {
                    break;
                }
                Some(_) => continue,
            }
        }
    }

    pub fn apply_file_path(mut self, file_path: &Path) -> T {
        let mut path = file_path.components().peekable();
        loop {
            match path.next() {
                Some(Component::Normal(comp)) => {
                    let last_part = path
                        .peek()
                        .is_none()
                        .then(|| Path::new(comp).file_stem())
                        .flatten();

                    match last_part {
                        Some(last_part) => {
                            return self.feed_filename(last_part);
                        }
                        None => {
                            self.feed_directory(comp);
                        }
                    }
                }
                None => {
                    return self.into_inner();
                }
                Some(_) => continue,
            }
        }
    }

    pub fn parse_filename(file_path: &Path, mut parsable: T) -> T {
        let mut path = file_path.components().peekable();
        loop {
            match path.next() {
                Some(Component::Normal(comp)) => {
                    let final_part = path
                        .peek()
                        .is_none()
                        .then(|| Path::new(comp).file_stem())
                        .flatten();

                    match final_part {
                        Some(final_part) => {
                            let final_part = final_part.to_string_lossy();
                            let tokens = Tokenizer::new(&final_part);
                            parsable.parse_name(tokens);
                            return parsable;
                        }
                        None => {
                            let comp = comp.to_string_lossy();
                            let tokens = Tokenizer::new(&comp);
                            parsable.parse_parent(tokens);
                        }
                    }
                }
                None => {
                    return parsable;
                }
                Some(_) => continue,
            }
        }
    }

    pub fn parse_str(s: &str, mut parsable: T) -> T {
        let tokens = Tokenizer::new(&s);
        parsable.parse_name(tokens);
        return parsable;
    }

    pub fn feed_filename(mut self, file_name: &OsStr) -> T {
        let file_name = file_name.to_string_lossy();
        self.inner.parse_name(Tokenizer::new(&file_name));
        self.inner
    }

    pub fn feed_directory(&mut self, dir_name: &OsStr) {
        let dir_name = dir_name.to_string_lossy();
        self.inner.parse_parent(Tokenizer::new(&dir_name));
    }

    pub fn into_inner(self) -> T {
        self.inner
    }
}

pub fn walk_show_dirs(dirs: Vec<PathBuf>) -> Vec<(Video, ShowIdentifier)> {
    use std::fs;
    let mut files = Vec::new();
    let start = Instant::now();

    let mut directories: Vec<(PathBuf, Parser<ShowIdent>)> = dirs
        .into_iter()
        .map(|p| {
            let mut parser = Parser::new(ShowIdent::default());
            parser.apply_dir_path(&p);
            (p, parser)
        })
        .collect();

    while let Some((current_dir, parser)) = directories.pop() {
        let Ok(mut read_dir) = fs::read_dir(&current_dir) else {
            tracing::warn!("Failed to read show directory {}", current_dir.display());
            continue;
        };
        let mut supported_paths = Vec::new();

        // true value means that some episodes are missing number
        // we should try to sort them alphabetically to get their numbers
        let mut need_sort = false;

        while let Some(Ok(entry)) = read_dir.next() {
            let Ok(metadata) = entry.metadata() else {
                tracing::warn!("Failed to get fs metadata for {}", entry.path().display());
                continue;
            };
            let path = entry.path();
            if metadata.is_dir() {
                let Some(dir_name) = path.file_name() else {
                    continue;
                };
                if dir_name
                    .to_str()
                    .is_some_and(|f| EXTRAS_FOLDERS.iter().any(|e| f.eq_ignore_ascii_case(e)))
                {
                    tracing::trace!("Skipping extras directory: {}", path.display());
                    continue;
                }
                let mut new_dir_parser = parser.clone();
                new_dir_parser.feed_directory(dir_name);
                directories.push((path, new_dir_parser));
                continue;
            }
            if !path
                .extension()
                .is_some_and(|e| VideoContainer::try_from(e).is_ok())
            {
                tracing::trace!(
                    "Ignoring file without supported extension: {}",
                    path.display()
                );
                continue;
            };
            if metadata.is_file() {
                let Some(file_name) = path.file_name() else {
                    continue;
                };
                let metadata_parser = parser.clone();
                let show_ident: Result<ShowIdentifier, ShowIdent> =
                    metadata_parser.feed_filename(file_name).try_into();
                if let Err(ident) = &show_ident {
                    if ident.episode.is_none() {
                        need_sort = true;
                    }
                }
                supported_paths.push((path, show_ident));
            } else {
                tracing::trace!("Skipping unsupported file: {}", path.display());
            }
        }
        if need_sort {
            tracing::trace!("Sorting detected episodes");
            supported_paths.sort_by(|(a, _), (b, _)| a.cmp(b));
        }
        for (i, (path, ident_result)) in supported_paths.into_iter().enumerate() {
            let video = Video::from_path_unchecked(&path);
            match ident_result {
                Ok(identifier) => {
                    files.push((video, identifier));
                }
                Err(ident) => {
                    let identifier = ShowIdentifier {
                        episode: ident.episode.unwrap_or(i as u16 + 1),
                        season: ident.season.unwrap_or(1),
                        title: ident.title,
                        year: ident.year,
                        attributes: ident.attributes,
                    };
                    files.push((video, identifier));
                }
            }
        }
    }

    tracing::debug!("Walking show dirs took {:?}", start.elapsed());
    files
}

pub async fn walk_movie_dirs(mut dirs: Vec<PathBuf>) -> Vec<(Video, MovieIdentifier)> {
    use tokio::fs;
    let mut files = Vec::new();

    while let Some(current_dir) = dirs.pop() {
        let Ok(mut read_dir) = fs::read_dir(&current_dir).await else {
            tracing::warn!("Failed to read movie directory {}", current_dir.display());
            continue;
        };

        while let Ok(Some(entry)) = read_dir.next_entry().await {
            let path = entry.path();
            if path.is_dir() {
                let Some(dir_name) = path.file_name() else {
                    continue;
                };
                if dir_name
                    .to_str()
                    .is_some_and(|f| EXTRAS_FOLDERS.iter().any(|e| f.eq_ignore_ascii_case(e)))
                {
                    tracing::trace!("Skipping extras directory: {}", path.display());
                    continue;
                }
                dirs.push(path);
                continue;
            }
            if !path
                .extension()
                .is_some_and(|e| VideoContainer::try_from(e).is_ok())
            {
                tracing::trace!(
                    path = %path.display(),
                    "Ignoring file without proper video extension",
                );
                continue;
            };
            if path.is_file() {
                let ident = Parser::parse_filename(&path, MovieIdent::default());
                let identifier = MovieIdentifier {
                    title: ident.title,
                    year: ident.year,
                    attributes: ident.attributes,
                };
                let video = Video::from_path_unchecked(path);
                files.push((video, identifier));
            }
        }
    }
    files
}
