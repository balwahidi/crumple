//! JSONL report records (field names frozen by ARCHITECTURE §9 T4).

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct Evals {
    pub jpeg: u8,
    pub webp: u8,
    pub avif: u8,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Record {
    pub input: String,
    pub output: Option<String>,
    pub status: &'static str,
    pub reason: Option<String>,
    pub codec: Option<&'static str>,
    pub quality: Option<u8>,
    pub bytes_in: Option<u64>,
    pub bytes_out: Option<u64>,
    pub score: Option<f64>,
    pub target: f64,
    pub evals: Option<Evals>,
    pub ms: Option<u64>,
}

impl Record {
    pub(crate) fn bare(input: String, status: &'static str, reason: &str, target: f64) -> Self {
        Record {
            input,
            output: None,
            status,
            reason: Some(reason.to_string()),
            codec: None,
            quality: None,
            bytes_in: None,
            bytes_out: None,
            score: None,
            target,
            evals: None,
            ms: None,
        }
    }
}

#[derive(Debug, Default, Clone, Serialize)]
pub(crate) struct Summary {
    pub summary: bool,
    pub images: u64,
    pub optimized: u64,
    pub kept: u64,
    pub skipped: u64,
    pub errors: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub wall_ms: u64,
    pub peak_rss_bytes: u64,
}

impl Summary {
    pub(crate) fn add(&mut self, r: &Record) {
        self.images += 1;
        match r.status {
            "optimized" => self.optimized += 1,
            "kept-original" => self.kept += 1,
            "skipped" => self.skipped += 1,
            _ => self.errors += 1,
        }
        if r.status == "optimized" || r.status == "kept-original" {
            self.bytes_in += r.bytes_in.unwrap_or(0);
            self.bytes_out += r.bytes_out.unwrap_or(0);
        }
    }
}
