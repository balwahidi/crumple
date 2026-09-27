//! `crumple` command line: discovery, scheduler, memory gate and report.

mod budget;
mod discover;
mod pipeline_stub;
mod report;
mod rss;

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, LineWriter, Write};
use std::num::{NonZeroU32, NonZeroUsize};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use clap::{Parser, Subcommand};

use budget::MemoryBudget;
use discover::Found;
use pipeline_stub::{optimize_one, Codec, Status};
use report::{Record, Summary};

/// Bytes of working memory charged per pixel by the memory gate (§5).
const BYTES_PER_PIXEL: u64 = 200;

/// Local-first batch image optimizer (pre-alpha).
#[derive(Parser, Debug)]
#[command(name = "crumple", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Optimize image files or folders (recursive) into an output folder.
    Optimize(OptimizeArgs),
}

#[derive(clap::Args, Debug)]
struct OptimizeArgs {
    /// Image files or folders (recursive).
    #[arg(required = true)]
    inputs: Vec<PathBuf>,
    /// Output folder.
    #[arg(long)]
    out: PathBuf,
    /// Score in (0, 100] or medium|high|excellent|lossless-looking.
    #[arg(long, default_value = "80", value_parser = parse_target)]
    target: f64,
    /// Comma list of jpeg,webp,avif,png, or `same`.
    #[arg(long, default_value = "jpeg,webp,avif,png", value_parser = parse_formats)]
    formats: Formats,
    /// Fit within this width (downscale only).
    #[arg(long)]
    max_width: Option<NonZeroU32>,
    /// Fit within this height (downscale only).
    #[arg(long)]
    max_height: Option<NonZeroU32>,
    /// Worker threads (default: available parallelism).
    #[arg(long)]
    jobs: Option<NonZeroUsize>,
    /// Memory budget, e.g. 512MiB, 1GiB or plain bytes.
    #[arg(long, default_value = "1GiB", value_parser = parse_memory)]
    max_memory: u64,
    /// Skip images with more pixels than this, e.g. 24M or a plain number.
    #[arg(long, default_value = "24M", value_parser = parse_pixels)]
    max_pixels: u64,
    /// Write one JSON object per image to this file.
    #[arg(long)]
    report: Option<PathBuf>,
    /// Replace existing output files.
    #[arg(long)]
    overwrite: bool,
    /// Write nothing, but still report.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    Jpeg,
    Webp,
    Avif,
    Png,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Formats {
    List(Vec<Format>),
    Same,
}

/// Settings handed to the per-image pipeline.
#[derive(Debug, Clone)]
#[allow(dead_code)] // read by the real pipeline (T6)
pub(crate) struct Settings {
    pub target: f64,
    pub formats: Formats,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
}

fn parse_target(s: &str) -> Result<f64, String> {
    let v = match s.to_ascii_lowercase().as_str() {
        "medium" => 70.0,
        "high" => 80.0,
        "excellent" => 85.0,
        "lossless-looking" => 90.0,
        other => other
            .parse::<f64>()
            .map_err(|_| format!("invalid target `{s}`"))?,
    };
    if v > 0.0 && v <= 100.0 {
        Ok(v)
    } else {
        Err(format!("target must be in (0, 100], got `{s}`"))
    }
}

fn parse_formats(s: &str) -> Result<Formats, String> {
    if s.eq_ignore_ascii_case("same") {
        return Ok(Formats::Same);
    }
    let mut list = Vec::new();
    for part in s.split(',').map(str::trim) {
        let f = match part.to_ascii_lowercase().as_str() {
            "jpeg" => Format::Jpeg,
            "webp" => Format::Webp,
            "avif" => Format::Avif,
            "png" => Format::Png,
            _ => return Err(format!("unknown format `{part}`")),
        };
        if !list.contains(&f) {
            list.push(f);
        }
    }
    Ok(Formats::List(list))
}

fn split_suffix<'a>(s: &'a str, suffixes: &[(&str, u64)]) -> Result<(&'a str, u64), String> {
    let lower = s.to_ascii_lowercase();
    for (suf, mult) in suffixes {
        if lower.ends_with(suf) {
            return Ok((s[..s.len() - suf.len()].trim(), *mult));
        }
    }
    Ok((s.trim(), 1))
}

