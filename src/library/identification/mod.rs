use std::{
    ffi::OsStr,
    path::{Component, Path, PathBuf},
    time::Instant,
};

use crate::library::{EXTRAS_FOLDERS, SUPPORTED_FILES, Video, movie::MovieIdent, show::ShowIdent};

use super::{movie::MovieIdentifier, show::ShowIdentifier};

const NAME_NOISE: [&str; 68] = [
    "3d",
    "sbs",
    "tab",
    "hsbs",
    "htab",
    "mvc",
    "hdr",
    "hdr-dvt",
    "hdc",
    "uhd",
    "ultrahd",
    "4k",
    "ac3",
    "dts",
    "dubbed",
    "dc",
    "divx",
    "divx5",
    "dsr",
    "dsrip",
    "dvd",
    "dvdrip",
    "dvdscr",
    "dvdscreener",
    "dvdivx",
    "hdtv",
    "hdrip",
    "hdtvrip",
    "ntsc",
    "ogg",
    "ogm",
    "pdtv",
    "repack",
    "rerip",
    "r5",
    "svcd",
    "nfo",
    "nfofix",
    "brrip",
    "bdrip",
    "480p",
    "480i",
    "576p",
    "576i",
    "720p",
    "720i",
    "1080p",
    "1080i",
    "2160p",
    "hrhd",
    "hrhdtv",
    "hddvd",
    "bluray",
    "blu-ray",
    "x264",
    "x265",
    "h264",
    "h265",
    "xvid",
    "xvidvd",
    "xxx",
    "www",
    "kp",
    "web-dl",
    "webdl",
    "webrip",
    "aac",
    "dts",
];

pub(super) const SPECIAL_CHARS: [char; 3] = [',', '_', ' '];

#[derive(Debug, Clone)]
pub struct Parser<T> {
    inner: T,
}

