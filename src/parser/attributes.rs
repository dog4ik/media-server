use crate::parser::{symbol::Symbol, tokenizer::Token};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
pub enum ResolutionAttr {
    #[serde(rename = "480p")]
    P480,
    #[serde(rename = "720p")]
    P720,
    #[serde(rename = "1080p")]
    P1080,
    #[serde(rename = "2160p")]
    P2160,
}

impl ResolutionAttr {
    fn from_height(height: u16) -> Option<Self> {
        Some(match height {
            480 => Self::P480,
            720 => Self::P720,
            1080 => Self::P1080,
            2160 => Self::P2160,
            _ => return None,
        })
    }

    /// Recognize `1080p`, `720i`, `1920x1080`, `4k` and friends.
    pub fn from_symbol(symbol: Symbol<'_>) -> Option<Self> {
        let value = symbol.0;
        for alias in ["4k", "uhd", "ultrahd"] {
            if value.eq_ignore_ascii_case(alias) {
                return Some(Self::P2160);
            }
        }
        if let Some(height) = value.strip_suffix(['p', 'P', 'i', 'I']) {
            return Self::from_height(height.parse().ok()?);
        }
        let (width, height) = value.split_once(['x', 'X'])?;
        width.parse::<u16>().ok()?;
        Self::from_height(height.parse().ok()?)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceAttr {
    Web,
    WebDl,
    WebRip,
    BluRay,
    Dvd,
    Telesync,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CodecAttr {
    H264,
    H265,
    Av1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Tag {
    Dubbed,
    Subbed,
    DualAudio,
    MultiAudio,
    MultiSubs,
    Uncensored,
    Hdr,
    Extended,
    Remastered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case", untagged)]
pub enum Attribute {
    Resolution(ResolutionAttr),
    Source(SourceAttr),
    Codec(CodecAttr),
    Tag(Tag),
}

/// A recognized attribute together with the number of tokens it covers
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    pub attribute: Attribute,
    pub consumed: usize,
}

static PHRASES: &[(&[&str], Attribute)] = &[
    // Source
    (&["web"], Attribute::Source(SourceAttr::Web)),
    (&["webdl"], Attribute::Source(SourceAttr::WebDl)),
    (&["web", "dl"], Attribute::Source(SourceAttr::WebDl)),
    (&["webrip"], Attribute::Source(SourceAttr::WebRip)),
    (&["web", "rip"], Attribute::Source(SourceAttr::WebRip)),
    (&["bluray"], Attribute::Source(SourceAttr::BluRay)),
    (&["blu", "ray"], Attribute::Source(SourceAttr::BluRay)),
    (&["bd"], Attribute::Source(SourceAttr::BluRay)),
    (&["bdrip"], Attribute::Source(SourceAttr::BluRay)),
    (&["brrip"], Attribute::Source(SourceAttr::BluRay)),
    (&["dvd"], Attribute::Source(SourceAttr::Dvd)),
    (&["dvdrip"], Attribute::Source(SourceAttr::Dvd)),
    (&["dvd", "rip"], Attribute::Source(SourceAttr::Dvd)),
    (&["telesync"], Attribute::Source(SourceAttr::Telesync)),
    // Codec
    (&["x264"], Attribute::Codec(CodecAttr::H264)),
    (&["h264"], Attribute::Codec(CodecAttr::H264)),
    (&["h", "264"], Attribute::Codec(CodecAttr::H264)),
    (&["avc"], Attribute::Codec(CodecAttr::H264)),
    (&["x265"], Attribute::Codec(CodecAttr::H265)),
    (&["h265"], Attribute::Codec(CodecAttr::H265)),
    (&["h", "265"], Attribute::Codec(CodecAttr::H265)),
    (&["hevc"], Attribute::Codec(CodecAttr::H265)),
    (&["av1"], Attribute::Codec(CodecAttr::Av1)),
    // Tags
    (&["dubbed"], Attribute::Tag(Tag::Dubbed)),
    (&["subbed"], Attribute::Tag(Tag::Subbed)),
    (&["dual", "audio"], Attribute::Tag(Tag::DualAudio)),
    (&["dual"], Attribute::Tag(Tag::DualAudio)),
    (&["multi", "audio"], Attribute::Tag(Tag::MultiAudio)),
    (&["multi", "subs"], Attribute::Tag(Tag::MultiSubs)),
    (&["uncensored"], Attribute::Tag(Tag::Uncensored)),
    (&["hdr"], Attribute::Tag(Tag::Hdr)),
    (&["extended"], Attribute::Tag(Tag::Extended)),
    (&["remastered"], Attribute::Tag(Tag::Remastered)),
];

fn phrase_matches(words: &[&str], tokens: &[Token<'_>]) -> bool {
    words.len() <= tokens.len()
        && words.iter().zip(tokens).all(|(word, token)| match token {
            Token::Symbol(Symbol(symbol)) => word.eq_ignore_ascii_case(symbol),
            _ => false,
        })
}

/// Recognize the attribute starting preferring the longest match.
pub fn recognize(tokens: &[Token<'_>]) -> Option<Match> {
    let longest = PHRASES
        .iter()
        .filter(|(words, _)| phrase_matches(words, tokens))
        .max_by_key(|(words, _)| words.len());

    if let Some((words, attribute)) = longest {
        return Some(Match {
            attribute: *attribute,
            consumed: words.len(),
        });
    }

    let Token::Symbol(symbol) = tokens.first()? else {
        return None;
    };
    ResolutionAttr::from_symbol(*symbol).map(|resolution| Match {
        attribute: Attribute::Resolution(resolution),
        consumed: 1,
    })
}

/// Everything recognized in a single file name.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
pub struct Attributes {
    pub resolution: Option<ResolutionAttr>,
    pub source: Option<SourceAttr>,
    pub codec: Option<CodecAttr>,
    pub tags: Vec<Tag>,
}

impl Attributes {
    #[cfg(test)]
    pub fn parse(tokens: &mut crate::parser::tokenizer::Tokenizer<'_>) -> Self {
        let mut attributes = Self::default();

        while !tokens.remaining().is_empty() {
            match recognize(tokens.remaining()) {
                Some(found) => {
                    attributes.insert(found.attribute);
                    tokens.advance_by(found.consumed);
                }
                None => {
                    tokens.advance();
                }
            }
        }
        attributes
    }

    /// Fold `other` in, keeping whatever `self` already holds
    pub fn merge(&mut self, other: Attributes) {
        if let Some(v) = other.resolution {
            self.insert(Attribute::Resolution(v));
        }
        if let Some(v) = other.source {
            self.insert(Attribute::Source(v));
        }
        if let Some(v) = other.codec {
            self.insert(Attribute::Codec(v));
        }
        for tag in other.tags {
            self.insert(Attribute::Tag(tag));
        }
    }

    pub fn insert(&mut self, attribute: Attribute) {
        match attribute {
            Attribute::Resolution(v) => {
                self.resolution.get_or_insert(v);
            }
            Attribute::Source(v) => {
                self.source.get_or_insert(v);
            }
            Attribute::Codec(v) => {
                self.codec.get_or_insert(v);
            }
            Attribute::Tag(v) => {
                if !self.tags.contains(&v) {
                    self.tags.push(v);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::tokenizer::Tokenizer;

    use super::*;

    fn parse(input: &str) -> Attributes {
        Attributes::parse(&mut Tokenizer::new(input))
    }

    fn head(input: &str) -> Option<Match> {
        recognize(Tokenizer::new(input).remaining())
    }

    #[test]
    fn multi_word_phrases_match_across_separators() {
        for (input, expected) in [
            ("WEB-DL", Attribute::Source(SourceAttr::WebDl)),
            ("web.dl", Attribute::Source(SourceAttr::WebDl)),
            ("WEB DL", Attribute::Source(SourceAttr::WebDl)),
            ("Blu-Ray", Attribute::Source(SourceAttr::BluRay)),
            ("Dual Audio", Attribute::Tag(Tag::DualAudio)),
            ("H.264", Attribute::Codec(CodecAttr::H264)),
        ] {
            let found = head(input).unwrap_or_else(|| panic!("{input:?} matched nothing"));
            assert_eq!(expected, found.attribute, "wrong attribute for {input:?}");
        }
    }

    #[test]
    fn longest_phrase_wins() {
        // `web` alone is a source, but `web dl` is a longer entry and must take precedence
        assert_eq!(
            Some(Match {
                attribute: Attribute::Source(SourceAttr::WebDl),
                consumed: 2
            }),
            head("WEB-DL.1080p")
        );
        assert_eq!(
            Some(Match {
                attribute: Attribute::Source(SourceAttr::Web),
                consumed: 1
            }),
            head("web.h265")
        );
    }

    #[test]
    fn phrases_do_not_reach_across_group_edges() {
        let found = head("WEB[DL]").unwrap();
        assert_eq!(Attribute::Source(SourceAttr::Web), found.attribute);
        assert_eq!(1, found.consumed);
    }

    #[test]
    fn phrases_do_not_reach_across_explicit_separators() {
        let found = head("WEB - DL").unwrap();
        assert_eq!(Attribute::Source(SourceAttr::Web), found.attribute);
        assert_eq!(1, found.consumed);
    }

    #[test]
    fn matching_is_case_insensitive() {
        for input in ["WEBRIP", "webrip", "WebRip"] {
            assert_eq!(
                Some(Attribute::Source(SourceAttr::WebRip)),
                head(input).map(|m| m.attribute),
                "{input:?}"
            );
        }
    }

    #[test]
    fn resolutions() {
        for (input, expected) in [
            ("1080p", ResolutionAttr::P1080),
            ("720i", ResolutionAttr::P720),
            ("2160p", ResolutionAttr::P2160),
            ("480P", ResolutionAttr::P480),
            ("1920x1080", ResolutionAttr::P1080),
            ("1280X720", ResolutionAttr::P720),
            ("4k", ResolutionAttr::P2160),
            ("UHD", ResolutionAttr::P2160),
        ] {
            assert_eq!(
                Some(Attribute::Resolution(expected)),
                head(input).map(|m| m.attribute),
                "{input:?}"
            );
        }
    }

    #[test]
    fn episode_markers_are_not_resolutions() {
        for input in ["02x03", "2009x02", "s2009x02", "1x02", "s01e05"] {
            assert_eq!(None, head(input), "{input:?}");
        }
    }

    #[test]
    fn real_release_names() {
        let attributes = parse("Fleabag.S01E01.1080p.AMZN.WEB-DL.DD+5.1.H.264-NTb");
        assert_eq!(Some(ResolutionAttr::P1080), attributes.resolution);
        assert_eq!(Some(SourceAttr::WebDl), attributes.source);
        assert_eq!(Some(CodecAttr::H264), attributes.codec);

        let attributes =
            parse("[Judas] Fullmetal Alchemist Brotherhood - 01 [BD 1080p HEVC x265 10bit FLAC]");
        assert_eq!(Some(ResolutionAttr::P1080), attributes.resolution);
        assert_eq!(Some(SourceAttr::BluRay), attributes.source);
        assert_eq!(Some(CodecAttr::H265), attributes.codec);

        let attributes = parse("Cyberpunk.Edgerunners.S01E02.DUBBED.1080p.WEBRip.x265-RARBG");
        assert_eq!(Some(ResolutionAttr::P1080), attributes.resolution);
        assert_eq!(Some(SourceAttr::WebRip), attributes.source);
        assert_eq!(Some(CodecAttr::H265), attributes.codec);
        assert_eq!(vec![Tag::Dubbed], attributes.tags);

        let attributes =
            parse("[9volt] Sousou no Frieren - 38 (S02E10) (Dual Audio) (WEB 1080p HEVC EAC-3)");
        assert_eq!(Some(ResolutionAttr::P1080), attributes.resolution);
        assert_eq!(Some(SourceAttr::Web), attributes.source);
        assert_eq!(vec![Tag::DualAudio], attributes.tags);

        let attributes = parse(
            "Smoking Behind the Supermarket With You S01E06 Smoke 6 1080p NF WEB-DL DUAL AAC2.0 H.264-VARYG (Super no Ura de Yani Suu Futari, Dual-Audio, Multi-Subs)",
        );
        assert_eq!(Some(ResolutionAttr::P1080), attributes.resolution);
        assert_eq!(Some(SourceAttr::WebDl), attributes.source);
        assert_eq!(vec![Tag::DualAudio, Tag::MultiSubs], attributes.tags);
    }

    #[test]
    fn tags_are_deduplicated() {
        let attributes = parse("Show.DUBBED.1080p.dubbed.720p.x264.x265");
        assert_eq!(vec![Tag::Dubbed], attributes.tags);
        assert_eq!(Some(ResolutionAttr::P1080), attributes.resolution);
        assert_eq!(Some(CodecAttr::H264), attributes.codec);
    }
}