fn parse_scaled(s: &str, suffixes: &[(&str, u64)]) -> Result<u64, String> {
    let (num, mult) = split_suffix(s, suffixes)?;
    let n: u64 = num.parse().map_err(|_| format!("invalid size `{s}`"))?;
    n.checked_mul(mult)
        .ok_or_else(|| format!("size `{s}` overflows"))
}

fn parse_memory(s: &str) -> Result<u64, String> {
    let v = parse_scaled(s, &[("kib", 1 << 10), ("mib", 1 << 20), ("gib", 1 << 30)])?;
    if v == 0 {
        return Err("max-memory must be positive".into());
    }
    Ok(v)
}

fn parse_pixels(s: &str) -> Result<u64, String> {
    parse_scaled(s, &[("k", 1_000), ("m", 1_000_000)])
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Optimize(args) => run(args),
    }
}

struct Task {
    found: Found,
    pixels: Option<u64>,
    collides: bool,
}

/// Running totals plus the report file. Every record is written as soon as it
/// completes, so a crash mid-run (a C codec abort, say) keeps the finished ones.
struct Sink {
    summary: Summary,
    report: Option<LineWriter<File>>,
    report_error: Option<std::io::Error>,
}

impl Sink {
    fn emit(&mut self, r: &Record) {
        self.summary.add(r);
        self.write_line(&to_json(r));
    }

    fn write_line(&mut self, line: &str) {
        if self.report_error.is_some() {
            return;
        }
        if let Some(w) = self.report.as_mut() {
            if let Err(e) = writeln!(w, "{line}") {
                self.report_error = Some(e);
            }
        }
    }
}

fn create_report(path: &Path) -> std::io::Result<LineWriter<File>> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    File::create(path).map(LineWriter::new)
}

