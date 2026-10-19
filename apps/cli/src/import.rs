//! `cyanotype import`: walk, hash, check, upload parts straight to storage, complete.
//!
//! Memory: every part is streamed from disk by byte range in 256 KiB chunks, so nothing is buffered
//! beyond `jobs x part-jobs x 256 KiB` of in-flight request bodies, regardless of file size.
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI64, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

use anyhow::{Context, Result};
use clap::Args;
use futures_util::{StreamExt, stream};
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio::sync::Semaphore;

use crate::{
    api::{Api, ApiError, ByteCb, CheckRequest, CheckResponse, DonePart, LimitInfo},
    cache::{self, HashCache},
    config,
    walk::{self, Candidate, WalkOptions},
};

#[derive(Args, Debug)]
pub struct ImportArgs {
    /// Files and/or directories to import (directories are walked recursively).
    #[arg(required = true)]
    pub paths: Vec<PathBuf>,
    /// Files uploaded in parallel.
    #[arg(short, long, default_value_t = 4, value_parser = clap::value_parser!(u32).range(1..=64))]
    pub jobs: u32,
    /// Parts uploaded in parallel per file (files over 16 MiB are split into 16 MiB parts).
    #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u32).range(1..=16))]
    pub part_jobs: u32,
    /// Only import files matching this glob (repeatable). Matched against the file name and the path.
    #[arg(long, value_name = "GLOB")]
    pub include: Vec<String>,
    /// Skip files and directories matching this glob (repeatable).
    #[arg(long, value_name = "GLOB")]
    pub exclude: Vec<String>,
    /// Follow symbolic links (off by default: links are skipped).
    #[arg(long)]
    pub follow_symlinks: bool,
    /// Import videos (mov, mp4, m4v) as originals. On by default; `--videos=false` or `--no-videos` to skip.
    #[arg(long, require_equals = true, num_args = 0..=1, default_value = "true", default_missing_value = "true")]
    pub videos: bool,
    /// Shorthand for `--videos=false`.
    #[arg(long, conflicts_with = "videos")]
    pub no_videos: bool,
    /// List what would be imported and exit. Needs no login and makes no network calls.
    #[arg(short = 'n', long)]
    pub dry_run: bool,
    /// Ignore cached hashes and re-read every file.
    #[arg(long)]
    pub rehash: bool,
    /// Where to write the list of failed files (default: ./cyanotype-import-failures.txt, only if any fail).
    #[arg(long, value_name = "FILE")]
    pub failures_file: Option<PathBuf>,
    /// Hash cache location (default: $XDG_CACHE_HOME/cyanotype/hashes.jsonl).
    #[arg(long, value_name = "FILE")]
    pub cache_file: Option<PathBuf>,
    /// Print a line per file.
    #[arg(short, long)]
    pub verbose: bool,
}

#[derive(Debug)]
enum Outcome {
    Uploaded,
    Existing,
    Failed(String),
    Cancelled,
}

struct Ctx {
    api: Api,
    cache: HashCache,
    hash_slots: Semaphore,
    upload_slots: Semaphore,
    part_jobs: usize,
    /// Net bytes accounted to the progress bar.
    bytes: AtomicI64,
    bar: ProgressBar,
    verbose: bool,
    cancelled: AtomicBool,
    /// Why the run stopped early, when the account's quota or daily cap refused an upload (the first refusal).
    stopped: std::sync::Mutex<Option<LimitInfo>>,
    /// Content hashes already handled (or in flight) in this run, so identical files are sent once.
    seen: std::sync::Mutex<std::collections::HashSet<String>>,
}

impl Ctx {
    fn add(&self, delta: i64) {
        let v = self.bytes.fetch_add(delta, Ordering::Relaxed) + delta;
        self.bar.set_position(v.max(0) as u64);
    }

    /// Print above the progress bar (or plainly when there is no terminal and the bar is hidden).
    fn println(&self, line: String) {
        if self.bar.is_hidden() {
            eprintln!("{line}");
        } else {
            self.bar.println(line);
        }
    }

    fn say(&self, line: String) {
        if self.verbose {
            self.println(line);
        }
    }
}

/// The file's mtime as an RFC 3339 string, unless it is implausible for a photo (before 1990, or in the future,
/// which is what zeroed or unset camera clocks produce).
pub fn mtime_hint(mtime: Option<SystemTime>, now: SystemTime) -> Option<String> {
    let t = mtime?;
    let secs = t.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs();
    const JAN_1990: u64 = 631_152_000;
    let now_secs = now.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs();
    if secs < JAN_1990 || secs > now_secs + 86_400 {
        return None;
    }
    OffsetDateTime::from_unix_timestamp(secs as i64)
        .ok()?
        .format(&Rfc3339)
        .ok()
}