pub trait Parseable {
    fn parse_parent(&mut self, folder_tokens: Vec<Token<'_>>);
    fn parse_name(&mut self, name_tokens: Vec<Token<'_>>);
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
                            let tokens = tokenize_path(&final_part);
                            parsable.parse_name(tokens);
                            return parsable;
                        }
                        None => {
                            let comp = comp.to_string_lossy();
                            let tokens = tokenize_path(&comp);
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

    pub fn feed_filename(mut self, file_name: &OsStr) -> T {
        let file_name = file_name.to_string_lossy();
        self.inner.parse_name(tokenize_path(&file_name));
        self.inner
    }

    pub fn feed_directory(&mut self, dir_name: &OsStr) {
        let dir_name = dir_name.to_string_lossy();
        self.inner.parse_parent(tokenize_path(&dir_name));
    }

    pub fn into_inner(self) -> T {
        self.inner
    }
}

pub const OPEN_BRACKETS: [char; 3] = ['(', '[', '{'];
pub const CLOSE_BRACKETS: [char; 3] = [')', ']', '}'];
pub const SEPARATORS: [char; 4] = ['-', '_', ' ', '.'];

#[derive(Debug, PartialEq, Eq)]
pub enum Token<'a> {
    /// Represents anything that is not noise, year group or separator. It "may" contain show title
    Unknown(&'a str),
    /// Noise represents elements from [NAME_NOISE](NAME_NOISE)
    Noise(&'a str),
    /// 4 numbers digit token that "may" represent the release year
    Year(u16),
    GroupStart,
    /// Separator that have separators as neighbors
    ///
    /// For example in `Show - S02E3.mkv` `-` is explicit separator
    ExplicitSeparator,
    GroupEnd,
}

fn visit_year(value: &str, current_year: i32) -> Option<u16> {
    if value.len() == 4 {
        let year = value.parse().ok()?;
        if year > current_year as u16 {
            return None;
        }
        return Some(year);
    }
    None
}

fn tokenize_path(file_name: &str) -> Vec<Token<'_>> {
    let mut group_tag = None;
    let mut tokens = Vec::new();
    let mut current_token_start = 0;
    let total_tokens = file_name.len();
    let current_year = time::OffsetDateTime::now_utc().year();

    for (char_idx, (i, char)) in file_name.char_indices().enumerate() {
        if current_token_start > i {
            continue;
        }
        if let Some(close_token) = OPEN_BRACKETS
            .iter()
            .enumerate()
            .find_map(|(i, c)| (*c == char).then_some(CLOSE_BRACKETS[i]))
        {
            if i - current_token_start != 0 {
                let token = &file_name[current_token_start..i];
                if let Some(year) = visit_year(token, current_year) {
                    tokens.push(Token::Year(year));
                } else {
                    tokens.push(Token::Unknown(token));
                }
            }
            group_tag = Some(close_token);
            tokens.push(Token::GroupStart);
            current_token_start = i + 1;
            continue;
        }
        if group_tag.is_some_and(|t| char == t) {
            if i - current_token_start != 0 {
                let token = &file_name[current_token_start..i];
                if let Some(year) = visit_year(token, current_year) {
                    tokens.push(Token::Year(year));
                } else {
                    tokens.push(Token::Unknown(token));
                }
            }
            group_tag = None;
            tokens.push(Token::GroupEnd);
            current_token_start = i + 1;
            continue;
        }
        if SEPARATORS.contains(&char) {
            if i - current_token_start != 0 {
                let token = &file_name[current_token_start..i];
                if let Some(year) = visit_year(token, current_year) {
                    tokens.push(Token::Year(year));
                } else {
                    tokens.push(Token::Unknown(token));
                }
            }

            if let Some((prev, next)) = char_idx
                .checked_sub(1)
                .and_then(|idx| file_name.chars().nth(idx))
                .zip(file_name.chars().nth(char_idx + 1))
            {
                if SEPARATORS.contains(&prev) && next == prev {
                    tokens.push(Token::ExplicitSeparator);
                    current_token_start = i + 2;
                    continue;
                }
            };

            current_token_start = i + 1;
            continue;
        }
        // We should check noise only if it separate token
        if i - current_token_start == 0 {
            for noise in NAME_NOISE {
                let remaining_tokens = total_tokens - current_token_start;
                if noise.len() <= remaining_tokens {
                    let file_noise =
                        &file_name.get(current_token_start..current_token_start + noise.len());
                    if let Some(file_noise) = file_noise.filter(|n| n.eq_ignore_ascii_case(noise)) {
                        current_token_start = i + noise.len();
                        tokens.push(Token::Noise(file_noise));
                        break;
                    }
                }
            }
        }
    }

    if current_token_start != total_tokens {
        let remainder = &file_name[current_token_start..file_name.len()];
        tokens.push(Token::Unknown(remainder));
    }

    tokens
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
        let mut dir_season = None;
        let mut dir_year = None;
        let mut dir_show_title = None;

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
                    tracing::warn!("Skipping extras directory: {}", path.display());
                    continue;
                }
                let mut new_dir_parser = parser.clone();
                new_dir_parser.feed_directory(dir_name);
                directories.push((path, new_dir_parser));
                continue;
            }
            let Some(extension) = path.extension().and_then(|x| x.to_str()) else {
                tracing::trace!("Ignoring file without extension: {}", path.display());
                continue;
            };
            if metadata.is_file() && SUPPORTED_FILES.contains(&extension) {
                let Some(file_name) = path.file_name() else {
                    continue;
                };
                let metadata_parser = parser.clone();
                let show_ident: Result<ShowIdentifier, ShowIdent> =
                    metadata_parser.feed_filename(file_name).try_into();
                match &show_ident {
                    // Successfully parsed show identifier
                    Ok(identifier) => {
                        // use successfully parsed children for directory identifier
                        dir_show_title = Some(identifier.title.clone());
                        dir_season = Some(identifier.season);
                        if let Some(year) = identifier.year {
                            dir_year = Some(year);
                        }
                    }
                    // Failed parsed show identifier
                    Err(ident) => {
                        if !ident.title.is_empty() {
                            dir_show_title = Some(ident.title.clone());
                        }
                        if let Some(season) = ident.season {
                            dir_season = Some(season);
                        }
                        if ident.episode.is_none() {
                            need_sort = true;
                        }
                        if let Some(year) = ident.year {
                            dir_year = Some(year);
                        }
                    }
                };
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
                Err(mut ident) => {
                    if let Some(dir_title) = &dir_show_title {
                        if ident.title.is_empty() {
                            ident.title = dir_title.clone();
                        }
                    }
                    // Here is the right time to use video container metadata.
                    // The problem is that running ffprobe is expensive and will affect the startup time
                    let identifier = ShowIdentifier {
                        episode: ident.episode.unwrap_or(i as u16 + 1),
                        season: ident.season.or(dir_season).unwrap_or(1),
                        title: ident.title,
                        year: ident.year.or(dir_year),
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
                dirs.push(path);
                continue;
            }
            let Some(extension) = path.extension().and_then(|x| x.to_str()) else {
                tracing::trace!("Ignoring file without extension: {}", path.display());
                continue;
            };
            if path.is_file() && SUPPORTED_FILES.contains(&extension) {
                let ident = Parser::parse_filename(&path, MovieIdent::default());
                let identifier = MovieIdentifier {
                    title: ident.title,
                    year: ident.year,
                };
                let video = Video::from_path_unchecked(path);
                files.push((video, identifier));
            }
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use crate::library::identification::tokenize_path;

    use super::Token;

    fn tokenize_test<'a>(tests: impl IntoIterator<Item = (&'a str, Vec<Token<'a>>)>) {
        for (test, expected) in tests {
            assert_eq!(expected, tokenize_path(test));
        }
    }

    #[test]
    pub fn tokenize_shows() {
        let tests = [
            (
                "Cyberpunk.Edgerunners.S01E02.DUBBED.1080p.WEBRip.x265-RARBG[eztv.re]",
                vec![
                    Token::Unknown("Cyberpunk"),
                    Token::Unknown("Edgerunners"),
                    Token::Unknown("S01E02"),
                    Token::Noise("DUBBED"),
                    Token::Noise("1080p"),
                    Token::Noise("WEBRip"),
                    Token::Noise("x265"),
                    Token::Unknown("RARBG"),
                    Token::GroupStart,
                    Token::Unknown("eztv"),
                    Token::Unknown("re"),
                    Token::GroupEnd,
                ],
            ),
            (
                "shogun.2024.s01e05.2160p.web.h265-successfulcrab",
                vec![
                    Token::Unknown("shogun"),
                    Token::Year(2024),
                    Token::Unknown("s01e05"),
                    Token::Noise("2160p"),
                    Token::Unknown("web"),
                    Token::Noise("h265"),
                    Token::Unknown("successfulcrab"),
                ],
            ),
            (
                "Foo (2019).S04E03",
                vec![
                    Token::Unknown("Foo"),
                    Token::GroupStart,
                    Token::Year(2019),
                    Token::GroupEnd,
                    Token::Unknown("S04E03"),
                ],
            ),
            (
                "Inception - 2010 - 1080p - BluRay - x264 - YIFY",
                vec![
                    Token::Unknown("Inception"),
                    Token::ExplicitSeparator,
                    Token::Year(2010),
                    Token::ExplicitSeparator,
                    Token::Noise("1080p"),
                    Token::ExplicitSeparator,
                    Token::Noise("BluRay"),
                    Token::ExplicitSeparator,
                    Token::Noise("x264"),
                    Token::ExplicitSeparator,
                    Token::Unknown("YIFY"),
                ],
            ),
        ];
        tokenize_test(tests);
    }

    #[test]
    fn tokenize_movies() {
        let tests = [(
            "Aladdin.WEB-DL.KP.1080p-SOFCJ",
            vec![
                Token::Unknown("Aladdin"),
                Token::Noise("WEB-DL"),
                Token::Noise("KP"),
                Token::Noise("1080p"),
                Token::Unknown("SOFCJ"),
            ],
        )];
        tokenize_test(tests);
    }
}
