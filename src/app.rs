use crate::adapters::{self, AdapterReport, ParseOutcome};
use crate::ladder::CompressLevel;
use crate::runner::{RunOpts, RunOutput};
use crate::stats::Counter;
use crate::{archive, budget, config::Config, fallback, runner, sniff, stats, toon};
use anyhow::Result;
use serde_json::json;
use std::borrow::Cow;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Per-run options for `run_wrap` (everything except the command itself).
pub struct WrapOpts {
    pub level: CompressLevel,
    pub raw: bool,
    pub tags: Vec<String>,
    pub fast: bool,
    /// JUnit XML file or directory to render as a test report after the run.
    pub junit: Option<PathBuf>,
    /// A pure output filter dropped from a `-c` pipeline (disclosed).
    pub dropped_filter: Option<String>,
}

pub fn run_wrap(argv: &[String], opts: &WrapOpts, cfg: &Config) -> Result<i32> {
    // Adapter path: detect first, because prepare() must extend argv.
    if !opts.raw {
        if let Some(adapter) = adapters::find_adapter(argv) {
            return run_with_adapter(adapter.as_ref(), argv, opts, cfg);
        }
    }
    let started = std::time::SystemTime::now();
    if opts.raw {
        // Escape hatch: byte-identical output streamed live in arrival
        // order, no footer, no stats — but archived.
        let streamed = RunOpts {
            stream: true,
            heartbeat: None,
        };
        let run = match runner::run_with(argv, &streamed) {
            Ok(r) => r,
            Err(e) => return not_found_or_err(e, argv),
        };
        let code = runner::exit_code(&run.captured.status);
        archive_record(argv, "raw", &run, code, &opts.tags, cfg);
        return Ok(code);
    }
    let run = match runner::run_with(argv, &RunOpts::buffered()) {
        Ok(r) => r,
        Err(e) => return not_found_or_err(e, argv),
    };
    let code = runner::exit_code(&run.captured.status);
    if let Some(path) = &opts.junit {
        if let Some(rendered) = harvest_junit(path, &argv[0], started, cfg) {
            let candidate = Some((rendered, "junit"));
            return Ok(emit_generic(
                argv, &run, code, candidate, opts.level, &opts.tags, cfg, None,
            ));
        }
    }
    Ok(transform_emit_record(
        argv, &run, code, opts.level, &opts.tags, cfg,
    ))
}

/// `--junit <path>` / `[command.X] junit`: render the JUnit XML the command
/// wrote as a test report. A directory means every `*.xml` in it (gradle
/// writes one per class), merged. A file older than this run is stale (the
/// command failed before writing it) and is ignored with a warning.
fn harvest_junit(
    path: &Path,
    runner_name: &str,
    started: std::time::SystemTime,
    cfg: &Config,
) -> Option<String> {
    let files: Vec<PathBuf> = if path.is_dir() {
        let mut v: Vec<PathBuf> = std::fs::read_dir(path)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("xml"))
            .collect();
        v.sort();
        v
    } else if path.is_file() {
        vec![path.to_path_buf()]
    } else {
        eprintln!(
            "cartoon: --junit path {} not found; using the compression ladder instead",
            path.display()
        );
        return None;
    };
    // Filesystem timestamps come from a coarser clock than SystemTime::now()
    // (Linux stamps new inodes with the jiffy-granular coarse clock; FAT and
    // HFS+ round to 1-2 s), so a file the child wrote can read as slightly
    // older than `started`. Allow that slack; a genuinely stale file from a
    // previous run is older by far more.
    let cutoff = started
        .checked_sub(std::time::Duration::from_secs(2))
        .unwrap_or(started);
    let fresh: Vec<&PathBuf> = files
        .iter()
        .filter(|f| {
            std::fs::metadata(f)
                .and_then(|m| m.modified())
                .is_ok_and(|t| t >= cutoff)
        })
        .collect();
    if fresh.is_empty() {
        eprintln!(
            "cartoon: no JUnit file under {} was written by this run (stale results ignored); using the compression ladder instead",
            path.display()
        );
        return None;
    }
    // The report carries the wrapped command's name as its runner label.
    let runner: &'static str = Box::leak(runner_name.to_string().into_boxed_str());
    let reports: Vec<_> = fresh
        .iter()
        .filter_map(|f| {
            let xml = std::fs::read_to_string(f).ok()?;
            adapters::pytest::parse_junit_named(&xml, runner)
                .map_err(|e| eprintln!("cartoon: skipping {}: {e}", f.display()))
                .ok()
        })
        .collect();
    let merged = adapters::report::merge(reports)?;
    Some(adapters::report::render(&merged, cfg.trace_lines, None))
}

