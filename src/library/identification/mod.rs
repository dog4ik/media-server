const EXTRAS_FOLDERS: [&str; 11] = [
    "behind the scenes",
    "deleted scenes",
    "interviews",
    "scenes",
    "samples",
    "shorts",
    "featurettes",
    "clips",
    "other",
    "extras",
    "trailers",
];

const NAME_NOISE: &[&str] = &[
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
    "pal",
    "pdtv",
    "proper",
    "repack",
    "rerip",
    "r5",
    "bd5",
    "bd",
    "se",
    "svcd",
    "nfo",
    "nfofix",
    "ws",
    "ts",
    "tc",
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
    "aac",
    "dts",
];

pub const OPEN_BRACKETS: &[char] = &['(', '[', '{'];
pub const CLOSE_BRACKETS: &[char] = &[')', ']', '}'];
pub const SEPARATORS: &[char] = &['_', '.', ' '];

#[derive(Debug, PartialEq, Eq)]
enum Token<'a> {
    Name(&'a str),
    Noise(&'a str),
    Year(u16),
    Episode(u16),
    Season(u16),
    SeasonEpisode((u16, u16)),
}

fn visit_season(value: &str) -> Option<u16> {
    if value.len() != 3 && value.chars().nth(0).unwrap() != 's' {
        return None;
    }
    value[1..3].parse().ok()
}

fn visit_episode(value: &str) -> Option<u16> {
    if value.len() != 3 && value.chars().nth(0).unwrap() != 'e' {
        return None;
    }
    value[1..3].parse().ok()
}

fn visit_season_episode(value: &str) -> Option<(u16, u16)> {
    if value.len() == 6 {
        let season = visit_season(&value[..3])?;
        let episode = visit_episode(&value[3..6])?;
        return Some((season, episode));
    }
    None
}

fn visit_year(value: &str) -> Option<u16> {
    if value.len() != 4 {
        return None;
    }
    value.parse().ok()
}

pub fn tokenize_show<'a>(file_name: &'a str) -> Vec<Token<'a>> {
    let is_spaced = file_name.contains(' ');
    let raw_tokens = match is_spaced {
        true => file_name.split(' '),
        false => file_name.split('.'),
    };
    let mut past_name = false;
    let mut need_name = true;
    let mut group_tag = None;
    let mut tokens = Vec::new();
    for mut token in raw_tokens {
        if let Some(stripped_token) = token.strip_prefix(OPEN_BRACKETS) {
            token = stripped_token;
            group_tag = Some(token.chars().next().unwrap());
            if !need_name {
                past_name = true;
            }
        }
        if let Some(stripped_token) = group_tag.and_then(|t| token.strip_suffix(t)) {
            token = stripped_token;
            group_tag = None;
        }
        if NAME_NOISE.contains(&token) {
            past_name = true;
            tokens.push(Token::Noise(token));
            continue;
        }
        if let Some(season_episode) = visit_season_episode(token) {
            tokens.push(Token::SeasonEpisode(season_episode));
            past_name = true;
            continue;
        }
        if let Some(season) = visit_season(token) {
            tokens.push(Token::Season(season));
            past_name = true;
            continue;
        }
        if let Some(episode) = visit_episode(token) {
            tokens.push(Token::Episode(episode));
            past_name = true;
            continue;
        }
        if let Some(year) = visit_year(token) {
            tokens.push(Token::Year(year));
            past_name = true;
            continue;
        }
        if group_tag.is_none() && !past_name {
            tokens.push(Token::Name(token));
            need_name = false;
            continue;
        }
        tokens.push(Token::Noise(token));
    }

    tokens
}

#[cfg(test)]
mod tests {
    use crate::library::identification::tokenize_show;

    use super::Token;