/// `(offset, length)` of part `number` (1-based) of a `size`-byte file cut into `part_size` pieces.
pub fn part_range(number: i32, size: u64, part_size: u64) -> Option<(u64, u64)> {
    if number < 1 || part_size == 0 {
        return None;
    }
    let offset = (number as u64 - 1).checked_mul(part_size)?;
    if offset >= size {
        return None;
    }
    Some((offset, part_size.min(size - offset)))
}

/// Streaming sha256; returns the hex digest and the number of bytes read.
fn hash_file(path: &Path) -> std::io::Result<(String, u64)> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut n = 0u64;
    loop {
        let r = f.read(&mut buf)?;
        if r == 0 {
            break;
        }
        h.update(&buf[..r]);
        n += r as u64;
    }
    Ok((format!("{:x}", h.finalize()), n))
}

async fn file_hash(ctx: &Ctx, f: &Candidate) -> Result<(String, u64), String> {
    let meta = std::fs::metadata(&f.path).map_err(|e| format!("cannot stat: {e}"))?;
    let size = meta.len();
    if size == 0 {
        return Err("file is empty".into());
    }
    let key = cache::Key::new(&f.path, size, meta.modified().ok());
    if let Some(h) = key.as_ref().and_then(|k| ctx.cache.get(k)) {
        return Ok((h, size));
    }
    let _slot = ctx.hash_slots.acquire().await.map_err(|e| e.to_string())?;
    let path = f.path.clone();
    let (hex, read) = tokio::task::spawn_blocking(move || hash_file(&path))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("cannot read: {e}"))?;
    if read != size {
        return Err("file changed while hashing".into());
    }
    if let Some(k) = key {
        ctx.cache.put(k, hex.clone());
    }
    Ok((hex, size))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| "file".into(), |n| n.to_string_lossy().into_owned())
}

async fn process(ctx: &Arc<Ctx>, f: &Candidate) -> Outcome {
    if ctx.cancelled.load(Ordering::Relaxed) {
        return Outcome::Cancelled;
    }
    let (sha, size) = match file_hash(ctx, f).await {
        Ok(v) => v,
        Err(e) => {
            ctx.add(f.size as i64);
            return Outcome::Failed(e);
        }
    };
    if !ctx.seen.lock().unwrap().insert(sha.clone()) {
        // Same content as another file in this run (a copy): nothing more to send.
        ctx.add(f.size as i64);
        return Outcome::Existing;
    }
    // Bytes this file has contributed to the bar so far, so the total can be settled exactly at the end.
    let counted = Arc::new(AtomicI64::new(0));
    let cb: ByteCb = {
        let counted = counted.clone();
        let ctx = ctx.clone();
        Arc::new(move |d| {
            counted.fetch_add(d, Ordering::Relaxed);
            ctx.add(d);
        })
    };
    let res = upload_file(ctx, f, &sha, size, &cb).await;
    // Settle: whatever the outcome, this file accounts for exactly its size on the bar.
    ctx.add(f.size as i64 - counted.load(Ordering::Relaxed));
    match res {
        Ok(true) => Outcome::Uploaded,
        Ok(false) => Outcome::Existing,
        Err(ApiError::Fatal(m)) if m == CANCELLED => Outcome::Cancelled,
        // The quota or the daily cap is reached: every further upload would be refused, so stop starting new ones (parts
        // already on their way finish) and say why at the end. A single file that is too big is just that file's failure.
        Err(ApiError::Limit(l)) if l.stops_import() => {
            ctx.stopped.lock().unwrap().get_or_insert(l);
            ctx.cancelled.store(true, Ordering::Relaxed);
            Outcome::Cancelled
        }
        Err(e) => Outcome::Failed(e.to_string()),
    }
}

const CANCELLED: &str = "interrupted";

