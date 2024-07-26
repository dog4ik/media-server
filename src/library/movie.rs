use serde::Serialize;

use super::{ContentIdentifier, Media};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MovieIdentifier {
    pub title: String,
    pub year: Option<u32>,
}

impl From<MovieIdentifier> for ContentIdentifier {
    fn from(val: MovieIdentifier) -> Self {
        ContentIdentifier::Movie(val)
    }
}

impl Media for MovieIdentifier {
    fn identify(tokens: &[String]) -> Option<Self> {
        let mut name: Option<String> = None;
        let mut year: Option<u32> = None;
        for token in tokens {
            let chars: Vec<char> = token.chars().collect();
            let is_year = token.len() == 6
                && chars[0] == '('
                && chars[1].is_ascii_digit()
                && chars[2].is_ascii_digit()
                && chars[3].is_ascii_digit()
                && chars[4].is_ascii_digit()
                && chars[5] == ')'
                && (chars[1] == '1' || chars[1] == '2');
            if is_year {
                if let Ok(parsed_year) = token[1..6].parse() {
                    year = Some(parsed_year);
                }
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
                None => name = Some(token.to_string()),
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

#[cfg(test)]
mod tests {
    use crate::{library::Media, utils};

    use super::MovieIdentifier;

    #[test]
    fn identify_movies() {
        let tests = [
            (
                "Inception.2010.1080p.BluRay.x264.YIFY",
                MovieIdentifier {
                    title: "inception".into(),
                    year: None,
                },
            ),
            (
                "The.Matrix.1999.2160p.UHD.BluRay.x265.10bit.HDR.DTS-HD.MA.5.1-SWTYBLZ",
                MovieIdentifier {
                    title: "the matrix".into(),
                    year: None,
                },
            ),
        ];
        for (input, expected) in tests {
            let tokens = utils::tokenize_filename(input);
            assert_eq!(MovieIdentifier::identify(&tokens).unwrap(), expected);
        }
    }
}