/// Shared tail of the non-adapter flow: content-sniff or transform under the
/// ladder, then `emit_generic`. Used by wrapped runs and by `ingest`.
fn transform_emit_record(
    argv: &[String],
    run: &RunOutput,
    code: i32,
    level: CompressLevel,
    tags: &[String],
    cfg: &Config,
) -> i32 {
    // Output that arrived without a matching argv0 (a wrapper script running
    // xcodebuild, JUnit XML on stdout) still gets a structured rendering.
    let c = &run.captured;
    let candidate = sniff::sniff(&c.stdout, &c.stderr, code);
    emit_generic(argv, run, code, candidate, level, tags, cfg, None)
}

/// Archive a run under `mode`. archive.rs stores the text view; when the
/// child wrote non-UTF-8, the original bytes replace it so the raw log is
/// byte-exact. Failures are swallowed (and reported by archive.rs) → None.
fn archive_record(
    argv: &[String],
    mode: &str,
    run: &RunOutput,
    code: i32,
    tags: &[String],
    cfg: &Config,
) -> Option<archive::RunRef> {
    let r = archive::record(argv, mode, &run.captured, code, tags, cfg)?;
    restore_raw_bytes(&r, run);
    Some(r)
}

fn restore_raw_bytes(r: &archive::RunRef, run: &RunOutput) {
    if run.has_raw_bytes() {
        let _ = std::fs::write(r.dir.join("stdout.log"), run.stdout_bytes());
        let _ = std::fs::write(r.dir.join("stderr.log"), run.stderr_bytes());
    }
}

/// A lossy rendering is only allowed when the raw log it points at exists
/// (or the user turned archiving off): otherwise the original is the only
/// copy and must be emitted untouched.
fn lossy_allowed(archived: Option<&archive::RunRef>, cfg: &Config) -> bool {
    archived.is_some() || cfg.keep_runs == 0
}

fn footer(v: serde_json::Value) -> String {
    format!("\n{}", toon::encode(&v))
}

fn raw_log_footer(dir: &Path) -> String {
    footer(json!({ "raw_log": dir.display().to_string() }))
}

/// Disclosure for a run cut short by a signal cartoon forwarded.
fn interrupted_footer(run: &RunOutput) -> String {
    run.interrupted
        .map(|s| footer(json!({ "interrupted": runner::signal_name(s) })))
        .unwrap_or_default()
}