    fn test<'a>(tests: impl IntoIterator<Item = (&'a str, Vec<Token<'a>>)>) {
        for (test, expected) in tests {
            let test = test.to_lowercase();
            assert_eq!(expected, tokenize_show(&test));
        }
    }

    #[test]
    pub fn show_names() {
        let tests = [
            (
                "Cyberpunk.Edgerunners.S01E02.DUBBED.1080p.WEBRip.x265-RARBG[eztv.re]",
                vec![
                    Token::Name("cyberpunk"),
                    Token::Name("edgerunners"),
                    Token::SeasonEpisode((1, 2)),
                    Token::Noise("dubbed"),
                    Token::Noise("1080p"),
                    Token::Noise("webrip"),
                    Token::Noise("x265-rarbg[eztv"),
                    Token::Noise("re]"),
                ],
            ),
            (
                "shogun.2024.s01e05.2160p.web.h265-successfulcrab",
                vec![
                    Token::Name("shogun"),
                    Token::Year(2024),
                    Token::SeasonEpisode((1, 5)),
                    Token::Noise("2160p"),
                    Token::Noise("web"),
                    Token::Noise("h265-successfulcrab"),
                ],
            ),
        ];
        test(tests);
    }

    #[test]
    fn movie_names() {
        let tests = [(
            "Aladdin.WEB-DL.KP.1080p-SOFCJ",
            vec![
                Token::Name("aladdin"),
                Token::Noise("web-dl"),
                Token::Noise("kp"),
                Token::Noise("1080p-sofcj"),
            ],
        )];
        test(tests);
    }


    fn simple_episodes_tests() {
        let tests = [
                ("/server/anything_s01e02.mp4", "anything", 1, 2),
                ("/server/anything_s1e2.mp4", "anything", 1, 2),
                ("/server/anything_s01.e02.mp4", "anything", 1, 2),
                ("/server/anything_102.mp4", "anything", 1, 2),
                ("/server/anything_1x02.mp4", "anything", 1, 2),
                ("/server/The Walking Dead 4x01.mp4", "The Walking Dead", 4, 1),
                ("/server/the_simpsons-s02e01_18536.mp4", "the_simpsons", 2, 1),
                ("/server/Temp/S01E02 foo.mp4", "", 1, 2),
                ("Series/4x12 - The Woman.mp4", "", 4, 12),
                ("Series/LA X, Pt. 1_s06e32.mp4", "LA X, Pt. 1", 6, 32),
                ("[Baz-BarF,oo - [1080p][Multiple Subtitle]/[Baz-Bar] Foo - 05 [1080p][Multiple Subtitle].mkv", "Foo", 1, 5),
                ("/Foo/The.Series.Name.S01E04.WEBRip.x264-Baz[Bar/,the.series.name.s01e04.webrip.x264-Baz[Bar].mkv", "The.Series.Name", 1, 4),
                ("Love.Death.and.Robots.S01.1080p.NF.WEB-DL.DDP5.1.x264-NTG/Love.Death.and.Robots.S01E01.Sonnies.Edge.1080p.NF.WEB-DL.DDP5.1.x264-NTG.mkv", "Love.Death.and.Robots", 1, 1),
                ("[YuiSubs ,Tensura Nikki - Tensei Shitara Slime Datta Ken/[YuiSubs] Tensura Nikki - Tensei Shitara Slime Datta Ken - 12 (NVENC H.265 1080p).mkv", "Tensura Nikki - Tensei Shitara Slime Datta Ken", 1, 12),
                ("[Baz-BarF,oo - 01 - 12[1080p][Multiple Subtitle]/[Baz-Bar] Foo - 05 [1080p][Multiple Subtitle].mkv", "Foo", 1, 5),
                ("Series/4-12 - The Woman.mp4", "", 4, 12),
    ];
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
            ("/Drive/Season 1", 1, true),
            ("/Drive/s1", 1, true),
            ("/Drive/S1", 1, true),
            ("/Drive/Season 2", 2, true),
            ("/Drive/Season 02", 2, true),
            ("/Drive/Seinfeld/S02", 2, true),
            ("/Drive/Seinfeld/2", 2, true),
            ("/Drive/Seinfeld - S02", 2, true),
            ("/Drive/Season 2009", 2009, true),
            ("/Drive/Season1", 1, true),
            (
                "The Wonder Years/The.Wonder.Years.S04.PDTV.x264-JCH",
                4,
                true,
            ),
            ("/Drive/Season 7 (2016)", 7, false),
            ("/Drive/Staffel 7 (2016)", 7, false),
            ("/Drive/Stagione 7 (2016)", 7, false),
            ("/Drive/Season (8)", -1, false),
            ("/Drive/3.Staffel", 3, false),
            ("/Drive/s06e05", -1, false),
            (
                "/Drive/The.Legend.of.Condor.Heroes.2017.V2.web-dl.1080p.h264.aac-hdctv",
                -1,
                false,
            ),
            ("/Drive/extras", 0, true),
            ("/Drive/specials", 0, true),
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

    fn episodes_path_test() {
        let tests = [
            ("/media/Foo/Foo-S01E01", "Foo", 1, 1),
            ("/media/Foo - S04E011", "Foo", 4, 11),
            ("/media/Foo/Foo s01x01", "Foo", 1, 1),
            ("/media/Foo (2019)/Season 4/Foo (2019).S04E03", "Foo (2019)", 4, 3),
            (r#"D:\media\Foo\Foo-S01E01"#, "Foo", 1, 1),
            (r#"D:\media\Foo - S04E011"#, "Foo", 4, 11),
            (r#"D:\media\Foo\Foo s01x01"#, "Foo", 1, 1),
            (r#"D:\media\Foo (2019)\Season 4\Foo (2019).S04E03"#, "Foo (2019)", 4, 3),
            ("/Season 2/Elementary - 02x03-04-15 - Ep Name.mp4", "Elementary", 2, 3),
            ("/Season 1/seriesname S01E02 blah.avi", "seriesname", 1, 2),
            ("/Running Man/Running Man S2017E368.mkv", "Running Man", 2017, 368),
            ("/Season 1/seriesname 01x02 blah.avi", "seriesname", 1, 2),
            ("/Season 25/The Simpsons.S25E09.Steal this episode.mp4", "The Simpsons", 25, 9),
            ("/Season 1/seriesname S01x02 blah.avi", "seriesname", 1, 2),
            ("/Season 2/Elementary - 02x03 - 02x04 - 02x15 - Ep Name.mp4", "Elementary", 2, 3),
            ("/Season 1/seriesname S01xE02 blah.avi", "seriesname", 1, 2),
            ("/Season 02/Elementary - 02x03 - x04 - x15 - Ep Name.mp4", "Elementary", 2, 3),
            ("/Season 02/Elementary - 02x03x04x15 - Ep Name.mp4", "Elementary", 2, 3),
            ("/Season 02/Elementary - 02x03-E15 - Ep Name.mp4", "Elementary", 2, 3),
            ("/Season 1/Elementary - S01E23-E24-E26 - The Woman.mp4", "Elementary", 1, 23),
            ("/The Wonder Years/The.Wonder.Years.S04.PDTV.x264-JCH/The Wonder Years s04e07 Christmas Party NTSC PDTV.avi", "The Wonder Years", 4, 7),
            ("/The.Sopranos/Season 3/The Sopranos Season 3 Episode 09 - The Telltale Moozadell.avi", "The Sopranos", 3, 9),
            ("/Castle Rock 2x01 Que el rio siga su curso [WEB-DL HULU 1080p h264 Dual DD5.1 Subs].mkv", "Castle Rock", 2, 1),
            ("/After Life 1x06 Episodio 6 [WEB-DL NF 1080p h264 Dual DD 5.1 Sub].mkv", "After Life", 1, 6),
            ("/Season 4/Uchuu.Senkan.Yamato.2199.E03.avi", "Uchuu Senkan Yamoto 2199", 4, 3),
            ("The Daily Show/The Daily Show 25x22 - [WEBDL-720p][AAC 2.0][x264] Noah Baumbach-TBS.mkv", "The Daily Show", 25, 22),
            ("Watchmen (2019)/Watchmen 1x03 [WEBDL-720p][EAC3 5.1][h264][-TBS] - She Was Killed by Space Junk.mkv", "Watchmen (2019)", 1, 3),
            ("/The.Legend.of.Condor.Heroes.2017.V2.web-dl.1080p.h264.aac-hdctv/The.Legend.of.Condor.Heroes.2017.E07.V2.web-dl.1080p.h264.aac-hdctv.mkv", "The Legend of Condor Heroes 2017", 1, 7),
        ];
    }

    fn episodes_without_season_test() {
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
            ("Case Closed (1996-2007)/Case Closed - 317.mkv", 17),
            (r#"The Simpsons/The Simpsons 5 - 02 - Ep Name.avi"#, 2),
            (r#"The Simpsons/The Simpsons 5 - 02 Ep Name.avi"#, 2),
            (r#"Seinfeld/Seinfeld 0807 The Checks.avi"#, 7),
            // This is not supported anymore after removing the episode number 365+ hack from EpisodePathParser
            (r#"Case Closed (1996-2007)/Case Closed - 13.mkv"#, 13),
        ];
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
