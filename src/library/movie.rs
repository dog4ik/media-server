use std::path::Path;

use serde::Serialize;

use super::{
    ContentIdentifier, Media,
    identification::{Parseable as Parsable, Parser, SPECIAL_CHARS, Token},
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

    macro_rules! movie_tests {
        ($($name:ident: ($input:expr, $title:expr, $year:expr);)*) => {
            $(
                #[test]
                fn $name() {
                    let id = Parser::parse_filename(Path::new($input), MovieIdent::default());
                    let expected = MovieIdent { title: $title.into(), year: $year };
                    assert_eq!(expected, id, "mismatch for {:?}", $input);
                }
            )*
        };
    }

    movie_tests! {
        inception_2010: ("Inception.2010.1080p.BluRay.x264.YIFY", "Inception", Some(2010));
        the_matrix_1999: ("The.Matrix.1999.2160p.UHD.BluRay.x265.10bit.HDR.DTS-HD.MA.5.1-SWTYBLZ", "The Matrix", Some(1999));

        lib_blade_runner_2049: ("Blade.Runner.2049.2017.2160p.UHD.BluRay.x265.10bit.HDR.TrueHD.7.1.Atmos-TERMiNAL.mp4", "Blade Runner 2049", Some(2017));
        // Digit embedded inside a title word (`Se7en`).
        lib_se7en: ("Se7en.1995.REMASTERED.1080p.BluRay.x264-AMIABLE.mp4", "Se7en", Some(1995));
        // Title starts with a number.
        lib_12_angry_men: ("12 Angry Men (1957)/12.Angry.Men.1957.1080p.BluRay.x264-CtrlHD.mp4", "12 Angry Men", Some(1957));
        // Hyphenated title (`Spider-Man`, `Spider-Verse`).
        lib_spider_man: ("Spider-Man Into the Spider-Verse (2018)/Spider-Man.Into.the.Spider-Verse.2018.2160p.UHD.BluRay.x265.10bit.HDR.DTS-HD.MA.5.1-SWTYBLZ.mp4", "Spider Man Into the Spider Verse", Some(2018));
        // Filename title (romaji) differs from the directory title.
        lib_kimi_no_na_wa: ("Your Name (2016)/Kimi.no.Na.wa.2016.1080p.BluRay.x264.DTS-WiKi.mp4", "Kimi no Na wa", Some(2016));
        // Long multi-part title with an `EXTENDED` edition tag.
        lib_lotr_extended: ("The.Lord.of.the.Rings.The.Fellowship.of.the.Ring.2001.EXTENDED.2160p.UHD.BluRay.x265-BOREDOR.mp4", "The Lord of the Rings The Fellowship of the Ring", Some(2001));
        // Newer codec/audio tags (`AV1`, `OPUS`) and capitalised `1080P`.
        lib_resident_evil_av1: ("Resident.Evil.Vendetta.2017.Bluray.1080P.AV1.OPUS.5.1-DECK.mkv", "Resident Evil Vendetta", Some(2017));
    }
}