/// The generic flow for stdout AND stderr: each stream is laddered at the
/// same level (stdout may instead carry a JSON/sniff/JUnit candidate) and
/// guarded on its own — a rendering must beat that stream's original, footer
/// included, or the stream is emitted byte-identically. The archive is
/// written before anything is emitted; if it fails, nothing lossy is emitted
/// and no `raw_log` pointer is printed. `archived`: the archive attempt the
/// caller already made (adapter parse-failure path), else None.
#[allow(clippy::too_many_arguments)]
fn emit_generic(
    argv: &[String],
    run: &RunOutput,
    code: i32,
    candidate: Option<(String, &'static str)>,
    level: CompressLevel,
    tags: &[String],
    cfg: &Config,
    archived: Option<Option<archive::RunRef>>,
) -> i32 {
    let c = &run.captured;
    let counter = Counter::new(
        &cfg.tokenizer,
        c.stdout.len() + c.stderr.len(),
        cfg.max_tokens.is_some(),
    );
    // Transform before counting: the ladder's peak memory and the
    // tokenizer's tables should not be resident at the same time.
    let out_cand = candidate.or_else(|| transform(&c.stdout, level));
    let err_cand = transform_text(&c.stderr, level);
    let mut in_out = counter.count(&c.stdout);
    let mut in_err = counter.count(&c.stderr);

    // The pointer the footer will carry: the existing run, or a reserved slot.
    let (done, reserved) = match archived {
        Some(done) => (Some(done), None),
        None => (None, archive::reserve(cfg)),
    };
    let log_dir = match (&done, &reserved) {
        (Some(d), _) => d.as_ref().map(|r| r.dir.clone()),
        (None, r) => r.as_ref().map(|r| r.dir.clone()),
    };
    let log_footer = log_dir.as_deref().map(raw_log_footer).unwrap_or_default();
    let int_footer = interrupted_footer(run);

    let mut out_part = Part::Raw;
    let mut err_part = Part::Raw;
    let mut mode = "passthrough";
    if let Some((cand, tmode)) = out_cand {
        let text = format!("{cand}{log_footer}{int_footer}");
        let n = counter.count(&text);
        let (ok, n, o) = guard(&counter, &text, n, &c.stdout, in_out);
        in_out = o;
        if ok {
            out_part = Part::Text(Cow::Owned(text), n);
            mode = tmode;
        }
    }
    if let Some(cand) = err_cand {
        // With stdout untouched, stderr carries the footer of a lossy run.
        let text = match out_part {
            Part::Raw => format!("{cand}{log_footer}{int_footer}"),
            Part::Text(..) => cand,
        };
        let n = counter.count(&text);
        let (ok, n, o) = guard(&counter, &text, n, &c.stderr, in_err);
        in_err = o;
        if ok {
            err_part = Part::Text(Cow::Owned(text), n);
            if mode == "passthrough" {
                mode = level.as_str();
            }
        }
    }

    let run_ref = match (done, reserved) {
        (Some(done), _) => done,
        (None, Some(r)) => {
            let written = archive::write_reserved(r, argv, mode, &run.captured, code, tags, cfg);
            if let Some(w) = &written {
                restore_raw_bytes(w, run);
            }
            written
        }
        (None, None) => None,
    };
    if mode != "passthrough" && !lossy_allowed(run_ref.as_ref(), cfg) {
        // The raw log the footer points at does not exist: emit the original.
        out_part = Part::Raw;
        err_part = Part::Raw;
        mode = "passthrough";
    }
    deliver(
        argv,
        run,
        code,
        Emission {
            out: out_part,
            err: err_part,
            in_out,
            in_err,
            mode,
            normalize_newline: mode != "passthrough",
        },
        cfg,
        run_ref.as_ref().map(|r| r.id.as_str()),
    )
}

/// Net-savings guard with a cheap first look: when the `len/4` estimate
/// already decides clearly, keep it; when it is close (within 20%), count
/// both sides exactly. Returns (pays, candidate tokens, original tokens).
fn guard(
    counter: &Counter,
    cand: &str,
    cand_n: usize,
    orig: &str,
    orig_n: usize,
) -> (bool, usize, usize) {
    if counter.is_cheap() && is_close(cand_n, orig_n) {
        let (c, o) = (counter.exact(cand), counter.exact(orig));
        return (pays_for_itself(c, o), c, o);
    }
    (pays_for_itself(cand_n, orig_n), cand_n, orig_n)
}

/// Within 20% either way: an estimate cannot be trusted to decide.
fn is_close(cand_n: usize, orig_n: usize) -> bool {
    cand_n * 10 >= orig_n * 8 && cand_n * 8 <= orig_n * 10
}

fn passthrough_emission<'a>(in_out: usize, in_err: usize) -> Emission<'a> {
    Emission {
        out: Part::Raw,
        err: Part::Raw,
        in_out,
        in_err,
        mode: "passthrough",
        normalize_newline: false,
    }
}

/// One stream's emitted form: the child's original bytes, or text plus its
/// token count.
enum Part<'a> {
    Raw,
    Text(Cow<'a, str>, usize),
}

struct Emission<'a> {
    out: Part<'a>,
    err: Part<'a>,
    /// Token counts of the captured streams.
    in_out: usize,
    in_err: usize,
    mode: &'static str,
    /// End a transformed stdout with a newline (never a passthrough one).
    normalize_newline: bool,
}

