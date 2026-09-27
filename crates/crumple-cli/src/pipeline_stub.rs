//! Stubbed per-image pipeline. T6 replaces this file with the real one.
// Variants and fields below are produced only by the real pipeline (T6).
#![allow(dead_code)]

use std::path::Path;

use crate::report::Evals;
use crate::Settings;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    Optimized,
    KeptOriginal,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Codec {
    Jpeg,
    Webp,
    Avif,
    Png,
    Original,
}

impl Codec {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Codec::Jpeg => "jpeg",
            Codec::Webp => "webp",
            Codec::Avif => "avif",
            Codec::Png => "png",
            Codec::Original => "original",
        }
    }

    /// Output extension, or `None` for a kept original (keeps its own).
    pub(crate) fn ext(self) -> Option<&'static str> {
        match self {
            Codec::Jpeg => Some("jpg"),
            Codec::Webp => Some("webp"),
            Codec::Avif => Some("avif"),
            Codec::Png => Some("png"),
            Codec::Original => None,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ImageOutcome {
    pub status: Status,
    pub reason: Option<String>,
    pub codec: Option<Codec>,
    pub quality: Option<u8>,
    pub score: Option<f64>,
    pub evals: Option<Evals>,
    /// Encoded output bytes; `None` with `Codec::Original` means copy the input.
    pub data: Option<Vec<u8>>,
}

pub(crate) fn optimize_one(_input: &Path, _bytes: &[u8], _settings: &Settings) -> ImageOutcome {
    ImageOutcome {
        status: Status::KeptOriginal,
        reason: None,
        codec: Some(Codec::Original),
        quality: None,
        score: None,
        evals: None,
        data: None,
    }
}
