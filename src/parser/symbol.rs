use std::sync::LazyLock;

/// Contains tokens we want to skip during parsing that are not in [Attributes](crate::parser::attributes::Attribute)
const NAME_NOISE: &[&str] = &[
    "3d",
    "sbs",
    "tab",
    "hsbs",
    "htab",
    "mvc",
    "hdc",
    "ac3",
    "dts",
    "dc",
    "divx",
    "divx5",
    "dsr",
    "dsrip",
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
    "576p",
    "576i",
    "hrhd",
    "hrhdtv",
    "hddvd",
    "xvid",
    "xvidvd",
    "xxx",
    "www",
    "kp",
    "aac",
];

static CURRENT_YEAR: LazyLock<u16> =
    LazyLock::new(|| time::OffsetDateTime::now_utc().year() as u16);

/// "word" token representation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Symbol<'a>(pub &'a str);

impl Symbol<'_> {
    /// Try to parse a symbol as a release year
    pub fn as_release_year(self) -> Option<u16> {
        let current_year = *CURRENT_YEAR;
        let value = self.0;
        if value.len() == 4 {
            let year = value.parse().ok()?;
            if year > current_year || year < current_year - 200 {
                return None;
            }
            return Some(year);
        }
        None
    }

    pub fn is_noise(self) -> bool {
        NAME_NOISE.iter().any(|n| n.eq_ignore_ascii_case(self.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_year() {
        assert_eq!(
            Symbol(&CURRENT_YEAR.to_string()).as_release_year(),
            Some(*CURRENT_YEAR)
        );

        assert_eq!(Symbol("2024").as_release_year(), Some(2024));
        assert_eq!(Symbol("2077").as_release_year(), None);
        assert_eq!(Symbol("1800").as_release_year(), None);
    }
}
