//! Usage: bench_metric <png> [--ours-only]
//! Times ssimulacra2::compute_frame_ssimulacra2 against Reference::score
//! (median of 10) and reports the process peak working set in bytes per pixel.
//!
//! [Reviewer] The two paths are timed interleaved (crate, ours, crate, ours, ...)
//! so that background load hits both equally; the minimum is printed as well.
//! The peak working set covers whatever ran in the process, so with both paths
//! it is the crate's peak. `--ours-only` skips the crate to measure
//! `Reference::new` + `Reference::score` alone.

use crumple_metric::Reference;
use ssimulacra2::{compute_frame_ssimulacra2, ColorPrimaries, Rgb, TransferCharacteristic};
use std::time::{Duration, Instant};

fn load_rgba(path: &str) -> (Vec<u8>, u32, u32) {
    let mut dec = png::Decoder::new(std::io::BufReader::new(
        std::fs::File::open(path).expect("open png"),
    ));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().expect("png header");
    let mut buf = vec![0; reader.output_buffer_size().expect("size")];
    let info = reader.next_frame(&mut buf).expect("png frame");
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Indexed => unreachable!(),
    };
    (rgba, info.width, info.height)
}

fn crate_rgb(rgba: &[u8], w: u32, h: u32) -> Rgb {
    let data = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            [
                f32::from(p[0]) / 255.0,
                f32::from(p[1]) / 255.0,
                f32::from(p[2]) / 255.0,
            ]
        })
        .collect();
    Rgb::new(
        data,
        w as usize,
        h as usize,
        TransferCharacteristic::SRGB,
        ColorPrimaries::BT709,
    )
    .unwrap()
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

/// Peak working set of this process in bytes, without unsafe code.
fn peak_working_set() -> Option<u64> {
    if cfg!(windows) {
        let out = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!("(Get-Process -Id {}).PeakWorkingSet64", std::process::id()),
            ])
            .output()
            .ok()?;
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()
    } else {
        let s = std::fs::read_to_string("/proc/self/status").ok()?;
        let line = s.lines().find(|l| l.starts_with("VmHWM:"))?;
        let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(kb * 1024)
    }
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: bench_metric <png> [--ours-only]");
    let ours_only = std::env::args().nth(2).is_some_and(|a| a == "--ours-only");
    let (rgba, w, h) = load_rgba(&path);
    // Distortion: posterize RGB.
    let dist: Vec<u8> = rgba
        .iter()
        .enumerate()
        .map(|(i, &v)| if i % 4 == 3 { v } else { v & 0xF8 })
        .collect();

    let t = Instant::now();
    let reference = Reference::new(&rgba, w, h).unwrap();
    let new_time = t.elapsed();

    let mut crate_times = Vec::new();
    let mut score_times = Vec::new();
    let mut crate_score = f64::NAN;
    let mut our_score = 0.0;
    for _ in 0..10 {
        if !ours_only {
            let t = Instant::now();
            crate_score =
                compute_frame_ssimulacra2(crate_rgb(&rgba, w, h), crate_rgb(&dist, w, h)).unwrap();
            crate_times.push(t.elapsed());
        }
        let t = Instant::now();
        our_score = reference.score(&dist).unwrap();
        score_times.push(t.elapsed());
    }

    let s_min = *score_times.iter().min().unwrap();
    let s = median(score_times);
    println!("image: {path} ({w}x{h})");
    println!("Reference::new once:                      {new_time:?}");
    println!(
        "Reference::score median of 10:            {s:?} (min {s_min:?}, score {our_score:.6})"
    );
    if !ours_only {
        let c_min = *crate_times.iter().min().unwrap();
        let c = median(crate_times);
        println!(
            "crate compute_frame_ssimulacra2 median of 10: {c:?} (min {c_min:?}, score {crate_score:.6})"
        );
        let ratio = s.as_secs_f64() / c.as_secs_f64();
        println!(
            "ratio score/crate: {ratio:.3} (limit 0.75) -> {} (min/min {:.3})",
            if ratio <= 0.75 { "PASS" } else { "FAIL" },
            s_min.as_secs_f64() / c_min.as_secs_f64()
        );
    }
    match peak_working_set() {
        Some(b) => println!(
            "peak working set{}: {b} B = {:.1} B/px",
            if ours_only { " (ours only)" } else { "" },
            b as f64 / (f64::from(w) * f64::from(h))
        ),
        None => println!("peak working set: unavailable"),
    }
}