/// Ok(true) when bytes were uploaded, Ok(false) when the server already had the file.
async fn upload_file(ctx: &Ctx, f: &Candidate, sha: &str, size: u64, cb: &ByteCb) -> Result<bool, ApiError> {
    let req = CheckRequest {
        sha256: sha.to_string(),
        size_bytes: size as i64,
        filename: file_name(&f.path),
        content_type: walk::content_type(&f.path).to_string(),
        file_modified_at: mtime_hint(f.mtime, SystemTime::now()),
    };
    let mut chk = ctx.api.check(&req).await?;
    // Held for the whole upload (including re-checks), released when this returns.
    let mut slot = None;
    let mut last_refusal = String::new();
    for _round in 0..5 {
        let (upload_id, part_size, parts, uploaded) = match chk {
            CheckResponse::Exists { .. } => return Ok(false),
            CheckResponse::Upload {
                upload_id,
                part_size,
                parts,
                uploaded,
            } => (upload_id, part_size.max(1) as u64, parts, uploaded),
        };
        if slot.is_none() {
            slot = Some(
                ctx.upload_slots
                    .acquire()
                    .await
                    .map_err(|e| ApiError::Fatal(e.to_string()))?,
            );
        }
        // Parts a previous run already stored count as sent.
        for d in &uploaded {
            if let Some((_, len)) = part_range(d.number, size, part_size) {
                cb(len as i64);
            }
        }
        let mut etags: BTreeMap<i32, String> = uploaded.into_iter().map(|d| (d.number, d.etag)).collect();

        let mut expired = false;
        let mut puts = stream::iter(parts)
            .map(|p| {
                let cb = cb.clone();
                async move {
                    if ctx.cancelled.load(Ordering::Relaxed) {
                        return Err(ApiError::Fatal(CANCELLED.into()));
                    }
                    let (offset, len) = part_range(p.number, size, part_size).ok_or_else(|| {
                        ApiError::Fatal(format!("server asked for part {} of a {size} byte file", p.number))
                    })?;
                    let etag = ctx.api.put_part(&p.url, &f.path, offset, len, &cb).await?;
                    Ok::<_, ApiError>((p.number, etag))
                }
            })
            .buffer_unordered(ctx.part_jobs);
        while let Some(r) = puts.next().await {
            match r {
                Ok((n, etag)) => {
                    etags.insert(n, etag);
                }
                Err(ApiError::Expired(m)) => {
                    expired = true;
                    last_refusal = m;
                }
                Err(e) => return Err(e),
            }
        }
        drop(puts);
        if expired {
            // Presigned URLs ran out (very slow link): ask again, the server hands back fresh ones for what is left.
            chk = ctx.api.check(&req).await?;
            continue;
        }
        let done: Vec<DonePart> = etags
            .into_iter()
            .map(|(number, etag)| DonePart { number, etag })
            .collect();
        match ctx.api.complete(&upload_id, &done).await {
            Ok(_) => return Ok(true),
            // The upload row is gone: finished by an earlier attempt whose reply was lost, or aborted.
            Err(ApiError::NotFound) => chk = ctx.api.check(&req).await?,
            Err(e) => return Err(e),
        }
    }
    Err(ApiError::Fatal(format!(
        "gave up after repeated upload restarts; last refusal from storage: {last_refusal}"
    )))
}

