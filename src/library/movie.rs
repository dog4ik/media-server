use serde::Serialize;

use super::Media;

#[derive(Debug, Clone, Serialize)]
pub struct MovieIdentifier {
    pub title: String,
    pub year: Option<u32>,
}

impl Media for MovieIdentifier {
    fn identify(tokens: Vec<String>) -> Option<Self> {
        let mut name: Option<String> = None;
        let mut year: Option<u32> = None;
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
            if is_year {
                let _ = token[1..6].parse().map(|y| year = Some(y));
                continue;
            }

            // break when quality encountered, no more useful information
            if (token.len() == 4 || token.len() == 5)
                && chars[0].is_ascii_digit()
                && chars[1].is_ascii_digit()
                && chars[2].is_ascii_digit()
                && chars[3].is_ascii_digit()
            {
                break;
            }
            match name {
                Some(ref mut n) => n.push_str(&format!(" {}", token)),
                None => name = Some(token),
            }
        }
        if let Some(name) = name {
            let show_file = Self { title: name, year };
            Some(show_file)
        } else {
            None
        }
    }
    fn title(&self) -> &str {
        &self.title
    }
}