/// The last step of every non-raw run: apply the `--max-tokens` ceiling to
/// the TOTAL of what will be emitted (stdout and stderr), record stats for
/// exactly that, then write. Stats are recorded before writing so a closed
/// pipe (`cartoon … | head -1`) still lands in the ledger, and a write
/// error never panics or replaces the child's exit code.
fn deliver(
    argv: &[String],
    run: &RunOutput,
    code: i32,
    em: Emission,
    cfg: &Config,
    run_id: Option<&str>,
) -> i32 {
    let c = &run.captured;
    let tokens = |p: &Part, input: usize| match p {
        Part::Raw => input,
        Part::Text(_, n) => *n,
    };
    let mut out_tokens = tokens(&em.out, em.in_out) + tokens(&em.err, em.in_err);
    let mut out = em.out;
    let mut err = em.err;
    if let Some(max) = cfg.max_tokens.filter(|&m| out_tokens > m) {
        let text = |p: &'_ Part<'_>, orig: &str, normalize: bool| -> String {
            match p {
                Part::Raw => orig.to_string(),
                Part::Text(t, _) if normalize && !t.is_empty() && !t.ends_with('\n') => {
                    format!("{t}\n")
                }
                Part::Text(t, _) => t.to_string(),
            }
        };
        let (o, e) = budget::cap_streams(
            &text(&out, &c.stdout, em.normalize_newline),
            &text(&err, &c.stderr, false),
            max,
            &cfg.tokenizer,
            run_id,
        );
        let (no, ne) = (
            stats::estimate_tokens(&o, &cfg.tokenizer),
            stats::estimate_tokens(&e, &cfg.tokenizer),
        );
        out_tokens = no + ne;
        out = Part::Text(Cow::Owned(o), no);
        err = Part::Text(Cow::Owned(e), ne);
    }
    stats::record_counts(
        argv,
        em.mode,
        em.in_out + em.in_err,
        out_tokens,
        code,
        run_id,
    );
    let mut so = std::io::stdout().lock();
    let mut se = std::io::stderr().lock();
    // Any write error (BrokenPipe from `| head`, ENOSPC from >/dev/full) ends
    // the output; the child's exit code stands.
    let _ = (|| -> std::io::Result<()> {
        match (&out, &err) {
            (Part::Raw, Part::Raw) => run.replay(&mut so, &mut se)?,
            _ => {
                match &out {
                    Part::Raw => so.write_all(run.stdout_bytes())?,
                    Part::Text(t, _) => {
                        so.write_all(t.as_bytes())?;
                        if em.normalize_newline && !t.is_empty() && !t.ends_with('\n') {
                            so.write_all(b"\n")?;
                        }
                    }
                }
                so.flush()?;
                match &err {
                    Part::Raw => se.write_all(run.stderr_bytes())?,
                    Part::Text(t, _) => se.write_all(t.as_bytes())?,
                }
            }
        }
        so.flush()?;
        se.flush()
    })();
    if let Some(sig) = run.interrupted {
        // Raw output carries no footer, so say it on stderr as well.
        let _ = writeln!(
            se,
            "cartoon: interrupted by {}; output above is what the command printed before it exited",
            runner::signal_name(sig)
        );
    }
    code
}

/// `cartoon ingest (<file> | -)` — run an EXISTING log through the same
/// flow as a wrapped command: JSON detect → ladder → net-savings guard →
/// raw-log archive → stats. Exit code is 0 (nothing executed) unless the
/// source can't be read. Non-UTF-8 input is accepted (lossy for the
/// transforms; passthrough emits the original bytes).
pub fn run_ingest(
    source: &str,
    level: CompressLevel,
    tags: &[String],
    cfg: &Config,
) -> Result<i32> {
    let content = if source == "-" {
        use std::io::Read;
        let mut buf = Vec::new();
        std::io::stdin()
            .read_to_end(&mut buf)
            .map_err(|e| anyhow::anyhow!("cannot read stdin: {e}"))?;
        buf
    } else {
        std::fs::read(source).map_err(|e| anyhow::anyhow!("cannot read {source}: {e}"))?
    };
    let argv = vec!["ingest".to_string(), source.to_string()];
    let run = RunOutput::from_bytes(content);
    Ok(transform_emit_record(&argv, &run, 0, level, tags, cfg))
}

/// True when the report itself explains a failed run: at least one failing
/// test, or at least one error diagnostic.
fn report_shows_failure(report: &AdapterReport) -> bool {
    match report {
        AdapterReport::Tests(r) => r.failed > 0 || !r.failures.is_empty(),
        AdapterReport::Value(v) => v["summary"]["errors"].as_u64().unwrap_or(0) > 0,
    }
}