fn human(bytes: u64) -> String {
    const U: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

fn print_scan_summary(scan: &walk::Scan) {
    let total: u64 = scan.files.iter().map(|f| f.size).sum();
    let videos = scan.files.iter().filter(|f| f.kind == walk::Kind::Video).count();
    eprintln!(
        "Found {} files ({}), {} of them videos.",
        scan.files.len(),
        human(total),
        videos
    );
    for (why, n) in &scan.skipped {
        eprintln!("  not imported: {n} x {why}");
    }
    for (p, e) in &scan.errors {
        eprintln!("  unreadable: {}: {e}", p.display());
    }
}

fn dry_run(scan: &walk::Scan, verbose: bool) {
    let mut by_ext: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for f in &scan.files {
        let ext = f
            .path
            .extension()
            .map_or_else(String::new, |e| e.to_string_lossy().to_ascii_lowercase());
        let e = by_ext.entry(ext).or_default();
        e.0 += 1;
        e.1 += f.size;
        if verbose {
            println!("{}\t{}\t{}", walk::content_type(&f.path), f.size, f.path.display());
        }
    }
    println!("Dry run: would check {} files against the server.", scan.files.len());
    for (ext, (n, bytes)) in by_ext {
        println!("  .{ext:<6} {n:>8} files {:>12}", human(bytes));
    }
}

pub async fn run(args: ImportArgs) -> Result<bool> {
    let opts = WalkOptions {
        include: args.include.clone(),
        exclude: args.exclude.clone(),
        videos: args.videos && !args.no_videos,
        follow_symlinks: args.follow_symlinks,
    };
    let scan = {
        let roots = args.paths.clone();
        tokio::task::spawn_blocking(move || walk::scan(&roots, &opts)).await??
    };
    print_scan_summary(&scan);
    let mut failures: Vec<(PathBuf, String)> = scan.errors.clone();
    if args.dry_run {
        dry_run(&scan, args.verbose);
        return Ok(failures.is_empty());
    }
    if scan.files.is_empty() {
        write_failures(&args, &failures)?;
        return Ok(failures.is_empty());
    }

    let creds = config::credentials()?;
    // The server may require a newer CLI than this one; say so before doing any work.
    crate::update::enforce_minimum(&creds.server, &crate::release::Version::parse(crate::release::VERSION)?).await?;
    let api = Api::new(&creds.server, Some(&creds.api_key))?;
    // Fail fast on a bad key or unreachable server instead of once per file.
    let me = api.me().await.map_err(|e| anyhow::anyhow!("{}: {e}", creds.server))?;
    eprintln!("Importing to {} as {}.", creds.server, me.email);

    let cache = match &args.cache_file {
        Some(p) => HashCache::open(p, args.rehash)?,
        None => HashCache::open(&config::cache_path("hashes.jsonl")?, args.rehash).or_else(|e| {
            eprintln!("warning: hash cache unavailable ({e:#}); continuing without it");
            Ok::<_, anyhow::Error>(HashCache::in_memory())
        })?,
    };

    let total_files = scan.files.len();
    let total_bytes: u64 = scan.files.iter().map(|f| f.size).sum();
    let bar = ProgressBar::with_draw_target(Some(total_bytes), ProgressDrawTarget::stderr_with_hz(5));
    bar.set_style(
        ProgressStyle::with_template(
            "{bar:30.cyan/blue} {bytes}/{total_bytes} {binary_bytes_per_sec} ETA {eta} | {msg}",
        )
        .expect("static template")
        .progress_chars("=> "),
    );

    let jobs = args.jobs as usize;
    let ctx = Arc::new(Ctx {
        api,
        cache,
        hash_slots: Semaphore::new(jobs),
        upload_slots: Semaphore::new(jobs),
        part_jobs: args.part_jobs as usize,
        bytes: AtomicI64::new(0),
        bar: bar.clone(),
        verbose: args.verbose,
        cancelled: AtomicBool::new(false),
        stopped: Default::default(),
        seen: Default::default(),
    });

    tokio::spawn({
        // First Ctrl-C: stop starting new work and let in-flight parts finish. Second: exit now.
        let ctx = ctx.clone();
        async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                ctx.cancelled.store(true, Ordering::Relaxed);
                eprintln!("\nInterrupted: finishing in-flight parts (Ctrl-C again to quit now). Re-run to resume.");
                if tokio::signal::ctrl_c().await.is_ok() {
                    std::process::exit(130);
                }
            }
        }
    });

    let started = Instant::now();
    let (mut uploaded, mut existing, mut failed, mut cancelled) = (0u64, 0u64, 0u64, 0u64);
    let mut uploaded_bytes = 0u64;
    let mut done = 0usize;
    let mut results = stream::iter(scan.files.iter())
        .map(|f| {
            let ctx = ctx.clone();
            async move { (f, process(&ctx, f).await) }
        })
        // The hash/check stage runs ahead of the upload stage so uploads never wait on a queue of skips.
        .buffer_unordered(jobs * 4);
    while let Some((f, outcome)) = results.next().await {
        done += 1;
        match outcome {
            Outcome::Uploaded => {
                uploaded += 1;
                uploaded_bytes += f.size;
                ctx.say(format!("uploaded  {}", f.path.display()));
            }
            Outcome::Existing => {
                existing += 1;
                ctx.say(format!("exists    {}", f.path.display()));
            }
            Outcome::Failed(why) => {
                failed += 1;
                ctx.println(format!("FAILED    {}: {why}", f.path.display()));
                failures.push((f.path.clone(), why));
            }
            Outcome::Cancelled => cancelled += 1,
        }
        bar.set_message(format!(
            "{done}/{total_files} files, {uploaded} uploaded, {existing} existing, {failed} failed"
        ));
    }
    drop(results);
    bar.finish_and_clear();

    let secs = started.elapsed().as_secs_f64().max(0.001);
    println!("Done in {}.", fmt_duration(started.elapsed()));
    println!(
        "  uploaded:         {uploaded} files ({}, {}/s)",
        human(uploaded_bytes),
        human((uploaded_bytes as f64 / secs) as u64)
    );
    println!("  already present:  {existing} files");
    println!("  failed:           {}", failed + scan.errors.len() as u64);
    let stopped = ctx.stopped.lock().unwrap().clone();
    if let Some(l) = &stopped {
        println!("  not started:      {cancelled} files");
        println!();
        println!("Stopped: {}", l.message);
        match (l.code.as_str(), &l.retry_at) {
            ("daily_cap_exceeded", Some(at)) => {
                println!("Run the same command again after {at} to carry on where it stopped.")
            }
            ("daily_cap_exceeded", None) => println!("Run the same command again later to carry on where it stopped."),
            _ => println!(
                "Free up space (delete photos and empty the trash) or ask the administrator for more, then run the same command again."
            ),
        }
    } else if cancelled > 0 {
        println!("  not started:      {cancelled} files (interrupted; run the same command again to resume)");
    }
    for (p, why) in failures.iter().take(20) {
        println!("    {}: {why}", p.display());
    }
    if failures.len() > 20 {
        println!("    ... and {} more", failures.len() - 20);
    }
    write_failures(&args, &failures)?;
    Ok(failures.is_empty() && cancelled == 0)
}

