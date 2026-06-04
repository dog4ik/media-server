use std::path::Path;

use serde::Serialize;

use super::{
    ContentIdentifier, Media,
    identification::{Parseable, Parser, SPECIAL_CHARS, Token},
};

mod helpers {
    pub fn take_till_non_number(input: &mut &str) -> Option<u16> {
        let end = input
            .chars()
            .position(|c| !c.is_ascii_digit())
            .unwrap_or(input.len());
        let num = input[..end].parse().ok()?;
        *input = &input[end..];
        Some(num)
    }
}

fn parse_se_format(mut value: &str) -> Option<(u16, u16)> {
    parse_se_season(&mut value).zip(parse_se_episode(&mut value))
}

fn parse_se_season(value: &mut &str) -> Option<u16> {
    if value.starts_with(['s', 'S']) {
        *value = &value[1..];
        return helpers::take_till_non_number(value);
    }
    None
}

fn parse_se_episode(value: &mut &str) -> Option<u16> {
    if value.starts_with(['e', 'E']) {
        *value = &value[1..];
        return helpers::take_till_non_number(value);
    }
    None
}

fn parse_0x0_episode(value: &str) -> Option<(u16, u16)> {
    if let Some((mut season, mut episode)) = value.split_once('x') {
        let e = episode;
        if value.starts_with(['s', 'S']) {
            return Some((
                parse_se_season(&mut season)?,
                parse_se_episode(&mut episode).or_else(|| e.parse().ok())?,
            ));
        } else {
            return Some((season.parse().ok()?, episode.parse().ok()?));
        }
    }
    if let Some((season, episode)) = value.split_once('0') {
        return Some((season.parse().ok()?, episode.parse().ok()?));
    }
    None
}

/// Partial show identifier representation.
///
/// This is used during parsing where not all parts are yet known.
#[derive(Debug, Clone, Serialize, Default)]
pub struct ShowIdent {
    pub episode: Option<u16>,
    pub season: Option<u16>,
    pub title: String,
    pub year: Option<u16>,
}

impl Parseable for ShowIdent {
    fn parse_parent(&mut self, directory_tokens: Vec<Token<'_>>) {
        self.apply_parent_tokens(&directory_tokens);
    }

    fn parse_name(&mut self, name_tokens: Vec<Token<'_>>) {
        self.apply_name_tokens(&name_tokens);
    }
}

const SEASON_IDENTS: [&str; 2] = ["Season", "season"];