fn run_with_adapter(
    adapter: &dyn adapters::Adapter,
    argv: &[String],
    opts: &WrapOpts,
    cfg: &Config,
) -> Result<i32> {
    let tags = &opts.tags;
    let fast = opts.fast;
    let prepared = adapter.prepare(argv.to_vec());
    let fast_args = if fast {
        adapter.fast_args()
    } else {
        Vec::new()
    };
    let mut argv_run = prepared.argv.clone();
    argv_run.extend(fast_args.iter().cloned());
    let mut fast_note = (!fast_args.is_empty()).then(|| fast_args.join(" "));
    let mut run = match runner::run_with(&argv_run, &RunOpts::buffered()) {
        Ok(r) => r,
        Err(e) => return not_found_or_err(e, argv),
    };
    let mut code = runner::exit_code(&run.captured.status);
    // Bounded fallback: pytest exits 4 (usage error) when xdist is missing.
    // Nothing executed, so one serial retry is safe. Only on the exact
    // signature naming an arg WE injected in the unrecognized-arguments
    // list — a user's own typo'd args won't match and pass through.
    if fast_note.is_some()
        && code == 4
        && run.interrupted.is_none()
        && fast_args_rejected(&run.captured.stderr, &fast_args)
    {
        eprintln!("cartoon: --fast unavailable (pytest-xdist not installed?); reran serially");
        fast_note = None;
        run = match runner::run_with(&prepared.argv, &RunOpts::buffered()) {
            Ok(r) => r,
            Err(e) => return not_found_or_err(e, argv),
        };
        code = runner::exit_code(&run.captured.status);
    }
    let archived = archive_record(argv, adapter.name(), &run, code, tags, cfg);
    let captured = &run.captured;
    let run_id = archived.as_ref().map(|r| r.id.as_str());
    match adapter.parse(captured, &prepared) {
        Ok(ParseOutcome {
            report,
            passthrough_stdout,
            passthrough_stderr,
        }) => {
            let counter = Counter::new(
                &cfg.tokenizer,
                captured.stdout.len() + captured.stderr.len(),
                cfg.max_tokens.is_some(),
            );
            let in_out = counter.count(&captured.stdout);
            let in_err = counter.count(&captured.stderr);
            if !lossy_allowed(archived.as_ref(), cfg) {
                // No raw log to point at: the report would be the only copy.
                let em = passthrough_emission(in_out, in_err);
                return Ok(deliver(argv, &run, code, em, cfg, None));
            }
            // Central exit-code rule: a failed run whose report shows no
            // failure (pytest.exit, --cov-fail-under, a suite-level error,
            // a linker error behind a warning) keeps the raw streams, so
            // the reason is never lost. The exit code is always shown when
            // non-zero.
            let unexplained = code != 0 && !report_shows_failure(&report);
            let mut out = report.render(cfg.trace_lines, fast_note.as_deref());
            // `cartoon last` / `cartoon diff`: store the report beside the
            // raw log; compare with the previous run of this command.
            let vs_previous = archived
                .as_ref()
                .and_then(|r| crate::last::on_archived(r, argv, &report, &out));
            if code != 0 {
                out.push_str(&footer(json!({ "exit_code": code })));
            }
            if let Some(v) = vs_previous {
                out.push_str(&footer(json!({ "vs_previous": v })));
            }
            if let Some(r) = &archived {
                out.push_str(&raw_log_footer(&r.dir));
            }
            if let Some(f) = &opts.dropped_filter {
                // `cartoon -c 'pytest | tail -5'`: the report replaces the
                // filter's job; say so rather than silently ignoring it.
                out.push_str(&footer(json!({ "pipe_filter_dropped": f })));
            }
            out.push_str(&interrupted_footer(&run));
            if !out.ends_with('\n') {
                out.push('\n');
            }
            fn pick(given: Option<String>, orig: &str, unexplained: bool) -> Option<Cow<'_, str>> {
                match given {
                    Some(g) => Some(Cow::Owned(g)),
                    None if unexplained && !orig.is_empty() => Some(Cow::Borrowed(orig)),
                    None => None,
                }
            }
            let extra_out = pick(passthrough_stdout, &captured.stdout, unexplained);
            let extra_err = pick(passthrough_stderr, &captured.stderr, unexplained);
            if let Some(x) = &extra_out {
                out.push_str(x);
            }
            drop(extra_out);
            // stderr that IS the captured stderr goes out as the original bytes.
            let mut err_part = match extra_err {
                Some(e) if *e == captured.stderr => Part::Raw,
                Some(e) => {
                    let n = counter.count(&e);
                    Part::Text(e, n)
                }
                None => Part::Text(Cow::Borrowed(""), 0),
            };
            let (mut in_out, mut in_err) = (in_out, in_err);
            let mut out_n = counter.count(&out);
            let mut err_n = match &err_part {
                Part::Raw => in_err,
                Part::Text(_, n) => *n,
            };
            if counter.is_cheap() && is_close(out_n + err_n, in_out + in_err) {
                in_out = counter.exact(&captured.stdout);
                in_err = counter.exact(&captured.stderr);
                out_n = counter.exact(&out);
                err_n = match &mut err_part {
                    Part::Raw => in_err,
                    Part::Text(t, n) => {
                        *n = counter.exact(t);
                        *n
                    }
                };
            }
            // The injected machine format (e.g. `go test -json`) can dwarf
            // what the tool prints on its own; measure against the native
            // view when the adapter can reconstruct it, and show that view
            // instead of the machine stream when the report doesn't win.
            if let Some(native) = adapter.native_stdout(argv, captured) {
                let native_n = counter.exact(&native);
                if !pays_for_itself(out_n + err_n, native_n + in_err) {
                    let em = Emission {
                        out: Part::Text(Cow::Owned(native), native_n),
                        err: Part::Raw,
                        in_out: native_n,
                        in_err,
                        mode: "passthrough",
                        normalize_newline: false,
                    };
                    return Ok(deliver(argv, &run, code, em, cfg, run_id));
                }
                in_out = native_n;
            }
            // Net-savings guard, same rule as the ladder path: a report that
            // costs more tokens than the raw output (tiny suites, `-q` runs)
            // is replaced by the original streams, byte-identical.
            if !pays_for_itself(out_n + err_n, in_out + in_err) {
                let em = passthrough_emission(in_out, in_err);
                return Ok(deliver(argv, &run, code, em, cfg, run_id));
            }
            Ok(deliver(
                argv,
                &run,
                code,
                Emission {
                    out: Part::Text(Cow::Owned(out), out_n),
                    err: err_part,
                    in_out,
                    in_err,
                    mode: adapter.name(),
                    normalize_newline: false,
                },
                cfg,
                run_id,
            ))
        }
        Err(e) => {
            // Safety rule: never lose information. The captured streams still
            // carry the injected machine format, so fall back to the generic
            // ladder + guard rather than dumping them raw: the guard emits the
            // original byte-identically when nothing pays for itself.
            if run.interrupted.is_none() {
                // (An interrupted run's half-written report is expected.)
                eprintln!(
                    "cartoon: {} adapter failed to parse ({e}); compressing generically",
                    adapter.name()
                );
            }
            if let Some(f) = &opts.dropped_filter {
                // The report would have replaced the filter; the generic
                // output does not, so say it was not applied.
                eprintln!("cartoon: pipe filter `{f}` was not applied (adapter output unparsed)");
            }
            Ok(emit_generic(
                argv,
                &run,
                code,
                None,
                opts.level,
                tags,
                cfg,
                Some(archived),
            ))
        }
    }
}