fn write_failures(args: &ImportArgs, failures: &[(PathBuf, String)]) -> Result<()> {
    if failures.is_empty() {
        return Ok(());
    }
    let path = args
        .failures_file
        .clone()
        .unwrap_or_else(|| PathBuf::from("cyanotype-import-failures.txt"));
    let mut out = String::new();
    for (p, why) in failures {
        out.push_str(&format!("{}\t{}\n", p.display(), why.replace(['\n', '\t'], " ")));
    }
    std::fs::write(&path, out).with_context(|| format!("writing {}", path.display()))?;
    eprintln!(
        "Failure list written to {} (re-run the import to retry them).",
        path.display()
    );
    Ok(())
}

fn fmt_duration(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}h{:02}m{:02}s", s / 3600, s % 3600 / 60, s % 60)
    } else if s >= 60 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else {
        format!("{:.1}s", d.as_secs_f64())
    }
}

#[cfg(test)]
mod tests {
    use std::time::UNIX_EPOCH;

    use super::*;

    #[test]
    fn part_ranges_cover_the_file_exactly() {
        let ps = 16 * 1024 * 1024;
        let size = 40 * 1024 * 1024;
        assert_eq!(part_range(1, size, ps), Some((0, ps)));
        assert_eq!(part_range(2, size, ps), Some((ps, ps)));
        assert_eq!(part_range(3, size, ps), Some((2 * ps, 8 * 1024 * 1024)));
        assert_eq!(part_range(4, size, ps), None);
        assert_eq!(part_range(0, size, ps), None);
        assert_eq!(part_range(1, 5, ps), Some((0, 5)));
        // Exactly on a boundary: no empty trailing part.
        assert_eq!(part_range(2, 2 * ps, ps), Some((ps, ps)));
        assert_eq!(part_range(3, 2 * ps, ps), None);
    }

    #[test]
    fn mtime_hint_rejects_implausible_times() {
        let now = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let at = |s| Some(UNIX_EPOCH + Duration::from_secs(s));
        assert_eq!(
            mtime_hint(at(1_700_000_000), now).as_deref(),
            Some("2023-11-14T22:13:20Z")
        );
        assert_eq!(mtime_hint(at(0), now), None, "epoch zero");
        assert_eq!(mtime_hint(at(100_000_000), now), None, "before 1990");
        assert_eq!(mtime_hint(at(1_900_000_000), now), None, "future");
        assert_eq!(mtime_hint(None, now), None);
    }

    #[test]
    fn hashes_streaming_and_matches_known_digest() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f.bin");
        // Larger than the 1 MiB read buffer, so more than one read.
        let data = vec![7u8; 3 * 1024 * 1024 + 5];
        std::fs::write(&p, &data).unwrap();
        let (h, n) = hash_file(&p).unwrap();
        assert_eq!(n, data.len() as u64);
        assert_eq!(h, format!("{:x}", Sha256::digest(&data)));
        let e = d.path().join("e");
        std::fs::write(&e, b"abc").unwrap();
        assert_eq!(
            hash_file(&e).unwrap().0,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn human_sizes() {
        assert_eq!(human(0), "0 B");
        assert_eq!(human(1536), "1.5 KiB");
        assert_eq!(human(5 * 1024 * 1024 * 1024), "5.0 GiB");
    }
}

/// The upload pipeline against an in-process fake of the API + storage: retries, expired URLs, resume.
#[cfg(test)]
mod pipeline_tests {
    use std::{
        collections::HashMap,
        net::SocketAddr,
        sync::{Arc, Mutex},
    };

    use axum::{
        Json, Router,
        body::Bytes,
        extract::{Path as AxPath, State},
        http::{HeaderMap, StatusCode, header},
        response::IntoResponse,
        routing::{post, put},
    };
    use serde_json::{Value, json};

    use super::*;

    #[derive(Default)]
    struct Fake {
        base: String,
        /// How many times each part was PUT, and the bytes of the last PUT.
        puts: HashMap<i32, (u32, Vec<u8>)>,
        checks: u32,
        completes: Vec<Value>,
        /// part number -> status to answer with on its first PUT
        fail_first: HashMap<i32, u16>,
        /// parts a "previous run" already stored (resume)
        pre_uploaded: Vec<i32>,
        /// complete answers 404 once (reply lost on a previous attempt)
        complete_404_once: bool,
        /// `check` answers this status and body (and a `Retry-After: 0` header) this many times before it works.
        refuse: Option<(u16, Value, u32)>,
    }