fn run(args: OptimizeArgs) -> ExitCode {
    let start = Instant::now();
    let settings = Settings {
        target: args.target,
        formats: args.formats.clone(),
        max_width: args.max_width.map(NonZeroU32::get),
        max_height: args.max_height.map(NonZeroU32::get),
    };

    // An output folder inside an input folder is not searched, so a second run
    // does not optimize its own outputs.
    let out_canon = std::fs::canonicalize(&args.out).ok();
    let found = discover::discover(&args.inputs, out_canon.as_deref());

    let report = match &args.report {
        Some(p)
            if found
                .iter()
                .any(|f| f.supported && discover::same_path(p, &f.path)) =>
        {
            eprintln!("error: --report {} is one of the input images", p.display());
            return ExitCode::from(2);
        }
        Some(p) => match create_report(p) {
            Ok(w) => Some(w),
            Err(e) => {
                eprintln!("error: cannot create report {}: {e}", p.display());
                return ExitCode::from(2);
            }
        },
        None => None,
    };
    let sink = Mutex::new(Sink {
        summary: Summary::default(),
        report,
        report_error: None,
    });
    let emit = |r: Record| sink.lock().unwrap_or_else(|e| e.into_inner()).emit(&r);

    // Two inputs with the same case-folded relative path (one name under two input
    // roots, or `a.PNG` beside `a.png` on a case-sensitive disk) would write the same
    // output even in the long collision form. The first in path order wins.
    let mut first_by_rel: HashMap<String, PathBuf> = HashMap::new();
    let mut candidates = Vec::new();
    for f in found {
        if let Some(e) = &f.error {
            emit(Record::bare(disp(&f.path), "error", e, args.target));
        } else if f.symlink {
            emit(Record::bare(
                disp(&f.path),
                "skipped",
                "symlink",
                args.target,
            ));
        } else if !f.supported {
            emit(Record::bare(
                disp(&f.path),
                "skipped",
                "unsupported",
                args.target,
            ));
        } else {
            match first_by_rel.entry(discover::fold(&f.rel)) {
                Entry::Occupied(first) => emit(Record::bare(
                    disp(&f.path),
                    "error",
                    &format!("output name collides with {}", first.get().display()),
                    args.target,
                )),
                Entry::Vacant(v) => {
                    v.insert(f.path.clone());
                    candidates.push(f);
                }
            }
        }
    }
    let groups = discover::collisions(&candidates);
    let mut tasks: Vec<Task> = candidates
        .into_iter()
        .map(|f| Task {
            pixels: imagesize::size(&f.path)
                .ok()
                .map(|d| d.width as u64 * d.height as u64),
            collides: groups.contains(&discover::fold(&f.rel.with_extension(""))),
            found: f,
        })
        .collect();
    // Largest first (LPT); the sort is stable so equal sizes keep path order.
    tasks.sort_by_key(|t| std::cmp::Reverse(t.pixels.unwrap_or(0)));

    let jobs = args.jobs.map_or_else(
        || std::thread::available_parallelism().map_or(1, NonZeroUsize::get),
        NonZeroUsize::get,
    );
    let pool = match rayon::ThreadPoolBuilder::new().num_threads(jobs).build() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: cannot start thread pool: {e}");
            return ExitCode::from(2);
        }
    };
    let budget = MemoryBudget::new(args.max_memory);
    let claimed = Mutex::new(HashSet::new());

    for_each_lpt(&pool, &tasks, |t| {
        let rec = catch_panic(&t.found.path, args.target, || {
            process(t, &args, &settings, &budget, &claimed)
        });
        emit(rec);
    });

    let mut sink = sink.into_inner().unwrap_or_else(|e| e.into_inner());
    sink.summary.summary = true;
    sink.summary.wall_ms = start.elapsed().as_millis() as u64;
    sink.summary.peak_rss_bytes = rss::peak_rss_bytes();
    let line = to_json(&sink.summary);
    sink.write_line(&line);
    if let (Some(w), None) = (sink.report.as_mut(), &sink.report_error) {
        if let Err(e) = w.flush() {
            sink.report_error = Some(e);
        }
    }
    let summary = &sink.summary;
    eprintln!(
        "crumple: {} images, {} optimized, {} kept, {} skipped, {} errors; {} -> {} bytes in {} ms",
        summary.images,
        summary.optimized,
        summary.kept,
        summary.skipped,
        summary.errors,
        summary.bytes_in,
        summary.bytes_out,
        summary.wall_ms
    );
    if let Some(e) = &sink.report_error {
        eprintln!("error: writing report: {e}");
        return ExitCode::from(1);
    }
    if summary.errors > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

/// Runs `f` on every item, pool-width at a time, starting items in slice order
/// (the caller sorts largest first, so this is LPT scheduling).
///
/// Each pool thread pulls the next index from a shared cursor inside `broadcast`.
/// `par_iter` would be wrong twice over:
/// - its splitting starts items out of order (with 4 threads: 0, 100, 50, 150, ...);
/// - a worker that waits inside nested rayon work (oxipng's `parallel` feature in
///   T6) can steal and start another top-level item on the same stack. That item's
///   `MemoryBudget::acquire` then waits for budget held by the frame beneath it,
///   which deadlocks. Broadcast jobs are never stolen, so items never nest.
fn for_each_lpt<T: Sync>(pool: &rayon::ThreadPool, items: &[T], f: impl Fn(&T) + Sync) {
    let next = AtomicUsize::new(0);
    pool.broadcast(|_| {
        while let Some(item) = items.get(next.fetch_add(1, Ordering::Relaxed)) {
            f(item);
        }
    });
}

/// Turns a panic inside one image's pipeline (a decoder or encoder bug) into an
/// error record, so the rest of the batch and the report survive. The memory
/// guard is released while unwinding.
fn catch_panic(input: &Path, target: f64, f: impl FnOnce() -> Record) -> Record {
    std::panic::catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|p| {
        let msg = p
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| p.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("unknown");
        Record::bare(disp(input), "error", &format!("panic: {msg}"), target)
    })
}

fn to_json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).expect("report records always serialize")
}

fn disp(p: &Path) -> String {
    p.display().to_string()
}