/// True when the runner's usage error names one of the args WE injected.
/// pytest prints `unrecognized arguments: <tok> [<tok>...]` but may list only
/// the first offending token (e.g. `-n` without `auto`), so match exact
/// whitespace-separated tokens, not the joined string.
fn fast_args_rejected(stderr: &str, fast_args: &[String]) -> bool {
    stderr.lines().any(|line| {
        line.split("unrecognized arguments:")
            .nth(1)
            .map(|rest| {
                rest.split_whitespace()
                    .any(|tok| fast_args.iter().any(|a| a == tok))
            })
            .unwrap_or(false)
    })
}

fn not_found_or_err(e: anyhow::Error, argv: &[String]) -> Result<i32> {
    let Some(io) = e.downcast_ref::<std::io::Error>() else {
        return Err(e);
    };
    if io.kind() == std::io::ErrorKind::NotFound {
        eprintln!("cartoon: command not found: {}", argv[0]);
        return Ok(127);
    }
    if cannot_execute(io) {
        // Shell convention: found but not executable → 126, with the cause.
        eprintln!("cartoon: cannot execute {}: {e:#}", argv[0]);
        return Ok(126);
    }
    Err(e)
}

/// EACCES (no exec bit), EISDIR, ENOEXEC (not a binary, no shebang).
fn cannot_execute(io: &std::io::Error) -> bool {
    use std::io::ErrorKind;
    if matches!(
        io.kind(),
        ErrorKind::PermissionDenied | ErrorKind::IsADirectory
    ) {
        return true;
    }
    #[cfg(unix)]
    if io.raw_os_error() == Some(libc::ENOEXEC) {
        return true;
    }
    false
}

