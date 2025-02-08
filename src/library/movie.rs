use std::path::Path;

use serde::Serialize;

use super::{
    identification::{Parseable as Parsable, Parser, Token, SPECIAL_CHARS},
    ContentIdentifier, Media,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MovieIdent {
    pub title: String,
    pub year: Option<u16>,
}

impl Parsable for MovieIdent {
    fn parse_parent(&mut self, folder_tokens: Vec<Token<'_>>) {
        self.parse_tokens(folder_tokens);
    }

    fn parse_name(&mut self, name_tokens: Vec<Token<'_>>) {
        self.parse_tokens(name_tokens);
    }
}

impl MovieIdent {
    pub fn parse_tokens(&mut self, tokens: Vec<Token<'_>>) {
        let mut past_name = false;
        let mut title = String::new();
        let mut in_group = false;
        let mut year = None;
        for token in tokens.into_iter() {
            match token {
                Token::Unknown(t) => {
                    if !past_name && !in_group {
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
                    year = Some(y);
                    past_name = true;
                }
                Token::GroupStart => {
                    in_group = true;
                    if !title.is_empty() {
                        past_name = true;
                    }
                }
                Token::ExplicitSeparator => {
                    past_name = true;
                }
                Token::GroupEnd => {
                    in_group = false;
                }
            }
        }

        let title = title.trim_matches(SPECIAL_CHARS).to_string();

        if !title.is_empty() {
            self.title = title;
        }
        self.year = year.or(self.year);
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, utoipa::ToSchema)]
pub struct MovieIdentifier {
    pub title: String,
    pub year: Option<u16>,
}

impl MovieIdentifier {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, MovieIdent> {
        let ident = Parser::parse_filename(path.as_ref(), MovieIdent::default());
        if ident.title.is_empty() {
            Err(ident)
        } else {
            Ok(Self {
                title: ident.title,
                year: ident.year,
            })
        }
    }
}

impl From<MovieIdentifier> for ContentIdentifier {
    fn from(val: MovieIdentifier) -> Self {
        ContentIdentifier::Movie(val)
    }
}

impl Media for MovieIdentifier {
    type Ident = MovieIdent;
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::library::{identification::Parser, movie::MovieIdent};

    #[test]
    fn identify_movies() {
        let tests = [
            (
                "Inception.2010.1080p.BluRay.x264.YIFY",
                MovieIdent {
                    title: "Inception".into(),
                    year: Some(2010),
                },
            ),
            (
                "The.Matrix.1999.2160p.UHD.BluRay.x265.10bit.HDR.DTS-HD.MA.5.1-SWTYBLZ",
                MovieIdent {
                    title: "The Matrix".into(),
                    year: Some(1999),
                },
            ),
        ];
        for (input, expected) in tests {
            let identifier = Parser::parse_filename(Path::new(input), MovieIdent::default());
            assert_eq!(identifier, expected);
        }
    }
}