impl ShowIdent {
    pub fn apply_parent_tokens(&mut self, tokens: &[Token<'_>]) {
        if let (Some(Token::Unknown(season_ident)), Some(Token::Unknown(season_num))) =
            (tokens.first(), tokens.get(1))
        {
            if SEASON_IDENTS.contains(season_ident) {
                if let Ok(season_num) = season_num.parse() {
                    self.season = Some(season_num);
                    return;
                }
            }
        }
        self.apply_name(tokens);
    }

    /// Read the tokens and apply name, year, episode number etc. to the identifier
    ///
    /// Returns all [Token::Unknown] tokens that "may" be title
    pub fn apply_name<'a>(&mut self, tokens: &[Token<'a>]) -> Vec<&'a str> {
        let mut title = String::new();
        let mut season = None;
        let mut episode = None;
        let mut year = None;
        // true when we get past all name tokens(Usually name tokens come first)
        let mut past_name = false;
        // true if we are currently in group
        let mut in_group = false;
        // gather all unidentified tokens that *might* represent the name for the cases where title detection fails
        let mut fallback_name_tokens = Vec::new();

        for (i, token) in tokens.iter().enumerate() {
            match token {
                Token::Unknown(t) => {
                    if in_group {
                        continue;
                    }
                    // try to parse common episode formats
                    if let Some((s, e)) = parse_se_format(t).or_else(|| parse_0x0_episode(t)) {
                        season = Some(s);
                        episode = Some(e);
                        past_name = true;
                        continue;
                    }
                    // if we couldn't parse common season/episode try other formats
                    if season.is_none() {
                        {
                            let t = &mut &**t;
                            if let Some(s) = parse_se_season(t) {
                                season = Some(s);
                                past_name = true;
                                continue;
                            }
                        }
                        if *t == "Season" {
                            if let Some(s) = tokens.get(i + 1).and_then(|t| match t {
                                Token::Unknown(t) => t.parse().ok(),
                                _ => None,
                            }) {
                                past_name = true;
                                season = Some(s);
                                continue;
                            }
                        }
                    }
                    if episode.is_none() {
                        {
                            let t = &mut &**t;
                            if let Some(s) = parse_se_episode(t) {
                                episode = Some(s);
                                past_name = true;
                                continue;
                            }
                        }
                        if *t == "Episode" {
                            if let Some(e) = tokens.get(i + 1).and_then(|t| match t {
                                Token::Unknown(t) => t.parse().ok(),
                                _ => None,
                            }) {
                                episode = Some(e);
                                past_name = true;
                                continue;
                            }
                        }
                    }

                    // collect all unknown tokens in case we fail to detect title
                    fallback_name_tokens.push(*t);
                    // if we could not parse any season/episode yet, collect title tokens
                    let is_digits = || t.chars().all(|c| c.is_ascii_digit());
                    if !past_name && !in_group && !is_digits() {
                        if title.is_empty() {
                            title += t;
                        } else {
                            title += " ";
                            title += t;
                        }
                    }
                }
                Token::Noise(_) => {
                    // noise tokens are usually appear after the name
                    past_name = true;
                }
                Token::Year(y) => {
                    year = Some(*y);
                    past_name = true;
                }
                Token::GroupStart => {
                    in_group = true;
                    // A group whose sole content is an explicit `SxxExx` marker (e.g. `(S02E10)`)
                    if let (Some(Token::Unknown(t)), Some(Token::GroupEnd)) =
                        (tokens.get(i + 1), tokens.get(i + 2))
                    {
                        if let Some((s, e)) = parse_se_format(t) {
                            season = Some(s);
                            episode = Some(e);
                            past_name = true;
                        }
                    }
                }
                Token::GroupEnd => {
                    in_group = false;
                }
                Token::ExplicitSeparator => {
                    // this one is controversial. It is possible to have explicit separator between
                    // episode title and show name. Mostly it is not the case
                    past_name = true;
                }
            }
        }
        self.episode = episode.or(self.episode);
        self.season = season.or(self.season);
        self.year = year.or(self.year);
        if !title.is_empty() {
            self.title = title;
        }
        fallback_name_tokens
    }

    pub fn apply_name_tokens(&mut self, tokens: &[Token<'_>]) {
        let fallback_tokens = self.apply_name(tokens);

        let missing_title = self.title.is_empty();
        if self.episode.is_none() || missing_title {
            // we are in trouble because parser could not detect required information.
            tracing::warn!(?tokens, "Using episode detection fallback");
            let mut nums = Vec::new();
            // iterate over all the tokens that "may" be the title
            for token in fallback_tokens {
                if let Ok(num) = token.parse() {
                    if self.episode.is_none() {
                        nums.push(num);
                    }
                    continue;
                }
                if missing_title {
                    if self.title.is_empty() {
                        self.title += token;
                    } else {
                        self.title += " ";
                        self.title += token
                    }
                }
            }
            if self.episode.is_none() {
                let mut nums = nums.into_iter();
                // try to interpret numbers as episodes
                let season_episode = (nums.next(), nums.next());
                match season_episode {
                    (Some(ep), None) => self.episode = Some(ep),
                    (Some(se), Some(ep)) => {
                        if self.season.is_none() && self.episode.is_none() {
                            self.season = Some(se);
                            self.episode = Some(ep);
                        }
                        if self.episode.is_none() {
                            self.episode = Some(se);
                        }
                    }
                    _ => {}
                }
            }
        }

        self.title = self.title.trim_matches(SPECIAL_CHARS).to_string();

        if missing_title {
            tracing::warn!("Using title fallback: {}", self.title);
        }
    }

    /// Combine 2 idents. If 2 fields are same use self.
    pub fn merge(&mut self, other: ShowIdent) {
        self.episode = self.episode.or(other.episode);
        self.season = self.season.or(other.season);
        if self.title.is_empty() && !other.title.is_empty() {
            self.title = other.title;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::library::{identification::Parser, show::ShowIdent};

    macro_rules! episode_tests {
        ($($name:ident: ($input:expr, $title:expr, $season:expr, $episode:expr);)*) => {
            $(
                #[test]
                fn $name() {
                    let id = Parser::parse_filename(Path::new($input), ShowIdent::default());
                    assert_eq!($title, id.title, "title mismatch for {:?}: {:?}", $input, id);
                    assert_eq!($season, id.season, "season mismatch for {:?}: {:?}", $input, id);
                    assert_eq!($episode, id.episode, "episode mismatch for {:?}: {:?}", $input, id);
                }
            )*
        };
    }

    macro_rules! title_tests {
        ($($name:ident: ($input:expr, $title:expr);)*) => {
            $(
                #[test]
                fn $name() {
                    let id = Parser::parse_filename(Path::new($input), ShowIdent::default());
                    assert_eq!($title, id.title, "title mismatch for {:?}: {:?}", $input, id);
                }
            )*
        };
    }

    macro_rules! season_tests {
        ($($name:ident: ($input:expr, $season:expr);)*) => {
            $(
                #[test]
                fn $name() {
                    let id = Parser::parse_filename(Path::new($input), ShowIdent::default());
                    assert_eq!($season, id.season, "season mismatch for {:?}: {:?}", $input, id);
                }
            )*
        };
    }

    macro_rules! single_episode_tests {
        ($($name:ident: ($input:expr, $episode:expr);)*) => {
            $(
                #[test]
                fn $name() {
                    let id = Parser::parse_filename(Path::new($input), ShowIdent::default());
                    assert_eq!($episode, id.episode, "episode mismatch for {:?}: {:?}", $input, id);
                }
            )*
        };
    }

    // Jellyfin tests
    episode_tests! {
        simple_s01e02: ("/server/anything_s01e02.mp4", "anything", Some(1), Some(2));
        simple_s1e2: ("/server/anything_s1e2.mp4", "anything", Some(1), Some(2));
        simple_s01_dot_e02: ("/server/anything_s01.e02.mp4", "anything", Some(1), Some(2));
        // This one is supported but blocks `Season 2/One piece 1001`.
        simple_102: ("/server/anything_102.mp4", "anything", Some(1), Some(2));
        simple_1x02: ("/server/anything_1x02.mp4", "anything", Some(1), Some(2));
        simple_walking_dead_4x01: ("/server/The Walking Dead 4x01.mp4", "The Walking Dead", Some(4), Some(1));
        simple_simpsons_s02e01: ("/server/the_simpsons-s02e01_18536.mp4", "the simpsons", Some(2), Some(1));
        simple_temp_s01e02: ("/server/Temp/S01E02 foo.mp4", "Temp", Some(1), Some(2));
        simple_series_4x12: ("Series/4x12 - The Woman.mp4", "Series", Some(4), Some(12));
        // Current implementation thinks '.' is separator so resulting filename does not
        // contain it
        // simple_la_x: ("Series/LA X, Pt. 1_s06e32.mp4", "LA X, Pt. 1", Some(6), Some(32));
        simple_baz_bar_foo_05: ("[Baz-BarF,oo - [1080p][Multiple Subtitle]/[Baz-Bar] Foo - 05 [1080p][Multiple Subtitle].mkv", "Foo", None, Some(5));
        simple_series_name_s01e04: ("/Foo/The.Series.Name.S01E04.WEBRip.x264-Baz[Bar/,the.series.name.s01e04.webrip.x264-Baz[Bar].mkv", "the series name", Some(1), Some(4));
        simple_love_death_robots_s01e01: ("Love.Death.and.Robots.S01.1080p.NF.WEB-DL.DDP5.1.x264-NTG/Love.Death.and.Robots.S01E01.Sonnies.Edge.1080p.NF.WEB-DL.DDP5.1.x264-NTG.mkv", "Love Death and Robots", Some(1), Some(1));
        // We cannot know if text after explicit separator is the continuation of show name
        // simple_tensura: ("[YuiSubs ,Tensura Nikki - Tensei Shitara Slime Datta Ken/[YuiSubs] Tensura Nikki - Tensei Shitara Slime Datta Ken - 12 (NVENC H.265 1080p).mkv", "Tensura Nikki Tensei Shitara Slime Datta Ken", None, Some(12));
        simple_baz_bar_foo_05_alt: ("[Baz-BarF,oo - 01 - 12[1080p][Multiple Subtitle]/[Baz-Bar] Foo - 05 [1080p][Multiple Subtitle].mkv", "Foo", None, Some(5));
        simple_series_4_12: ("Series/4-12 - The Woman.mp4", "Series", Some(4), Some(12));
    }

    title_tests! {
        resolver_show_s01: ("The.Show.S01", "The Show");
        resolver_show_s01_complete: ("The.Show.S01.COMPLETE", "The Show");
        // resolver_show_acronym: ("S.H.O.W.S01", "S.H.O.W");
        // resolver_show_pi: ("The.Show.P.I.S01", "The Show P.I");
        resolver_show_season_1: ("The_Show_Season_1", "The Show");
        resolver_show_season_10: ("/something/The_Show/Season 10", "The Show");
        resolver_show_plain: ("The Show", "The Show");
        resolver_show_path: ("/some/path/The Show", "The Show");
        resolver_show_s02e10: ("/some/path/The Show s02e10 720p hdtv", "The Show");
        resolver_show_s02e10_episode_title: ("/some/path/The Show s02e10 the episode 720p hdtv", "The Show");
    }

    season_tests! {
        season_path_season_1: ("/Drive/Season 1", Some(1));
        season_path_s1: ("/Drive/s1", Some(1));
        season_path_s1_upper: ("/Drive/S1", Some(1));
        season_path_season_2: ("/Drive/Season 2", Some(2));
        season_path_season_02: ("/Drive/Season 02", Some(2));
        season_path_seinfeld_s02: ("/Drive/Seinfeld/S02", Some(2));
        // season_path_seinfeld_bare: ("/Drive/Seinfeld/2", Some(2));
        season_path_seinfeld_dash_s02: ("/Drive/Seinfeld - S02", Some(2));
        // season_path_season_2009: ("/Drive/Season 2009", Some(2009));
        // season_path_season1_joined: ("/Drive/Season1", Some(1));
        season_path_wonder_years_s04: ("The Wonder Years/The.Wonder.Years.S04.PDTV.x264-JCH", Some(4));
        season_path_season_7_year: ("/Drive/Season 7 (2016)", Some(7));
        // season_path_staffel_7: ("/Drive/Staffel 7 (2016)", Some(7));
        season_path_season_paren_8: ("/Drive/Season (8)", None);
        // season_path_staffel_3: ("/Drive/3.Staffel", Some(3));
        // season_path_s06e05: ("/Drive/s06e05", None);
        season_path_condor_heroes_none: ("/Drive/The.Legend.of.Condor.Heroes.2017.V2.web-dl.1080p.h264.aac-hdctv", None);
        season_path_extras: ("/Drive/extras", None);
        season_path_specials: ("/Drive/specials", None);
    }

    season_tests! {
        season_num_daily_show_25x22: ("The Daily Show/The Daily Show 25x22 - [WEBDL-720p][AAC 2.0][x264] Noah Baumbach-TBS.mkv", Some(25));
        season_num_s02e03: ("/Show/Season 02/S02E03 blah.avi", Some(2));
        season_num_name_s01x02: ("Season 1/seriesname S01x02 blah.avi", Some(1));
        season_num_s01x02: ("Season 1/S01x02 blah.avi", Some(1));
        season_num_name_s01xe02: ("Season 1/seriesname S01xE02 blah.avi", Some(1));
        season_num_01x02: ("Season 1/01x02 blah.avi", Some(1));
        season_num_s01e02: ("Season 1/S01E02 blah.avi", Some(1));
        season_num_s01xe02: ("Season 1/S01xE02 blah.avi", Some(1));
        season_num_name_01x02: ("Season 1/seriesname 01x02 blah.avi", Some(1));
        season_num_name_s01e02: ("Season 1/seriesname S01E02 blah.avi", Some(1));
        season_num_elementary_multi_02x: ("Season 2/Elementary - 02x03 - 02x04 - 02x15 - Ep Name.mp4", Some(2));
        season_num_multi_02x: ("Season 2/02x03 - 02x04 - 02x15 - Ep Name.mp4", Some(2));
        season_num_02x03_04_15: ("Season 2/02x03-04-15 - Ep Name.mp4", Some(2));
        season_num_elementary_02x03_04_15: ("Season 2/Elementary - 02x03-04-15 - Ep Name.mp4", Some(2));
        season_num_02x03_e15: ("Season 02/02x03-E15 - Ep Name.mp4", Some(2));
        season_num_elementary_02x03_e15: ("Season 02/Elementary - 02x03-E15 - Ep Name.mp4", Some(2));
        season_num_02x03_x04_x15: ("Season 02/02x03 - x04 - x15 - Ep Name.mp4", Some(2));
        season_num_elementary_02x03_x04_x15: ("Season 02/Elementary - 02x03 - x04 - x15 - Ep Name.mp4", Some(2));
        season_num_02x03x04x15: ("Season 02/02x03x04x15 - Ep Name.mp4", Some(2));
        season_num_elementary_02x03x04x15: ("Season 02/Elementary - 02x03x04x15 - Ep Name.mp4", Some(2));
        season_num_elementary_s01e23_multi: ("Season 1/Elementary - S01E23-E24-E26 - The Woman.mp4", Some(1));
        season_num_s01e23_multi: ("Season 1/S01E23-E24-E26 - The Woman.mp4", Some(1));
        season_num_simpsons_s25e09: ("Season 25/The Simpsons.S25E09.Steal this episode.mp4", Some(25));
        season_num_simpsons_dir_s25e09: ("The Simpsons/The Simpsons.S25E09.Steal this episode.mp4", Some(25));
        season_num_s2016e1: ("2016/Season s2016e1.mp4", Some(2016));
        season_num_2016x1: ("2016/Season 2016x1.mp4", Some(2016));
        season_num_2009x02: ("Season 2009/2009x02 blah.avi", Some(2009));
        season_num_s2009x02: ("Season 2009/S2009x02 blah.avi", Some(2009));
        season_num_s2009e02: ("Season 2009/S2009E02 blah.avi", Some(2009));
        season_num_s2009xe02: ("Season 2009/S2009xE02 blah.avi", Some(2009));
        season_num_name_2009x02: ("Season 2009/seriesname 2009x02 blah.avi", Some(2009));
        season_num_name_s2009x02: ("Season 2009/seriesname S2009x02 blah.avi", Some(2009));
        season_num_name_s2009e02: ("Season 2009/seriesname S2009E02 blah.avi", Some(2009));
        season_num_elementary_2009x_multi: ("Season 2009/Elementary - 2009x03 - 2009x04 - 2009x15 - Ep Name.mp4", Some(2009));
        season_num_2009x_multi: ("Season 2009/2009x03 - 2009x04 - 2009x15 - Ep Name.mp4", Some(2009));
        season_num_2009x03_04_15: ("Season 2009/2009x03-04-15 - Ep Name.mp4", Some(2009));
        season_num_elementary_2009x03_x04_x15: ("Season 2009/Elementary - 2009x03 - x04 - x15 - Ep Name.mp4", Some(2009));
        season_num_2009x03x04x15: ("Season 2009/2009x03x04x15 - Ep Name.mp4", Some(2009));
        season_num_elementary_2009x03x04x15: ("Season 2009/Elementary - 2009x03x04x15 - Ep Name.mp4", Some(2009));
        season_num_elementary_s2009e23_multi: ("Season 2009/Elementary - S2009E23-E24-E26 - The Woman.mp4", Some(2009));
        season_num_s2009e23_multi: ("Season 2009/S2009E23-E24-E26 - The Woman.mp4", Some(2009));
        season_num_series_1_12: ("Series/1-12 - The Woman.mp4", Some(1));
        season_num_running_man_s2017e368: ("Running Man/Running Man S2017E368.mkv", Some(2017));
        // season_num_case_closed_317: ("Case Closed (1996-2007)/Case Closed - 317.mkv", Some(3));
        // season_num_seinfeld_0807: ("Seinfeld/Seinfeld 0807 The Checks.avi", Some(8));
    }

    episode_tests! {
        path_foo_s01e01: ("/media/Foo/Foo-S01E01", "Foo", Some(1), Some(1));
        path_foo_s04e011: ("/media/Foo - S04E011", "Foo", Some(4), Some(11));
        path_foo_s01x01: ("/media/Foo/Foo s01x01", "Foo", Some(1), Some(1));
        path_foo_2019_s04e03: ("/media/Foo (2019)/Season 4/Foo (2019).S04E03.mp4", "Foo", Some(4), Some(3));
        path_elementary_02x03: ("/Season 2/Elementary - 02x03-04-15 - Ep Name.mp4", "Elementary", Some(2), Some(3));
        path_seriesname_s01e02: ("/Season 1/seriesname S01E02 blah.avi", "seriesname", Some(1), Some(2));
        path_running_man_s2017e368: ("/Running Man/Running Man S2017E368.mkv", "Running Man", Some(2017), Some(368));
        path_seriesname_01x02: ("/Season 1/seriesname 01x02 blah.avi", "seriesname", Some(1), Some(2));
        path_simpsons_s25e09: ("/Season 25/The Simpsons.S25E09.Steal this episode.mp4", "The Simpsons", Some(25), Some(9));
        path_seriesname_s01x02: ("/Season 1/seriesname S01x02 blah.avi", "seriesname", Some(1), Some(2));
        path_seriesname_s01xe02: ("/Season 1/seriesname S01xE02 blah.avi", "seriesname", Some(1), Some(2));
        path_wonder_years_s04e07: ("/The Wonder Years/The.Wonder.Years.S04.PDTV.x264-JCH/The Wonder Years s04e07 Christmas Party NTSC PDTV.avi", "The Wonder Years", Some(4), Some(7));
        path_sopranos_s03e09: ("/The.Sopranos/Season 3/The Sopranos Season 3 Episode 09 - The Telltale Moozadell.avi", "The Sopranos", Some(3), Some(9));
        path_castle_rock_2x01: ("/Castle Rock 2x01 Que el rio siga su cursor [WEB-DL HULU 1080p h264 Dual DD5.1 Subs].mkv", "Castle Rock", Some(2), Some(1));
        path_after_life_1x06: ("/After Life 1x06 Episodio 6 [WEB-DL NF 1080p h264 Dual DD 5.1 Sub].mkv", "After Life", Some(1), Some(6));
        path_yamato_e03: ("/Season 4/Uchuu.Senkan.Yamato.2199.E03.avi", "Uchuu Senkan Yamato", Some(4), Some(3));
        path_daily_show_25x22: ("The Daily Show/The Daily Show 25x22 - [WEBDL-720p][AAC 2.0][x264] Noah Baumbach-TBS.mkv", "The Daily Show", Some(25), Some(22));
        path_watchmen_1x03: ("Watchmen (2019)/Watchmen 1x03 [WEBDL-720p][EAC3 5.1][h264][-TBS] - She Was Killed by Space Junk.mkv", "Watchmen", Some(1), Some(3));
        path_death_note_14_combined: ("[SOFCJ-Raws] Death Note - 14 (BDRip 1920x1080 x264 VFR 10bit FLAC)_combined.mkv", "Death Note", None, Some(14));
        path_cool_show_14: ("boring shows/Cool Show/Season 4/14.mkv", "Cool Show", Some(4), Some(14));
        // Real library: absolute episode number with explicit SxxExx in a group, parent
        // directory carries the season number.
        path_frieren_s02e10: ("[9volt] Sousou no Frieren - Season 2 (WEB 1080p HEVC EAC-3 Dual Audio)/[9volt] Sousou no Frieren - 38 (S02E10) (Dual Audio) (WEB 1080p HEVC EAC-3) [DA50C6DF].mkv", "Sousou no Frieren", Some(2), Some(10));
        path_frieren_s02e01: ("[9volt] Sousou no Frieren - Season 2 (WEB 1080p HEVC EAC-3 Dual Audio)/[9volt] Sousou no Frieren - 29 (S02E01) (Dual Audio) (WEB 1080p HEVC EAC-3) [E15A4F27].mkv", "Sousou no Frieren", Some(2), Some(1));
        // Real library: absolute episode number, no season anywhere -> best case is season 1.
        path_death_note_s01e14: ("Death Note/[SOFCJ-Raws] Death Note - 14 Friend (BDRip 1920x1080 x264 VFR 10bit FLAC).mp4", "Death Note", Some(1), Some(14));
        path_death_note_s01e01: ("Death Note/[SOFCJ-Raws] Death Note - 01 Rebirth (BDRip 1920x1080 x264 VFR 10bit FLAC).mp4", "Death Note", Some(1), Some(1));
    }

    episode_tests! {
        // `[a]` group tag glued to the name, underscores used as separators.
        lib_cowboy_bebop_underscores: ("[a]Cowboy_Bebop/[a]Cowboy_Bebop_01_[1080p_FLAC].mp4", "Cowboy Bebop", None, Some(1));
        // Lowercase ` x ` inside the title must not be read as an `NxNN` episode marker.
        lib_spy_x_family: ("[ASW] SPY x FAMILY - 01 [1080p HEVC x265 10Bit][AAC].mp4", "SPY x FAMILY", None, Some(1));
        // Semicolon inside the title is preserved.
        lib_steins_gate: ("[Commie] Steins;Gate/[Commie] Steins;Gate - 01 [BD 1080p FLAC] [1AE0722F].mp4", "Steins;Gate", None, Some(1));
        // Episode title made of bare numbers (`1 23 45`) must not hijack the episode number.
        lib_chernobyl_numeric_title: ("Chernobyl (2019)/Season 1/Chernobyl 1x01 - 1 23 45 [WEBDL-1080p][EAC3 5.1][h265]-TBS.mp4", "Chernobyl", Some(1), Some(1));
        // Hyphen inside a group tag (`[Erai-raws]`).
        lib_vinland_saga: ("[Erai-raws] Vinland Saga/[Erai-raws] Vinland Saga - 01 [1080p][Multiple Subtitle].mp4", "Vinland Saga", None, Some(1));
        // Capital `X` as a real word in the title.
        lib_hunter_x_hunter: ("Hunter X Hunter (2011)/Season 1/[HorribleSubs] Hunter X Hunter - 01 [720p].mp4", "Hunter X Hunter", Some(1), Some(1));
        // Zero-padded three digit absolute episode number.
        lib_naruto_001: ("Naruto Shippuden/Naruto Shippuden - 001 [720p].mp4", "Naruto Shippuden", None, Some(1));
        // `title - NN - episode title` layout.
        lib_evangelion_01: ("[NERV] Neon Genesis Evangelion/[NERV] Neon Genesis Evangelion - 01 - Angel Attack [BD 1080p HEVC FLAC].mp4", "Neon Genesis Evangelion", None, Some(1));
        // Bare `S02` season directory plus dash-delimited filename.
        lib_seinfeld_s02_dir: ("Seinfeld/S02/Seinfeld - S02E01 - The Ex-Girlfriend.mp4", "Seinfeld", Some(2), Some(1));
        // Alternate title in parentheses lives in the directory name only.
        lib_shingeki_alt_title: ("[SubsPlease] Shingeki no Kyojin (Attack on Titan)/Season 1/[SubsPlease] Shingeki no Kyojin - 01 (1080p) [BATCH].mp4", "Shingeki no Kyojin", Some(1), Some(1));
        // `(US)` country suffix in the directory, `US` repeated in the filename.
        lib_office_us: ("The Office (US)/Season 1/The.Office.US.S01E01.Pilot.720p.BluRay.x264.mp4", "The Office US", Some(1), Some(1));
        // Verbose `Season N Episode NN` where the episode title repeats the show name.
        lib_sopranos_verbose: ("The Sopranos/Season 1/The Sopranos Season 1 Episode 01 - The Sopranos.mp4", "The Sopranos", Some(1), Some(1));
        // `DD+5.1` audio tag next to the episode marker.
        lib_fleabag_ddplus: ("Fleabag.S01E01.1080p.AMZN.WEB-DL.DD+5.1.H.264-NTb.mp4", "Fleabag", Some(1), Some(1));
        // Release metadata in parentheses on the directory name.
        lib_fma_brotherhood: ("[Judas] Fullmetal Alchemist Brotherhood (BD 1080p x265 HEVC)/[Judas] Fullmetal Alchemist Brotherhood - 01 [BD 1080p HEVC x265 10bit FLAC].mp4", "Fullmetal Alchemist Brotherhood", None, Some(1));
        // KNOWN FAIL: 4-digit absolute number `1001` is split as `10x01`, overriding the
        // `Season 21` directory. Best case should be season 21, episode 1001.
        // lib_one_piece_1001: ("One Piece/Season 21/One Piece 1001.mp4", "One Piece", Some(21), Some(1001));
    }

    single_episode_tests! {
        no_season_simpsons_s25e08: ("The Simpsons/The Simpsons.S25E08.Steal this episode.mp4", Some(8));
        no_season_simpsons_02_ep_name: ("The Simpsons/The Simpsons - 02 - Ep Name.avi", Some(2));
        no_season_simpsons_02: ("The Simpsons/02.avi", Some(2));
        no_season_02_ep_name: ("The Simpsons/02 - Ep Name.avi", Some(2));
        no_season_02_dash_ep_name: ("The Simpsons/02-Ep Name.avi", Some(2));
        no_season_02_dot_epname: ("The Simpsons/02.EpName.avi", Some(2));
        no_season_simpsons_dash_02: ("The Simpsons/The Simpsons - 02.avi", Some(2));
        no_season_simpsons_02_space_ep_name: ("The Simpsons/The Simpsons - 02 Ep Name.avi", Some(2));
        no_season_gj_club_07: ("GJ Club (2013)/GJ Club - 07.mkv", Some(7));
        // WOW, HOW???
        // no_season_case_closed_317: ("Case Closed (1996-2007)/Case Closed - 317.mkv", Some(17));
        no_season_simpsons_5_02: ("The Simpsons/The Simpsons 5 - 02 - Ep Name.avi", Some(2));
        no_season_simpsons_5_02_space: ("The Simpsons/The Simpsons 5 - 02 Ep Name.avi", Some(2));
        // no_season_seinfeld_0807: ("Seinfeld/Seinfeld 0807 The Checks.avi", Some(7));
        no_season_case_closed_13: ("Case Closed (1996-2007)/Case Closed - 13.mkv", Some(13));
    }

    // This test contains a lot of multiepisode videos which are currently not supported
    #[allow(unused)]
    fn episode_number_test() {
        let tests = [
            ("Season 21/One Piece 1001", 1001),
            (
                "Watchmen (2019)/Watchmen 1x03 [WEBDL-720p][EAC3 5.1][h264][-TBS] - She Was Killed by Space Junk.mkv",
                3,
            ),
            (
                "The Daily Show/The Daily Show 25x22 - [WEBDL-720p][AAC 2.0][x264] Noah Baumbach-TBS.mkv",
                22,
            ),
            (
                "Castle Rock 2x01 Que el rio siga su cursor [WEB-DL HULU 1080p h264 Dual DD5.1 Subs].mkv",
                1,
            ),
            (
                "After Life 1x06 Episodio 6 [WEB-DL NF 1080p h264 Dual DD 5.1 Sub].mkv",
                6,
            ),
            ("Season 02/S02E03 blah.avi", 3),
            ("Season 2/02x03 - 02x04 - 02x15 - Ep Name.mp4", 3),
            ("Season 02/02x03 - x04 - x15 - Ep Name.mp4", 3),
            ("Season 1/01x02 blah.avi", 2),
            ("Season 1/S01x02 blah.avi", 2),
            ("Season 1/S01E02 blah.avi", 2),
            ("Season 2/Elementary - 02x03-04-15 - Ep Name.mp4", 3),
            ("Season 1/S01xE02 blah.avi", 2),
            ("Season 1/seriesname S01E02 blah.avi", 2),
            ("Season 2/Episode - 16.avi", 16),
            ("Season 2/Episode 16.avi", 16),
            ("Season 2/Episode 16 - Some Title.avi", 16),
            ("Season 2/16 Some Title.avi", 16),
            ("Season 2/16 - 12 Some Title.avi", 16),
            ("Season 2/7 - 12 Angry Men.avi", 7),
            ("Season 1/seriesname 01x02 blah.avi", 2),
            ("Season 25/The Simpsons.S25E09.Steal this episode.mp4", 9),
            ("Season 1/seriesname S01x02 blah.avi", 2),
            (
                "Season 2/Elementary - 02x03 - 02x04 - 02x15 - Ep Name.mp4",
                3,
            ),
            ("Season 1/seriesname S01xE02 blah.avi", 2),
            ("Season 02/Elementary - 02x03 - x04 - x15 - Ep Name.mp4", 3),
            ("Season 02/Elementary - 02x03x04x15 - Ep Name.mp4", 3),
            ("Season 2/02x03-04-15 - Ep Name.mp4", 3),
            ("Season 02/02x03-E15 - Ep Name.mp4", 3),
            ("Season 02/Elementary - 02x03-E15 - Ep Name.mp4", 3),
            ("Season 1/Elementary - S01E23-E24-E26 - The Woman.mp4", 23),
            ("Season 2009/S2009E23-E24-E26 - The Woman.mp4", 23),
            ("Season 2009/2009x02 blah.avi", 2),
            ("Season 2009/S2009x02 blah.avi", 2),
            ("Season 2009/S2009E02 blah.avi", 2),
            ("Season 2009/seriesname 2009x02 blah.avi", 2),
            ("Season 2009/Elementary - 2009x03x04x15 - Ep Name.mp4", 3),
            ("Season 2009/2009x03x04x15 - Ep Name.mp4", 3),
            ("Season 2009/Elementary - 2009x03-E15 - Ep Name.mp4", 3),
            ("Season 2009/S2009xE02 blah.avi", 2),
            (
                "Season 2009/Elementary - S2009E23-E24-E26 - The Woman.mp4",
                23,
            ),
            ("Season 2009/seriesname S2009xE02 blah.avi", 2),
            ("Season 2009/2009x03-E15 - Ep Name.mp4", 3),
            ("Season 2009/seriesname S2009E02 blah.avi", 2),
            ("Season 2009/2009x03 - 2009x04 - 2009x15 - Ep Name.mp4", 3),
            ("Season 2009/2009x03 - x04 - x15 - Ep Name.mp4", 3),
            ("Season 2009/seriesname S2009x02 blah.avi", 2),
            (
                "Season 2009/Elementary - 2009x03 - 2009x04 - 2009x15 - Ep Name.mp4",
                3,
            ),
            ("Season 2009/Elementary - 2009x03-04-15 - Ep Name.mp4", 3),
            ("Season 2009/2009x03-04-15 - Ep Name.mp4", 3),
            (
                "Season 2009/Elementary - 2009x03 - x04 - x15 - Ep Name.mp4",
                3,
            ),
            ("Season 1/02 - blah-02 a.avi", 2),
            ("Season 1/02 - blah.avi", 2),
            ("Season 2/02 - blah 14 blah.avi", 2),
            ("Season 2/02.avi", 2),
            ("Season 2/2. Infestation.avi", 2),
            (
                "The Wonder Years/The.Wonder.Years.S04.PDTV.x264-JCH/The Wonder Years s04e07 Christmas Party NTSC PDTV.avi",
                7,
            ),
            ("Running Man/Running Man S2017E368.mkv", 368),
            (
                "Season 2/[HorribleSubs] Hunter X Hunter - 136 [720p].mkv",
                136,
            ), // triple digit episode number
            (
                "Log Horizon 2/[HorribleSubs] Log Horizon 2 - 03 [720p].mkv",
                3,
            ), // digit in series name
            ("Season 1/seriesname 05.mkv", 5), // no hyphen between series name and episode number
            ("[BBT-RMX] Ranma ½ - 154 [50AC421A].mkv", 154), // hyphens in the pre-name info, triple digit episode number
            ("Season 2/Episode 21 - 94 Meetings.mp4", 21),   // Title starts with a number
            (
                "/The.Legend.of.Condor.Heroes.2017.V2.web-dl.1080p.h264.aac-hdctv/The.Legend.of.Condor.Heroes.2017.E07.V2.web-dl.1080p.h264.aac-hdctv.mkv",
                7,
            ),
            ("Season 3/The Series Season 3 Episode 9 - The title.avi", 9),
            ("Season 3/The Series S3 E9 - The title.avi", 9),
            ("Season 3/S003 E009.avi", 9),
            ("Season 3/Season 3 Episode 9.avi", 9),
            (
                "[VCB-Studio] Re Zero kara Hajimeru Isekai Seikatsu [21][Ma10p_1080p][x265_flac].mkv",
                21,
            ),
            (
                "[CASO&Sumisora][Oda_Nobuna_no_Yabou][04][BDRIP][1920x1080][x264_AAC][7620E503].mp4",
                4,
            ),
        ];
    }
}

/// Full show identifier representation
///
/// Usually constructed from episode file name and parent directories
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ShowIdentifier {
    pub episode: u16,
    pub season: u16,
    pub title: String,
    pub year: Option<u16>,
}

impl ShowIdentifier {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, ShowIdent> {
        let ident = Parser::parse_filename(path.as_ref(), ShowIdent::default());
        if let Some((episode, season)) = ident.episode.zip(ident.season) {
            Ok(Self {
                episode,
                season,
                title: ident.title,
                year: ident.year,
            })
        } else {
            Err(ident)
        }
    }
}

impl From<ShowIdentifier> for ContentIdentifier {
    fn from(val: ShowIdentifier) -> Self {
        ContentIdentifier::Show(val)
    }
}

impl TryFrom<ShowIdent> for ShowIdentifier {
    type Error = ShowIdent;

    fn try_from(ident: ShowIdent) -> Result<Self, Self::Error> {
        if let Some((episode, season)) = ident.episode.zip(ident.season) {
            Ok(Self {
                episode,
                season,
                title: ident.title,
                year: ident.year,
            })
        } else {
            Err(ident)
        }
    }
}

impl Media for ShowIdentifier {
    type Ident = ShowIdent;
    fn identify(path: impl AsRef<Path>) -> Result<Self, Self::Ident>
    where
        Self: Sized,
    {
        Self::from_path(path)
    }
    fn title(&self) -> &str {
        &self.title
    }
}
