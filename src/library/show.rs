use std::path::Path;

use serde::Serialize;

use super::{
    identification::{Parseable, Parser, Token, SPECIAL_CHARS},
    ContentIdentifier, Media,
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

#[derive(Debug, Clone, Serialize, Default)]
pub struct ShowIdent {
    pub episode: Option<u16>,
    pub season: Option<u16>,
    pub title: String,
    pub year: Option<u16>,
}

impl Parseable for ShowIdent {
    fn parse_parent<'a>(&mut self, directory_tokens: Vec<Token<'a>>) {
        self.apply_parent_tokens(&directory_tokens);
    }

    fn parse_name<'a>(&mut self, name_tokens: Vec<Token<'a>>) {
        self.apply_name_tokens(&name_tokens);
    }
}

const SEASON_IDENTS: [&str; 2] = ["Season", "season"];

impl ShowIdent {
    pub fn apply_parent_tokens(&mut self, tokens: &[Token<'_>]) {
        match (tokens.get(0), tokens.get(1)) {
            (Some(Token::Unknown(season_ident)), Some(Token::Unknown(season_num))) => {
                if SEASON_IDENTS.contains(season_ident) {
                    if let Ok(season_num) = season_num.parse() {
                        self.season = Some(season_num);
                        return;
                    }
                }
            }
            _ => {}
        }
        self.apply_name(tokens);
    }

    pub fn apply_name<'a>(&mut self, tokens: &[Token<'a>]) -> Vec<&'a str> {
        let mut title = String::new();
        let mut season = None;
        let mut episode = None;
        let mut year = None;
        let mut past_name = false;
        let mut in_group = false;
        let mut fallback_name_tokens = Vec::new();

        for (i, token) in tokens.iter().enumerate() {
            match token {
                Token::Unknown(t) => {
                    if in_group {
                        continue;
                    }
                    if let Some((s, e)) = parse_se_format(t).or_else(|| parse_0x0_episode(t)) {
                        season = Some(s);
                        episode = Some(e);
                        past_name = true;
                        continue;
                    }
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
                    fallback_name_tokens.push(*t);
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
                    past_name = true;
                }
                Token::Year(y) => {
                    year = Some(*y);
                    past_name = true;
                }
                Token::GroupStart => {
                    in_group = true;
                }
                Token::GroupEnd => {
                    in_group = false;
                }
                Token::ExplicitSeparator => {
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
            let mut nums = Vec::new();
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

    // Jellyfin tests
    #[test]
    fn simple_episodes_tests() {
        fn test_simple_episode(
            (input, show_name, season, episode): (&str, &str, Option<u16>, Option<u16>),
        ) {
            let identifier = Parser::parse_filename(Path::new(input), ShowIdent::default());
            assert_eq!(show_name, identifier.title);
            assert_eq!(season, identifier.season);
            assert_eq!(episode, identifier.episode);
        }
        let tests = [
            ("/server/anything_s01e02.mp4", "anything", Some(1), Some(2)),
            ("/server/anything_s1e2.mp4", "anything", Some(1), Some(2)),
            ("/server/anything_s01.e02.mp4", "anything", Some(1), Some(2)),
            ("/server/anything_102.mp4", "anything", Some(1), Some(2)),
            ("/server/anything_1x02.mp4", "anything", Some(1), Some(2)),
            ("/server/The Walking Dead 4x01.mp4", "The Walking Dead", Some(4), Some(1)),
            ("/server/the_simpsons-s02e01_18536.mp4", "the simpsons", Some(2), Some(1)),
            ("/server/Temp/S01E02 foo.mp4", "Temp", Some(1), Some(2)),
            ("Series/4x12 - The Woman.mp4", "Series", Some(4), Some(12)),
            // Current implementation thinks '.' is separator so resulting filename does not
            // contain it
            // ("Series/LA X, Pt. 1_s06e32.mp4", "LA X, Pt. 1", Some(6), Some(32)),
            ("[Baz-BarF,oo - [1080p][Multiple Subtitle]/[Baz-Bar] Foo - 05 [1080p][Multiple Subtitle].mkv", "Foo", None, Some(5)),
            ("/Foo/The.Series.Name.S01E04.WEBRip.x264-Baz[Bar/,the.series.name.s01e04.webrip.x264-Baz[Bar].mkv", "the series name", Some(1), Some(4)),
            ("Love.Death.and.Robots.S01.1080p.NF.WEB-DL.DDP5.1.x264-NTG/Love.Death.and.Robots.S01E01.Sonnies.Edge.1080p.NF.WEB-DL.DDP5.1.x264-NTG.mkv", "Love Death and Robots", Some(1), Some(1)),
            // We cannot know if text after explicit separator is the continuation of show name
            // ("[YuiSubs ,Tensura Nikki - Tensei Shitara Slime Datta Ken/[YuiSubs] Tensura Nikki - Tensei Shitara Slime Datta Ken - 12 (NVENC H.265 1080p).mkv", "Tensura Nikki Tensei Shitara Slime Datta Ken", None, Some(12)),
            ("[Baz-BarF,oo - 01 - 12[1080p][Multiple Subtitle]/[Baz-Bar] Foo - 05 [1080p][Multiple Subtitle].mkv", "Foo", None, Some(5)),
            ("Series/4-12 - The Woman.mp4", "Series", Some(4), Some(12)),
        ];
        for test in tests {
            test_simple_episode(test);
        }
    }
    fn show_resolver_test() {
        let tests = [
            ("The.Show.S01", "The Show"),
            ("The.Show.S01.COMPLETE", "The Show"),
            ("S.H.O.W.S01", "S.H.O.W"),
            ("The.Show.P.I.S01", "The Show P.I"),
            ("The_Show_Season_1", "The Show"),
            ("/something/The_Show/Season 10", "The Show"),
            ("The Show", "The Show"),
            ("/some/path/The Show", "The Show"),
            ("/some/path/The Show s02e10 720p hdtv", "The Show"),
            (
                "/some/path/The Show s02e10 the episode 720p hdtv",
                "The Show",
            ),
        ];
    }
    fn season_path_test() {
        let tests = [
            ("/Drive/Season 1", 1),
            ("/Drive/s1", 1),
            ("/Drive/S1", 1),
            ("/Drive/Season 2", 2),
            ("/Drive/Season 02", 2),
            ("/Drive/Seinfeld/S02", 2),
            ("/Drive/Seinfeld/2", 2),
            ("/Drive/Seinfeld - S02", 2),
            ("/Drive/Season 2009", 2009),
            ("/Drive/Season1", 1),
            ("The Wonder Years/The.Wonder.Years.S04.PDTV.x264-JCH", 4),
            ("/Drive/Season 7 (2016)", 7),
            ("/Drive/Staffel 7 (2016)", 7),
            ("/Drive/Stagione 7 (2016)", 7),
            ("/Drive/Season (8)", -1),
            ("/Drive/3.Staffel", 3),
            ("/Drive/s06e05", -1),
            (
                "/Drive/The.Legend.of.Condor.Heroes.2017.V2.web-dl.1080p.h264.aac-hdctv",
                -1,
            ),
            ("/Drive/extras", 0),
            ("/Drive/specials", 0),
        ];
    }

    fn seasons_number_tests() {
        let tests = [
            ("The Daily Show/The Daily Show 25x22 - [WEBDL-720p][AAC 2.0][x264] Noah Baumbach-TBS.mkv", 25),
            ("/Show/Season 02/S02E03 blah.avi", 2),
            ("Season 1/seriesname S01x02 blah.avi", 1),
            ("Season 1/S01x02 blah.avi", 1),
            ("Season 1/seriesname S01xE02 blah.avi", 1),
            ("Season 1/01x02 blah.avi", 1),
            ("Season 1/S01E02 blah.avi", 1),
            ("Season 1/S01xE02 blah.avi", 1),
            ("Season 1/seriesname 01x02 blah.avi", 1),
            ("Season 1/seriesname S01E02 blah.avi", 1),
            ("Season 2/Elementary - 02x03 - 02x04 - 02x15 - Ep Name.mp4", 2),
            ("Season 2/02x03 - 02x04 - 02x15 - Ep Name.mp4", 2),
            ("Season 2/02x03-04-15 - Ep Name.mp4", 2),
            ("Season 2/Elementary - 02x03-04-15 - Ep Name.mp4", 2),
            ("Season 02/02x03-E15 - Ep Name.mp4", 2),
            ("Season 02/Elementary - 02x03-E15 - Ep Name.mp4", 2),
            ("Season 02/02x03 - x04 - x15 - Ep Name.mp4", 2),
            ("Season 02/Elementary - 02x03 - x04 - x15 - Ep Name.mp4", 2),
            ("Season 02/02x03x04x15 - Ep Name.mp4", 2),
            ("Season 02/Elementary - 02x03x04x15 - Ep Name.mp4", 2),
            ("Season 1/Elementary - S01E23-E24-E26 - The Woman.mp4", 1),
            ("Season 1/S01E23-E24-E26 - The Woman.mp4", 1),
            ("Season 25/The Simpsons.S25E09.Steal this episode.mp4", 25),
            ("The Simpsons/The Simpsons.S25E09.Steal this episode.mp4", 25),
            ("2016/Season s2016e1.mp4", 2016),
            ("2016/Season 2016x1.mp4", 2016),
            ("Season 2009/2009x02 blah.avi", 2009),
            ("Season 2009/S2009x02 blah.avi", 2009),
            ("Season 2009/S2009E02 blah.avi", 2009),
            ("Season 2009/S2009xE02 blah.avi", 2009),
            ("Season 2009/seriesname 2009x02 blah.avi", 2009),
            ("Season 2009/seriesname S2009x02 blah.avi", 2009),
            ("Season 2009/seriesname S2009E02 blah.avi", 2009),
            ("Season 2009/Elementary - 2009x03 - 2009x04 - 2009x15 - Ep Name.mp4", 2009),
            ("Season 2009/2009x03 - 2009x04 - 2009x15 - Ep Name.mp4", 2009),
            ("Season 2009/2009x03-04-15 - Ep Name.mp4", 2009),
            ("Season 2009/Elementary - 2009x03 - x04 - x15 - Ep Name.mp4", 2009),
            ("Season 2009/2009x03x04x15 - Ep Name.mp4", 2009),
            ("Season 2009/Elementary - 2009x03x04x15 - Ep Name.mp4", 2009),
            ("Season 2009/Elementary - S2009E23-E24-E26 - The Woman.mp4", 2009),
            ("Season 2009/S2009E23-E24-E26 - The Woman.mp4", 2009),
            ("Series/1-12 - The Woman.mp4", 1),
            ("Running Man/Running Man S2017E368.mkv", 2017),
            ("Case Closed (1996-2007)/Case Closed - 317.mkv", 3),
            ("Seinfeld/Seinfeld 0807 The Checks.avi", 8),
        ];
    }

    #[test]
    fn episodes_path_test() {
        fn test_simple_episode(
            (input, show_name, season, episode): (&str, &str, Option<u16>, Option<u16>),
        ) {
            let identifier = Parser::parse_filename(Path::new(input), ShowIdent::default());
            assert_eq!(show_name, identifier.title);
            assert_eq!(season, identifier.season);
            assert_eq!(episode, identifier.episode);
        }
        let tests = [
            ("/media/Foo/Foo-S01E01", "Foo", Some(1), Some(1)),
            ("/media/Foo - S04E011", "Foo", Some(4), Some(11)),
            ("/media/Foo/Foo s01x01", "Foo", Some(1), Some(1)),
            ("/media/Foo (2019)/Season 4/Foo (2019).S04E03.mp4", "Foo", Some(4), Some(3)),
            ("/Season 2/Elementary - 02x03-04-15 - Ep Name.mp4", "Elementary", Some(2), Some(3)),
            ("/Season 1/seriesname S01E02 blah.avi", "seriesname", Some(1), Some(2)),
            ("/Running Man/Running Man S2017E368.mkv", "Running Man", Some(2017), Some(368)),
            ("/Season 1/seriesname 01x02 blah.avi", "seriesname", Some(1), Some(2)),
            ("/Season 25/The Simpsons.S25E09.Steal this episode.mp4", "The Simpsons", Some(25), Some(9)),
            ("/Season 1/seriesname S01x02 blah.avi", "seriesname", Some(1), Some(2)),
            ("/Season 1/seriesname S01xE02 blah.avi", "seriesname", Some(1), Some(2)),
            // Multi episodes are not yet supported!
            //("/Season 2/Elementary - 02x03 - 02x04 - 02x15 - Ep Name.mp4", "Elementary", 2, 3),
            //("/Season 02/Elementary - 02x03 - x04 - x15 - Ep Name.mp4", "Elementary", 2, 3),
            //("/Season 02/Elementary - 02x03x04x15 - Ep Name.mp4", "Elementary", 2, 3),
            //("/Season 02/Elementary - 02x03-E15 - Ep Name.mp4", "Elementary", 2, 3),
            //("/Season 1/Elementary - S01E23-E24-E26 - The Woman.mp4", "Elementary", 1, 23),
            ("/The Wonder Years/The.Wonder.Years.S04.PDTV.x264-JCH/The Wonder Years s04e07 Christmas Party NTSC PDTV.avi", "The Wonder Years", Some(4), Some(7)),
            ("/The.Sopranos/Season 3/The Sopranos Season 3 Episode 09 - The Telltale Moozadell.avi", "The Sopranos", Some(3), Some(9)),
            ("/Castle Rock 2x01 Que el rio siga su curso [WEB-DL HULU 1080p h264 Dual DD5.1 Subs].mkv", "Castle Rock", Some(2), Some(1)),
            ("/After Life 1x06 Episodio 6 [WEB-DL NF 1080p h264 Dual DD 5.1 Sub].mkv", "After Life", Some(1), Some(6)),
            ("/Season 4/Uchuu.Senkan.Yamato.2199.E03.avi", "Uchuu Senkan Yamato", Some(4), Some(3)),
            ("The Daily Show/The Daily Show 25x22 - [WEBDL-720p][AAC 2.0][x264] Noah Baumbach-TBS.mkv", "The Daily Show", Some(25), Some(22)),
            ("Watchmen (2019)/Watchmen 1x03 [WEBDL-720p][EAC3 5.1][h264][-TBS] - She Was Killed by Space Junk.mkv", "Watchmen", Some(1), Some(3)),
            // ??? where the heck is season in this test
            //("/The.Legend.of.Condor.Heroes.2017.V2.web-dl.1080p.h264.aac-hdctv/The.Legend.of.Condor.Heroes.2017.E07.V2.web-dl.1080p.h264.aac-hdctv.mkv", "The Legend of Condor Heroes", 1, 7),
            ("[SOFCJ-Raws] Death Note - 14 (BDRip 1920x1080 x264 VFR 10bit FLAC)_combined.mkv", "Death Note", None, Some(14)),
            ("boring shows/Cool Show/Season 4/14.mkv", "Cool Show", Some(4), Some(14))
        ];
        for test in tests {
            test_simple_episode(test);
        }
    }

    #[test]
    fn episodes_without_season_test() {
        fn test_episodes_without_season((input, episode): (&str, u16)) {
            let identifier = Parser::parse_filename(Path::new(input), ShowIdent::default());
            assert_eq!(Some(episode), identifier.episode);
        }
        let tests = [
            ("The Simpsons/The Simpsons.S25E08.Steal this episode.mp4", 8),
            ("The Simpsons/The Simpsons - 02 - Ep Name.avi", 2),
            ("The Simpsons/02.avi", 2),
            ("The Simpsons/02 - Ep Name.avi", 2),
            ("The Simpsons/02-Ep Name.avi", 2),
            ("The Simpsons/02.EpName.avi", 2),
            ("The Simpsons/The Simpsons - 02.avi", 2),
            ("The Simpsons/The Simpsons - 02 Ep Name.avi", 2),
            ("GJ Club (2013)/GJ Club - 07.mkv", 7),
            // WOW, HOW???
            // ("Case Closed (1996-2007)/Case Closed - 317.mkv", 17),
            ("The Simpsons/The Simpsons 5 - 02 - Ep Name.avi", 2),
            ("The Simpsons/The Simpsons 5 - 02 Ep Name.avi", 2),
            // ("Seinfeld/Seinfeld 0807 The Checks.avi", 7),
            ("Case Closed (1996-2007)/Case Closed - 13.mkv", 13),
        ];
        for test in tests {
            test_episodes_without_season(test);
        }
    }

    fn episode_number_test() {
        let tests = [
            ("Season 21/One Piece 1001", 1001),
            ("Watchmen (2019)/Watchmen 1x03 [WEBDL-720p][EAC3 5.1][h264][-TBS] - She Was Killed by Space Junk.mkv", 3),
            ("The Daily Show/The Daily Show 25x22 - [WEBDL-720p][AAC 2.0][x264] Noah Baumbach-TBS.mkv", 22),
            ("Castle Rock 2x01 Que el rio siga su curso [WEB-DL HULU 1080p h264 Dual DD5.1 Subs].mkv", 1),
            ("After Life 1x06 Episodio 6 [WEB-DL NF 1080p h264 Dual DD 5.1 Sub].mkv", 6),
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
            ("Season 2/Elementary - 02x03 - 02x04 - 02x15 - Ep Name.mp4", 3),
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
            ("Season 2009/Elementary - S2009E23-E24-E26 - The Woman.mp4", 23),
            ("Season 2009/seriesname S2009xE02 blah.avi", 2),
            ("Season 2009/2009x03-E15 - Ep Name.mp4", 3),
            ("Season 2009/seriesname S2009E02 blah.avi", 2),
            ("Season 2009/2009x03 - 2009x04 - 2009x15 - Ep Name.mp4", 3),
            ("Season 2009/2009x03 - x04 - x15 - Ep Name.mp4", 3),
            ("Season 2009/seriesname S2009x02 blah.avi", 2),
            ("Season 2009/Elementary - 2009x03 - 2009x04 - 2009x15 - Ep Name.mp4", 3),
            ("Season 2009/Elementary - 2009x03-04-15 - Ep Name.mp4", 3),
            ("Season 2009/2009x03-04-15 - Ep Name.mp4", 3),
            ("Season 2009/Elementary - 2009x03 - x04 - x15 - Ep Name.mp4", 3),
            ("Season 1/02 - blah-02 a.avi", 2),
            ("Season 1/02 - blah.avi", 2),
            ("Season 2/02 - blah 14 blah.avi", 2),
            ("Season 2/02.avi", 2),
            ("Season 2/2. Infestation.avi", 2),
            ("The Wonder Years/The.Wonder.Years.S04.PDTV.x264-JCH/The Wonder Years s04e07 Christmas Party NTSC PDTV.avi", 7),
            ("Running Man/Running Man S2017E368.mkv", 368),
            ("Season 2/[HorribleSubs] Hunter X Hunter - 136 [720p].mkv", 136), // triple digit episode number
            ("Log Horizon 2/[HorribleSubs] Log Horizon 2 - 03 [720p].mkv", 3), // digit in series name
            ("Season 1/seriesname 05.mkv", 5), // no hyphen between series name and episode number
            ("[BBT-RMX] Ranma ½ - 154 [50AC421A].mkv", 154), // hyphens in the pre-name info, triple digit episode number
            ("Season 2/Episode 21 - 94 Meetings.mp4", 21), // Title starts with a number
            ("/The.Legend.of.Condor.Heroes.2017.V2.web-dl.1080p.h264.aac-hdctv/The.Legend.of.Condor.Heroes.2017.E07.V2.web-dl.1080p.h264.aac-hdctv.mkv", 7),
            ("Season 3/The Series Season 3 Episode 9 - The title.avi", 9),
            ("Season 3/The Series S3 E9 - The title.avi", 9),
            ("Season 3/S003 E009.avi", 9),
            ("Season 3/Season 3 Episode 9.avi", 9),
            ("[VCB-Studio] Re Zero kara Hajimeru Isekai Seikatsu [21][Ma10p_1080p][x265_flac].mkv", 21),
            ("[CASO&Sumisora][Oda_Nobuna_no_Yabou][04][BDRIP][1920x1080][x264_AAC][7620E503].mp4", 4),
        ];
    }
}

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