    type Shared = Arc<Mutex<Fake>>;
    const PART: i64 = 4;

    async fn check(State(f): State<Shared>, Json(req): Json<Value>) -> (StatusCode, HeaderMap, Json<Value>) {
        let mut f = f.lock().unwrap();
        f.checks += 1;
        if let Some((code, body, times)) = f.refuse.clone().filter(|r| r.2 > 0) {
            f.refuse = Some((code, body.clone(), times - 1));
            let mut h = HeaderMap::new();
            h.insert(header::RETRY_AFTER, "0".parse().unwrap());
            return (StatusCode::from_u16(code).unwrap(), h, Json(body));
        }
        let (code, body) = check_ok(&mut f, &req);
        (code, HeaderMap::new(), body)
    }

    fn check_ok(f: &mut Fake, req: &Value) -> (StatusCode, Json<Value>) {
        let size = req["size_bytes"].as_i64().unwrap();
        // A real server would not offer upload URLs again for a finished upload.
        if f.complete_404_once && f.checks > 1 && f.completes.len() == 1 {
            return (StatusCode::OK, Json(json!({"status": "exists", "asset_id": "a"})));
        }
        let n_parts = ((size + PART - 1) / PART) as i32;
        let done: Vec<i32> = f
            .pre_uploaded
            .iter()
            .copied()
            .chain(f.puts.iter().filter(|(_, (c, _))| *c > 0).map(|(n, _)| *n))
            .collect();
        let base = f.base.clone();
        let parts: Vec<Value> = (1..=n_parts)
            .filter(|n| !done.contains(n))
            .map(|n| json!({"number": n, "url": format!("{base}/put/{n}")}))
            .collect();
        let uploaded: Vec<Value> = done
            .iter()
            .map(|n| json!({"number": n, "etag": format!("\"etag{n}\"")}))
            .collect();
        (
            StatusCode::OK,
            Json(
                json!({"status": "upload", "upload_id": "u1", "part_size": PART, "parts": parts, "uploaded": uploaded}),
            ),
        )
    }

    async fn put_part(
        State(f): State<Shared>,
        AxPath(n): AxPath<i32>,
        headers: HeaderMap,
        body: Bytes,
    ) -> impl IntoResponse {
        let mut f = f.lock().unwrap();
        assert_eq!(
            headers[header::CONTENT_LENGTH].to_str().unwrap(),
            body.len().to_string(),
            "streamed body must carry an exact Content-Length"
        );
        let e = f.puts.entry(n).or_insert((0, vec![]));
        let first = e.0 == 0;
        e.1 = body.to_vec();
        if first && let Some(code) = f.fail_first.remove(&n) {
            // Failed attempts do not count as stored parts.
            f.puts.get_mut(&n).unwrap().0 = 0;
            return (StatusCode::from_u16(code).unwrap(), HeaderMap::new());
        }
        f.puts.get_mut(&n).unwrap().0 += 1;
        let mut h = HeaderMap::new();
        h.insert(header::ETAG, format!("\"etag{n}\"").parse().unwrap());
        (StatusCode::OK, h)
    }

    async fn complete(State(f): State<Shared>, Json(req): Json<Value>) -> impl IntoResponse {
        let mut f = f.lock().unwrap();
        let first = f.completes.is_empty();
        f.completes.push(req);
        if f.complete_404_once && first {
            return (StatusCode::NOT_FOUND, Json(json!({"error": "not found"})));
        }
        (StatusCode::CREATED, Json(json!({"asset_id": "a"})))
    }

    async fn serve(fake: Fake) -> (Shared, String) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        let base = format!("http://{addr}");
        let shared: Shared = Arc::new(Mutex::new(Fake {
            base: base.clone(),
            ..fake
        }));
        let app = Router::new()
            .route("/assets/check", post(check))
            .route("/assets/complete", post(complete))
            .route("/put/{n}", put(put_part))
            .with_state(shared.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (shared, base)
    }

    fn ctx(base: &str) -> Arc<Ctx> {
        Arc::new(Ctx {
            api: Api::new(base, Some("cyk_test")).unwrap(),
            cache: HashCache::in_memory(),
            hash_slots: Semaphore::new(2),
            upload_slots: Semaphore::new(2),
            part_jobs: 2,
            bytes: AtomicI64::new(0),
            bar: ProgressBar::hidden(),
            verbose: false,
            cancelled: AtomicBool::new(false),
            stopped: Default::default(),
            seen: Default::default(),
        })
    }