fn process(
    t: &Task,
    args: &OptimizeArgs,
    settings: &Settings,
    budget: &MemoryBudget,
    claimed: &Mutex<HashSet<String>>,
) -> Record {
    let input = &t.found.path;
    let target = args.target;
    if t.pixels.is_some_and(|p| p > args.max_pixels) {
        return Record::bare(disp(input), "skipped", "too-large", target);
    }
    let cost = t.pixels.unwrap_or(0).saturating_mul(BYTES_PER_PIXEL);
    let _guard = budget.acquire(cost);
    let started = Instant::now();

    let bytes = match std::fs::read(input) {
        Ok(b) => b,
        Err(e) => return Record::bare(disp(input), "error", &format!("read: {e}"), target),
    };
    let outcome = optimize_one(input, &bytes, settings);
    let mut rec = Record {
        input: disp(input),
        output: None,
        status: match outcome.status {
            Status::Optimized => "optimized",
            Status::KeptOriginal => "kept-original",
            Status::Error => "error",
        },
        reason: outcome.reason.clone(),
        codec: outcome.codec.map(Codec::name),
        quality: outcome.quality,
        bytes_in: Some(bytes.len() as u64),
        bytes_out: None,
        score: outcome.score,
        target,
        evals: outcome.evals,
        ms: None,
    };
    if outcome.status != Status::Error {
        let data: &[u8] = outcome.data.as_deref().unwrap_or(&bytes);
        let ext = outcome.codec.and_then(Codec::ext);
        match discover::output_path(&args.out, input, &t.found.rel, ext, t.collides) {
            Err(e) => fail(&mut rec, e),
            Ok(path) => {
                rec.output = Some(disp(&path));
                rec.bytes_out = Some(data.len() as u64);
                if let Err(e) = write_output(&path, data, args, claimed) {
                    fail(&mut rec, e);
                }
            }
        }
    }
    rec.ms = Some(started.elapsed().as_millis() as u64);
    rec
}

fn fail(rec: &mut Record, reason: String) {
    rec.status = "error";
    rec.reason = Some(reason);
    rec.bytes_out = None;
}

