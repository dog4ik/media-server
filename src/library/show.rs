use serde::Serialize;

use super::Media;

#[derive(Debug, Clone, Serialize)]
pub struct ShowIdentifier {
    pub episode: u8,
    pub season: u8,
    pub title: String,
}

impl Media for ShowIdentifier {
    fn identify(tokens: &Vec<String>) -> Option<Self> {
        let mut name: Option<String> = None;
        let mut season: Option<u8> = None;
        let mut episode: Option<u8> = None;

        for token in tokens {
            let chars: Vec<char> = token.chars().into_iter().collect();
            let is_year = token.len() == 6
                && chars[0] == '('
                && chars[1].is_ascii_digit()
                && chars[2].is_ascii_digit()
                && chars[3].is_ascii_digit()
                && chars[4].is_ascii_digit()
                && chars[5] == ')'
                && (chars[1] == '1' || chars[1] == '2');
            let is_year_appendix = token.len() == 4
                && chars[0].is_ascii_digit()
                && chars[1].is_ascii_digit()
                && chars[2].is_ascii_digit()
                && chars[3].is_ascii_digit()
                && (chars[0] == '1' || chars[0] == '2');
            if (is_year || is_year_appendix) && season.is_none() && episode.is_none() {
                continue;
            }

            if token.len() >= 6
                && chars[0] == 's'
                && chars[1].is_ascii_digit()
                && chars[2].is_ascii_digit()
                && chars[3] == 'e'
                && chars[4].is_ascii_digit()
                && chars[5].is_ascii_digit()
            {
                let s: Option<u8> = token[1..3].parse().ok();
                let e: Option<u8> = token[4..6].parse().ok();
                if let (Some(se), Some(ep)) = (s, e) {
                    season = Some(se);
                    episode = Some(ep);
                    break;
                };
            }
            match name {
                Some(ref mut n) => n.push_str(&format!(" {}", token)),
                None => name = Some(token.to_string()),
            }
        }
        if let (Some(name), Some(season), Some(episode)) = (name.clone(), season, episode) {
            let show_file = Self {
                episode,
                season,
                title: name,
            };
            Some(show_file)
        } else {
            None
        }
    }
    fn title(&self) -> &str {
        &self.title
    }
}