/// The net-savings guard: a candidate rendering must beat the original's
/// token count, or the original is emitted byte-identically.
pub fn pays_for_itself(candidate_tokens: usize, original_tokens: usize) -> bool {
    candidate_tokens < original_tokens
}

/// stdout's candidate rendering: TOON when the whole output is one JSON
/// document (or NDJSON), else the ladder. None when nothing changed
/// (passthrough — no copy of the output is made).
pub fn transform(stdout: &str, level: CompressLevel) -> Option<(String, &'static str)> {
    if let Some(json) = fallback::detect_document(stdout) {
        return Some((toon::encode(&json), "json"));
    }
    transform_text(stdout, level).map(|c| (c, level.as_str()))
}

/// The ladder alone (stderr, and stdout that is not JSON). None when it
/// changed nothing.
fn transform_text(text: &str, level: CompressLevel) -> Option<String> {
    if text.is_empty() {
        return None;
    }
    if let Some(view) = window_large(text) {
        return Some(crate::ladder::compress(&view, level));
    }
    let compressed = crate::ladder::compress(text, level);
    // The ladder's line-join drops a trailing newline; treat that as unchanged.
    let unchanged = compressed == text
        || (text.len() == compressed.len() + 1
            && text.ends_with('\n')
            && text.starts_with(compressed.as_str()));
    (!unchanged).then_some(compressed)
}

/// Outputs above this size reach the ladder as a window (see `window_large`).
const WINDOW_THRESHOLD_BYTES: usize = 4 << 20;
const WINDOW_HEAD_BYTES: usize = 512 << 10;
const WINDOW_TAIL_BYTES: usize = 1 << 20;
const WINDOW_MAX_ERROR_LINES: usize = 200;