fn write_output(
    path: &Path,
    data: &[u8],
    args: &OptimizeArgs,
    claimed: &Mutex<HashSet<String>>,
) -> Result<(), String> {
    let exists = || "output exists (use --overwrite)".to_string();
    // Backstop for names the up-front checks cannot foresee (an input literally named
    // `a.png.webp` beside `a.png` + `a.webp`): two inputs of one run never write the
    // same file, even with --overwrite.
    let key = discover::fold(&std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()));
    if !claimed
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key)
    {
        return Err("another input in this run writes the same output".to_string());
    }
    if args.dry_run {
        return if !args.overwrite && path.exists() {
            Err(exists())
        } else {
            Ok(())
        };
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create dir: {e}"))?;
    }
    let mut opts = OpenOptions::new();
    opts.write(true);
    if args.overwrite {
        opts.create(true).truncate(true);
    } else {
        // Atomic exists-check: no race with other workers or other processes.
        opts.create_new(true);
    }
    let mut file = opts.open(path).map_err(|e| match e.kind() {
        ErrorKind::AlreadyExists => exists(),
        _ => format!("write: {e}"),
    })?;
    let written = file.write_all(data);
    drop(file);
    if let Err(e) = written {
        // Do not leave a truncated file that the next run would refuse to replace.
        let _ = std::fs::remove_file(path);
        return Err(format!("write: {e}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_parsing() {
        assert_eq!(parse_target("80"), Ok(80.0));
        assert_eq!(parse_target("100"), Ok(100.0));
        assert_eq!(parse_target("72.5"), Ok(72.5));
        assert_eq!(parse_target("medium"), Ok(70.0));
        assert_eq!(parse_target("high"), Ok(80.0));
        assert_eq!(parse_target("excellent"), Ok(85.0));
        assert_eq!(parse_target("lossless-looking"), Ok(90.0));
        assert!(parse_target("0").is_err());
        assert!(parse_target("100.1").is_err());
        assert!(parse_target("-5").is_err());
        assert!(parse_target("great").is_err());
    }

    #[test]
    fn memory_parsing() {
        assert_eq!(parse_memory("512MiB"), Ok(512 << 20));
        assert_eq!(parse_memory("1GiB"), Ok(1 << 30));
        assert_eq!(parse_memory("123456"), Ok(123_456));
        assert!(parse_memory("0").is_err());
        assert!(parse_memory("lots").is_err());
        assert!(parse_memory("1TiB").is_err());
    }

    #[test]
    fn pixels_parsing() {
        assert_eq!(parse_pixels("24M"), Ok(24_000_000));
        assert_eq!(parse_pixels("5000000"), Ok(5_000_000));
        assert!(parse_pixels("x").is_err());
    }

    #[test]
    fn formats_parsing() {
        assert_eq!(parse_formats("same"), Ok(Formats::Same));
        assert_eq!(
            parse_formats("avif,jpeg"),
            Ok(Formats::List(vec![Format::Avif, Format::Jpeg]))
        );
        assert!(parse_formats("jxl").is_err());
        assert!(parse_formats("").is_err());
        assert!(parse_formats("jpeg,").is_err());
    }

    #[test]
    fn flag_validation() {
        let parse = |extra: &[&str]| {
            let mut argv = vec!["crumple", "optimize", "in", "--out", "o"];
            argv.extend_from_slice(extra);
            Cli::try_parse_from(argv)
        };
        assert!(parse(&[]).is_ok());
        for bad in [
            &["--jobs", "0"][..],
            &["--max-width", "0"],
            &["--max-height", "0"],
            &["--target", "nan"],
            &["--target", "inf"],
            &["--max-memory", "512MB"],
            &["--formats", "gif"],
        ] {
            let e = parse(bad).expect_err(&format!("{bad:?} must be rejected"));
            assert_eq!(e.exit_code(), 2, "{bad:?}");
        }
        assert!(Cli::try_parse_from(["crumple", "optimize", "in"]).is_err());
        assert!(Cli::try_parse_from(["crumple", "optimize", "--out", "o"]).is_err());
    }

    #[test]
    fn lpt_runs_each_item_once_in_order_without_nesting() {
        use std::cell::Cell;
        use std::time::Duration;
        thread_local!(static DEPTH: Cell<u32> = const { Cell::new(0) });

        // One thread: items start exactly in slice order.
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        let items: Vec<usize> = (0..50).collect();
        let order = Mutex::new(Vec::new());
        for_each_lpt(&pool, &items, |&i| order.lock().unwrap().push(i));
        assert_eq!(order.into_inner().unwrap(), items);

        // Many threads doing unequal nested rayon work (as oxipng's `parallel` feature
        // will inside a task): no thread ever starts an item while another item is
        // still on its stack. With `par_iter` this happened ~100 times per 2000 items.
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(8)
            .build()
            .unwrap();
        let items: Vec<usize> = (0..800).collect();
        let done = Mutex::new(Vec::new());
        let nested = AtomicUsize::new(0);
        for_each_lpt(&pool, &items, |&i| {
            DEPTH.with(|d| {
                if d.get() > 0 {
                    nested.fetch_add(1, Ordering::SeqCst);
                }
                d.set(d.get() + 1);
            });
            let us = |k: usize| Duration::from_micros(20 + (i * k % 400) as u64);
            rayon::join(|| std::thread::sleep(us(37)), || std::thread::sleep(us(91)));
            DEPTH.with(|d| d.set(d.get() - 1));
            done.lock().unwrap().push(i);
        });
        assert_eq!(nested.load(Ordering::SeqCst), 0);
        let mut done = done.into_inner().unwrap();
        done.sort_unstable();
        assert_eq!(done, items);
    }

    #[test]
    fn panic_becomes_error_record() {
        let r = catch_panic(Path::new("x.png"), 80.0, || panic!("decoder bug"));
        assert_eq!(r.status, "error");
        assert_eq!(r.reason.as_deref(), Some("panic: decoder bug"));
        let r = catch_panic(Path::new("x.png"), 80.0, || {
            panic!("{}", String::from("owned"))
        });
        assert_eq!(r.reason.as_deref(), Some("panic: owned"));
    }
}
