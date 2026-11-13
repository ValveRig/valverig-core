//! What a preset needs loaded, and how it says so.

use super::hash::Hash;
use super::text;
use crate::error::{Error, Result};
use crate::{MAX_SAMPLE_RATE, MIN_SAMPLE_RATE};
use std::fmt;

/// What kind of thing an asset is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    /// A `.nam` capture of an amplifier.
    Nam,
    /// A cabinet impulse response.
    Ir,
}

impl fmt::Display for AssetKind {
    /// How to name this kind in a sentence, article and all: "a capture",
    /// "an impulse response". [`AssetKind::as_str`] is the word a document
    /// spells instead.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Nam => "a capture",
            Self::Ir => "an impulse response",
        })
    }
}

impl AssetKind {
    /// The word this kind is written as in a document.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Nam => "nam",
            Self::Ir => "ir",
        }
    }

    /// Read the word a document wrote.
    ///
    /// # Errors
    ///
    /// [`Error::BadAssetKind`] if `s` is not a kind this format knows.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "nam" => Ok(Self::Nam),
            "ir" => Ok(Self::Ir),
            other => Err(Error::BadAssetKind(other.to_string())),
        }
    }
}

/// Something the rack needs loaded, named by content.
///
/// A preset carries assets under labels local to it (`plexi`, `cab`), and a
/// module refers to one by its label. The [`hash`](Self::hash) is the
/// identity; everything else is help.
///
/// ```
/// use valrig_core::preset::{asset_hash, Asset, AssetKind};
///
/// let asset = Asset {
///     kind: AssetKind::Nam,
///     hash: asset_hash(b"the bytes of a capture"),
///     file: Some("brit-plexi-50-v2.nam".to_string()),
///     sample_rate: None,
/// };
/// assert_eq!(asset.kind.as_str(), "nam");
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Asset {
    /// What it is.
    pub kind: AssetKind,
    /// Its content address, and its identity.
    pub hash: Hash,
    /// The name of the file it came from: a hint for a person reading the
    /// document and for a resolver searching its own library.
    ///
    /// A plain file name, never a path; see [`Error::BadFile`]. A resolver
    /// decides which directories to look in; the document only helps it
    /// recognise what it finds, and the hash is what confirms it.
    pub file: Option<String>,
    /// For an impulse response, the rate it was recorded at, when known, in
    /// Hz. A capture states its own rate, so this is refused on
    /// [`AssetKind::Nam`].
    pub sample_rate: Option<f64>,
}

impl Asset {
    /// Check this asset's shape, as the label it is carried under.
    ///
    /// # Errors
    ///
    /// [`Error::BadAssetLabel`], [`Error::BadFile`], [`Error::BadSampleRate`]
    /// or [`Error::UnwantedSampleRate`], each naming the label.
    pub(crate) fn validate(&self, label: &str) -> Result<()> {
        if !text::is_identifier(label) {
            return Err(Error::BadAssetLabel(label.to_string()));
        }
        if let Some(file) = &self.file
            && !text::is_file_name(file)
        {
            return Err(Error::BadFile {
                asset: label.to_string(),
                file: file.clone(),
            });
        }
        match (self.kind, self.sample_rate) {
            (AssetKind::Nam, Some(_)) => Err(Error::UnwantedSampleRate(label.to_string())),
            (_, Some(rate))
                if !(rate.is_finite() && (MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&rate)) =>
            {
                Err(Error::BadSampleRate {
                    asset: label.to_string(),
                    value: rate,
                })
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ir(sample_rate: Option<f64>) -> Asset {
        Asset {
            kind: AssetKind::Ir,
            hash: Hash::of(b"an impulse response"),
            file: None,
            sample_rate,
        }
    }

    #[test]
    fn a_kind_is_one_of_two_words() {
        assert_eq!(AssetKind::parse("nam").unwrap(), AssetKind::Nam);
        assert_eq!(AssetKind::parse("ir").unwrap(), AssetKind::Ir);
        assert_eq!(AssetKind::Ir.as_str(), "ir");
        for bad in ["wav", "NAM", "Nam", ""] {
            assert!(
                matches!(AssetKind::parse(bad), Err(Error::BadAssetKind(_))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_label_is_an_identifier() {
        assert!(ir(None).validate("cab").is_ok());
        for bad in ["", "Cab", "a/b", "a.b"] {
            assert!(
                matches!(ir(None).validate(bad), Err(Error::BadAssetLabel(_))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_stated_rate_is_a_rate_an_audio_file_could_have() {
        for ok in [MIN_SAMPLE_RATE, 44_100.0, 48_000.0, MAX_SAMPLE_RATE] {
            assert!(ir(Some(ok)).validate("cab").is_ok(), "{ok}");
        }
        // The band is the crate's, so a rate the DSP would refuse is
        // refused here, where the document is read, rather than later when
        // a rack is prepared.
        for bad in [
            0.0,
            1.0,
            MIN_SAMPLE_RATE - 1.0,
            -48_000.0,
            f64::NAN,
            f64::INFINITY,
            MAX_SAMPLE_RATE + 1.0,
            1e300,
        ] {
            assert!(
                matches!(
                    ir(Some(bad)).validate("cab"),
                    Err(Error::BadSampleRate { .. })
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_capture_carries_its_own_rate() {
        let mut nam = ir(None);
        nam.kind = AssetKind::Nam;
        assert!(nam.validate("plexi").is_ok());
        nam.sample_rate = Some(48_000.0);
        assert!(matches!(nam.validate("plexi"), Err(Error::UnwantedSampleRate(l)) if l == "plexi"));
    }

    #[test]
    fn a_file_hint_is_a_file_name() {
        let mut a = ir(None);
        a.file = Some("Cab 4x12.wav".into());
        assert!(a.validate("cab").is_ok());
        for bad in ["/etc/passwd", "../../a.wav", "sub/a.wav", "", ".."] {
            a.file = Some(bad.into());
            assert!(
                matches!(a.validate("cab"), Err(Error::BadFile { .. })),
                "{bad:?}"
            );
        }
    }
}