    fn candidate(dir: &Path, content: &[u8]) -> Candidate {
        let path = dir.join("IMG_1.cr3");
        std::fs::write(&path, content).unwrap();
        let m = std::fs::metadata(&path).unwrap();
        Candidate {
            path,
            size: m.len(),
            mtime: m.modified().ok(),
            kind: walk::Kind::Image,
        }
    }

    fn stored(f: &Shared) -> Vec<u8> {
        let f = f.lock().unwrap();
        let mut keys: Vec<_> = f.puts.keys().copied().collect();
        keys.sort();
        keys.iter().flat_map(|k| f.puts[k].1.clone()).collect()
    }

    #[tokio::test]
    async fn retries_5xx_refreshes_expired_urls_and_completes_with_every_etag() {
        let dir = tempfile::tempdir().unwrap();
        let data = b"0123456789"; // 3 parts of 4,4,2
        let (fake, base) = serve(Fake {
            fail_first: HashMap::from([(2, 503), (3, 403)]),
            ..Default::default()
        })
        .await;
        let ctx = ctx(&base);
        let f = candidate(dir.path(), data);
        let out = process(&ctx, &f).await;
        assert!(matches!(out, Outcome::Uploaded), "{out:?}");
        let fk = fake.lock().unwrap();
        assert_eq!(fk.checks, 2, "one re-check after the expired URL");
        assert_eq!(fk.completes.len(), 1);
        let parts = fk.completes[0]["parts"].as_array().unwrap();
        let nums: Vec<_> = parts.iter().map(|p| p["number"].as_i64().unwrap()).collect();
        assert_eq!(nums, [1, 2, 3]);
        assert_eq!(parts[2]["etag"], "\"etag3\"");
        drop(fk);
        assert_eq!(stored(&fake), data, "parts are exact byte ranges of the file");
        // The bar settles on exactly the file size despite the retried and re-signed parts.
        assert_eq!(ctx.bytes.load(Ordering::Relaxed), data.len() as i64);
    }

    #[tokio::test]
    async fn resumes_with_parts_the_server_already_has() {
        let dir = tempfile::tempdir().unwrap();
        let data = b"abcdefghij";
        let (fake, base) = serve(Fake {
            pre_uploaded: vec![1, 2],
            ..Default::default()
        })
        .await;
        let ctx = ctx(&base);
        let out = process(&ctx, &candidate(dir.path(), data)).await;
        assert!(matches!(out, Outcome::Uploaded), "{out:?}");
        let fk = fake.lock().unwrap();
        assert_eq!(
            fk.puts.keys().copied().collect::<Vec<_>>(),
            [3],
            "only the missing part is sent"
        );
        assert_eq!(fk.puts[&3].1, b"ij");
        let nums: Vec<_> = fk.completes[0]["parts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["number"].as_i64().unwrap())
            .collect();
        assert_eq!(nums, [1, 2, 3], "complete lists the old parts too");
        assert_eq!(ctx.bytes.load(Ordering::Relaxed), 10);
    }

    #[tokio::test]
    async fn lost_complete_reply_is_reconciled_by_checking_again() {
        let dir = tempfile::tempdir().unwrap();
        let (fake, base) = serve(Fake {
            complete_404_once: true,
            ..Default::default()
        })
        .await;
        let ctx = ctx(&base);
        // complete -> 404 (it already went through) -> re-check says "exists" -> counted as uploaded-or-present, not failed.
        let out = process(&ctx, &candidate(dir.path(), b"abcdefgh")).await;
        assert!(matches!(out, Outcome::Existing), "{out:?}");
        assert_eq!(fake.lock().unwrap().completes.len(), 1);
    }

    #[tokio::test]
    async fn identical_files_in_one_run_are_sent_once_and_bad_keys_fail_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let (fake, base) = serve(Fake::default()).await;
        let ctx = ctx(&base);
        let a = candidate(dir.path(), b"samesame");
        let b = Candidate {
            path: dir.path().join("copy.cr3"),
            ..a.clone()
        };
        std::fs::copy(&a.path, &b.path).unwrap();
        assert!(matches!(process(&ctx, &a).await, Outcome::Uploaded));
        assert!(matches!(process(&ctx, &b).await, Outcome::Existing));
        assert_eq!(fake.lock().unwrap().checks, 1);

        // A file that vanished is a per-file failure, not a crash.
        let gone = Candidate {
            path: dir.path().join("nope.cr3"),
            ..a
        };
        assert!(matches!(process(&ctx, &gone).await, Outcome::Failed(_)));
    }

    fn refusal(code: &str, message: &str, retry_at: Option<&str>) -> Value {
        let mut v = json!({"error": message, "code": code});
        if let Some(t) = retry_at {
            v["retry_at"] = json!(t);
        }
        v
    }