/// A huge output (multi-megabyte logs) is useless to an agent whole and
/// costs ~13x its size in ladder memory. Keep the head, the tail, and every
/// error line from the middle (up to a cap), with a marker saying exactly
/// what was omitted; the full text is in the raw-log archive, and if the
/// archive write fails the caller emits the original streams instead.
fn window_large(text: &str) -> Option<String> {
    if text.len() <= WINDOW_THRESHOLD_BYTES {
        return None;
    }
    let head_end = text[..WINDOW_HEAD_BYTES]
        .rfind('\n')
        .map_or(WINDOW_HEAD_BYTES, |i| i + 1);
    let mut tail_start = text.len() - WINDOW_TAIL_BYTES;
    while !text.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    let tail_start = text[tail_start..]
        .find('\n')
        .map_or(tail_start, |i| tail_start + i + 1);
    let middle = &text[head_end..tail_start];
    let mut omitted = 0usize;
    let mut errors: Vec<&str> = Vec::new();
    let mut extra_errors = 0usize;
    for line in middle.lines() {
        omitted += 1;
        if crate::ladder::is_error_line(line) {
            if errors.len() < WINDOW_MAX_ERROR_LINES {
                errors.push(line);
            } else {
                extra_errors += 1;
            }
        }
    }
    let mut view = String::with_capacity(head_end + (text.len() - tail_start) + 4096);
    view.push_str(&text[..head_end]);
    view.push_str(&format!(
        "… cartoon: {omitted} lines ({:.1} MB) of a {:.1} MB output omitted; \
         {} error line(s) from them follow{} — full text in raw_log\n",
        middle.len() as f64 / 1048576.0,
        text.len() as f64 / 1048576.0,
        errors.len(),
        if extra_errors > 0 {
            format!(" (+{extra_errors} more not shown)")
        } else {
            String::new()
        }
    ));
    for e in errors {
        view.push_str(e);
        view.push('\n');
    }
    view.push_str("… cartoon: end of omitted section\n");
    view.push_str(&text[tail_start..]);
    Some(view)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn huge_output_is_windowed_keeping_middle_errors() {
        let mut text = String::new();
        let mut i = 0;
        while text.len() < WINDOW_THRESHOLD_BYTES + (1 << 20) {
            text.push_str(&format!("step {i} compiled ok\n"));
            if i == 150_000 {
                text.push_str("error: linker failed in module zeta\n");
            }
            i += 1;
        }
        let view = window_large(&text).expect("windowed");
        assert!(view.len() < 2 << 20, "{}", view.len());
        assert!(view.starts_with("step 0 compiled ok\n"));
        assert!(view.ends_with(&format!("step {} compiled ok\n", i - 1)));
        assert!(view.contains("error: linker failed in module zeta"));
        assert!(view.contains("lines (") && view.contains("omitted"));
        assert!(window_large("small\n").is_none());
    }

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn rejected_when_only_first_token_listed() {
        let stderr = "ERROR: usage: pytest [options]\npytest: error: unrecognized arguments: -n\n  inifile: /x/pyproject.toml\n";
        assert!(fast_args_rejected(stderr, &args(&["-n", "auto"])));
    }

    #[test]
    fn rejected_when_both_tokens_listed() {
        let stderr = "pytest: error: unrecognized arguments: -n auto\n";
        assert!(fast_args_rejected(stderr, &args(&["-n", "auto"])));
    }

    #[test]
    fn not_rejected_by_dash_n_inside_a_path() {
        let stderr =
            "pytest: error: unrecognized arguments: --bogus\nhint: see tests/test-n-gram.py\n";
        assert!(!fast_args_rejected(stderr, &args(&["-n", "auto"])));
    }

    #[test]
    fn not_rejected_without_marker() {
        assert!(!fast_args_rejected(
            "some other exit-4 error",
            &args(&["-n", "auto"])
        ));
    }

    #[test]
    fn guard_rejects_candidates_that_do_not_shrink() {
        assert!(pays_for_itself(10, 100));
        assert!(!pays_for_itself(100, 10));
        assert!(!pays_for_itself(50, 50), "equal is not a win");
    }

    #[test]
    fn transform_safe_passthrough_when_no_rule_fires() {
        assert!(transform("plain prose line", CompressLevel::Safe).is_none());
        assert!(transform("plain prose line\n", CompressLevel::Safe).is_none());
    }

    #[test]
    fn transform_safe_reports_safe_mode_when_rules_fire() {
        let (out, mode) = transform("\x1b[32mok\x1b[0m\n\n\n\nend", CompressLevel::Safe).unwrap();
        assert_eq!(mode, "safe");
        assert!(out.contains("ok"));
    }

    #[test]
    fn transform_json_still_wins() {
        let (_, mode) = transform("{\"a\": 1}", CompressLevel::Safe).unwrap();
        assert_eq!(mode, "json");
    }

    #[test]
    fn transform_never_drops_text_before_a_json_tail() {
        let mut out: String = (1..=50)
            .map(|i| format!("test_case_{i} ... ok\n"))
            .collect();
        out.push_str("test_case_51 ... FAILED\n{\"coverage\": 81.5}\n");
        let rendered = transform(&out, CompressLevel::Safe)
            .map(|(t, _)| t)
            .unwrap_or(out.clone());
        assert!(rendered.contains("test_case_51 ... FAILED"), "{rendered}");
    }

    #[test]
    fn transform_encodes_ndjson_records() {
        let out: String = (0..20)
            .map(|i| format!("{{\"id\": {i}, \"state\": \"ok\"}}\n"))
            .collect();
        let (t, mode) = transform(&out, CompressLevel::Safe).unwrap();
        assert_eq!(mode, "json");
        assert!(t.contains("[20]"), "{t}");
        assert!(t.contains("19,ok"), "{t}");
    }

    #[test]
    fn report_failure_detection_covers_both_shapes() {
        use crate::adapters::report::TestReport;
        let tests = |failed| {
            AdapterReport::Tests(TestReport {
                runner: "pytest",
                total: 3,
                passed: 3 - failed,
                failed,
                skipped: 0,
                duration_s: 0.1,
                failures: Vec::new(),
            })
        };
        assert!(!report_shows_failure(&tests(0)));
        assert!(report_shows_failure(&tests(1)));
        let diag = |errors: u64| {
            AdapterReport::Value(
                json!({"runner": "x", "summary": {"errors": errors, "warnings": 1}}),
            )
        };
        assert!(!report_shows_failure(&diag(0)));
        assert!(report_shows_failure(&diag(2)));
    }

    #[cfg(unix)]
    #[test]
    fn exec_errors_map_to_126() {
        let e = |code| std::io::Error::from_raw_os_error(code);
        assert!(cannot_execute(&e(libc::EACCES)));
        assert!(cannot_execute(&e(libc::EISDIR)));
        assert!(cannot_execute(&e(libc::ENOEXEC)));
        assert!(!cannot_execute(&e(libc::ENOENT)));
    }
}