    #[tokio::test]
    async fn a_quota_refusal_stops_the_import_without_retrying() {
        let dir = tempfile::tempdir().unwrap();
        let msg = "Not enough storage: this file needs 10 B and your account has 0 B left of 5 B.";
        let (fake, base) = serve(Fake {
            refuse: Some((413, refusal("quota_exceeded", msg, None), 100)),
            ..Default::default()
        })
        .await;
        let ctx = ctx(&base);
        let out = process(&ctx, &candidate(dir.path(), b"0123456789")).await;
        assert!(matches!(out, Outcome::Cancelled), "{out:?}");
        assert_eq!(fake.lock().unwrap().checks, 1, "asked once, not again");
        assert!(ctx.cancelled.load(Ordering::Relaxed), "no new file is started");
        assert_eq!(ctx.stopped.lock().unwrap().as_ref().unwrap().message, msg);
        // The next file is not even sent to the server.
        let other = Candidate {
            path: dir.path().join("two.cr3"),
            ..candidate(dir.path(), b"other bytes")
        };
        std::fs::write(&other.path, b"other bytes").unwrap();
        assert!(matches!(process(&ctx, &other).await, Outcome::Cancelled));
        assert_eq!(fake.lock().unwrap().checks, 1);
    }

    #[tokio::test]
    async fn the_daily_cap_429_is_not_retried_and_keeps_the_reset_time() {
        let dir = tempfile::tempdir().unwrap();
        let (fake, base) = serve(Fake {
            refuse: Some((
                429,
                refusal(
                    "daily_cap_exceeded",
                    "Daily upload limit reached.",
                    Some("2030-01-02T03:04:05Z"),
                ),
                100,
            )),
            ..Default::default()
        })
        .await;
        let ctx = ctx(&base);
        let out = process(&ctx, &candidate(dir.path(), b"0123456789")).await;
        assert!(matches!(out, Outcome::Cancelled), "{out:?}");
        assert_eq!(
            fake.lock().unwrap().checks,
            1,
            "a daily cap is not congestion: no retry"
        );
        let l = ctx.stopped.lock().unwrap().clone().unwrap();
        assert_eq!(
            (l.code.as_str(), l.retry_at.as_deref()),
            ("daily_cap_exceeded", Some("2030-01-02T03:04:05Z"))
        );
        assert!(l.stops_import());
    }

    #[tokio::test]
    async fn a_file_over_the_size_limit_fails_alone_and_the_import_goes_on() {
        let dir = tempfile::tempdir().unwrap();
        let msg = "This file is 10 B and the largest file your account may upload is 5 B.";
        let (fake, base) = serve(Fake {
            refuse: Some((413, refusal("file_too_large", msg, None), 1)),
            ..Default::default()
        })
        .await;
        let ctx = ctx(&base);
        match process(&ctx, &candidate(dir.path(), b"0123456789")).await {
            Outcome::Failed(why) => assert_eq!(why, msg),
            other => panic!("{other:?}"),
        }
        assert!(!ctx.cancelled.load(Ordering::Relaxed));
        assert!(ctx.stopped.lock().unwrap().is_none());
        // The next file (the server accepts again) uploads.
        let next = Candidate {
            path: dir.path().join("small.cr3"),
            ..candidate(dir.path(), b"tiny")
        };
        std::fs::write(&next.path, b"tiny").unwrap();
        assert!(matches!(process(&ctx, &next).await, Outcome::Uploaded));
        assert_eq!(fake.lock().unwrap().checks, 2);
    }

    #[tokio::test]
    async fn an_ordinary_429_is_still_retried() {
        let dir = tempfile::tempdir().unwrap();
        let (fake, base) = serve(Fake {
            refuse: Some((429, json!({"error": "too many requests"}), 1)),
            ..Default::default()
        })
        .await;
        let ctx = ctx(&base);
        assert!(matches!(
            process(&ctx, &candidate(dir.path(), b"0123456789")).await,
            Outcome::Uploaded
        ));
        assert_eq!(fake.lock().unwrap().checks, 2);
        assert!(ctx.stopped.lock().unwrap().is_none());
    }

    #[test]
    fn limit_bodies_are_recognised_and_other_errors_are_not() {
        let l = LimitInfo::from_body(r#"{"error":"x","code":"quota_exceeded","remaining_bytes":0}"#).unwrap();
        assert!(l.stops_import());
        let l = LimitInfo::from_body(r#"{"error":"x","code":"file_too_large"}"#).unwrap();
        assert!(!l.stops_import());
        assert!(LimitInfo::from_body(r#"{"error":"x"}"#).is_none());
        assert!(LimitInfo::from_body(r#"{"error":"x","code":"something_else"}"#).is_none());
        assert!(LimitInfo::from_body("not json").is_none());
    }
}
