# Cartoon Compression Ladder Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Tiered compression for generic CLI output — safe deterministic rules by default, lossy rules opt-in — per the approved spec at `docs/superpowers/specs/2026-06-11-cartoon-compression-ladder-design.md`.

**Architecture:** `src/heuristic.rs` becomes a `src/ladder/` module: one pure function per rule, a `CompressLevel` enum selects the subset, rules apply in fixed order. The fallback path in `app.rs::transform` switches from a `heuristic_on: bool` to a `CompressLevel`. A golden corpus under `tests/corpus/` asserts signal retention and token floors in CI.

**Tech Stack:** Rust, clap (existing), regex (existing), tiktoken-rs via `stats::estimate_tokens` (existing). No new dependencies.

**Status:** Plan committed for a future release. Do NOT start implementation until the release is scheduled. This plan covers Phase 1 in full; Phases 2 (Drain) and 3 (model tier) are outlined at the end and each requires its own plan, gated on Phase 1 corpus results.

---

## File structure (Phase 1)

| File | Responsibility |
| --- | --- |
| `src/ladder/mod.rs` | `CompressLevel` enum, rule ordering, `compress(text, level)` orchestrator |
| `src/ladder/safe.rs` | Safe rules: `strip_ansi`, `collapse_blanks`, `collapse_repeats` (moved from `heuristic.rs`) |
| `src/ladder/progress.rs` | Safe rule: `collapse_progress` |
| `src/ladder/levels.rs` | Aggressive rule: `filter_levels` |
| `src/ladder/near_dups.rs` | Aggressive rule: `collapse_near_dups` |
| `src/ladder/diagnostics.rs` | Aggressive rule: `extract_diagnostics` |
| `src/ladder/window.rs` | Aggressive rule: `window_errors` |
| `src/cli.rs` | `--compress` flag, `--heuristic` alias |
| `src/config.rs` | `[compress] level`, `[command.<name>] level`, legacy `heuristic` key |
| `src/main.rs` | Level resolution precedence |
| `src/app.rs` | `transform` takes `CompressLevel`; mode strings `safe`/`aggressive` |
| `src/heuristic.rs` | Deleted at the end (contents moved to `src/ladder/safe.rs`) |
| `tests/corpus.rs` | Golden corpus harness |
| `tests/corpus/<fixture>/` | `log.txt` + `manifest.toml` per fixture |

Rule order (fixed, documented in `mod.rs`):
`strip_ansi → collapse_progress → collapse_blanks → collapse_repeats` (safe)
`→ filter_levels → collapse_near_dups → extract_diagnostics → window_errors` (aggressive)
Phase 2 inserts `drain` after `collapse_near_dups`. Phase 3 appends `model_score` last.

---

### Task 1: `CompressLevel` enum and ladder module skeleton

**Files:**
- Create: `src/ladder/mod.rs`
- Modify: `src/lib.rs` (add `pub mod ladder;`)

- [ ] **Step 1: Write the failing tests**

Create `src/ladder/mod.rs`:

```rust
//! Tiered compression ladder for generic (no-adapter) CLI output.
//! Rules are pure functions applied in a fixed order; `CompressLevel`
//! selects the subset. See docs/superpowers/specs/2026-06-11-*.md.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressLevel {
    Safe,
    Aggressive,
}

impl CompressLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            CompressLevel::Safe => "safe",
            CompressLevel::Aggressive => "aggressive",
        }
    }
}

impl std::str::FromStr for CompressLevel {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "safe" => Ok(CompressLevel::Safe),
            "aggressive" => Ok(CompressLevel::Aggressive),
            other => Err(format!(
                "invalid compress level '{other}' (expected: safe | aggressive)"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_parses_known_values() {
        assert_eq!("safe".parse::<CompressLevel>().unwrap(), CompressLevel::Safe);
        assert_eq!(
            "aggressive".parse::<CompressLevel>().unwrap(),
            CompressLevel::Aggressive
        );
    }

    #[test]
    fn level_rejects_unknown_value() {
        assert!("turbo".parse::<CompressLevel>().is_err());
    }

    #[test]
    fn level_round_trips_as_str() {
        assert_eq!(CompressLevel::Safe.as_str(), "safe");
        assert_eq!(CompressLevel::Aggressive.as_str(), "aggressive");
    }
}
```

Add to `src/lib.rs`: `pub mod ladder;`

- [ ] **Step 2: Run tests**

Run: `cargo test ladder::`
Expected: 3 PASS (enum + tests land together; the "failing first" step for pure scaffolding is the compile error before `lib.rs` is updated).

- [ ] **Step 3: Commit**

```bash
git add src/ladder/mod.rs src/lib.rs
git commit -m "feat: CompressLevel enum and ladder module skeleton"
```

### Task 2: Move existing heuristic rules into `ladder/safe.rs` as separate functions

The current `heuristic::compress` (src/heuristic.rs:6-42) interleaves ANSI strip, blank collapse, and repeat collapse in one pass. Decompose into three pure functions with identical combined behavior. Note the existing subtlety: blank lines update `prev`, so identical lines separated by a blank are NOT collapsed — running `collapse_blanks` before `collapse_repeats` (with blanks participating in `prev` tracking) preserves this.

**Files:**
- Create: `src/ladder/safe.rs`
- Modify: `src/ladder/mod.rs` (add `mod safe;` and re-export)

- [ ] **Step 1: Write the failing tests**

Create `src/ladder/safe.rs` with tests first (port the three tests from `src/heuristic.rs:44-66` and add per-rule tests):

```rust
use regex::Regex;
use std::sync::OnceLock;

/// Strip ANSI escape sequences. Pure; returns a new String.
pub fn strip_ansi(text: &str) -> String {
    static ANSI: OnceLock<Regex> = OnceLock::new();
    let ansi = ANSI.get_or_init(|| Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]").unwrap());
    ansi.replace_all(text, "").into_owned()
}

/// Collapse runs of blank lines (after trailing-whitespace trim) to one.
pub fn collapse_blanks(text: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut blank_run = 0usize;
    for raw in text.lines() {
        let line = raw.trim_end();
        if line.is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        out.push(line);
    }
    out.join("\n")
}

/// Collapse exact consecutive duplicate lines to `line` + `  (xN)`.
/// Blank lines participate in tracking, so duplicates separated by a
/// blank are intentionally NOT collapsed across the gap.
pub fn collapse_repeats(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut prev: Option<&str> = None;
    let mut repeat = 0usize;
    for line in text.lines() {
        if prev == Some(line) {
            repeat += 1;
            continue;
        }
        if repeat > 0 {
            out.push(format!("  (x{})", repeat + 1));
            repeat = 0;
        }
        out.push(line.to_string());
        prev = Some(line);
    }
    if repeat > 0 {
        out.push(format!("  (x{})", repeat + 1));
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_ansi_codes() {
        assert_eq!(strip_ansi("\x1b[32mPASS\x1b[0m ok"), "PASS ok");
    }

    #[test]
    fn collapses_blank_runs_to_one() {
        assert_eq!(collapse_blanks("a\n\n\n\nb"), "a\n\nb");
    }

    #[test]
    fn dedupes_identical_consecutive_lines() {
        assert_eq!(collapse_repeats("same\nsame\nsame\nend"), "same\n  (x3)\nend");
    }

    #[test]
    fn does_not_dedupe_across_blank_gap() {
        // blanks update prev: "x\n\nx" stays three lines
        let composed = collapse_repeats(&collapse_blanks("x\n\n\nx"));
        assert_eq!(composed, "x\n\nx");
    }

    #[test]
    fn plain_prose_unchanged() {
        let prose = "first line\nsecond line\nthird line";
        assert_eq!(collapse_repeats(&collapse_blanks(&strip_ansi(prose))), prose);
    }
}
```

In `src/ladder/mod.rs` add:

```rust
mod safe;
pub use safe::{collapse_blanks, collapse_repeats, strip_ansi};
```

- [ ] **Step 2: Run tests**

Run: `cargo test ladder::safe`
Expected: 5 PASS. Also run `cargo test heuristic` — old tests must still pass (heuristic.rs untouched until Task 9).

- [ ] **Step 3: Commit**

```bash
git add src/ladder/safe.rs src/ladder/mod.rs
git commit -m "feat: decompose heuristic into ladder safe rules"
```

### Task 3: `collapse_progress` rule (safe tier)

Collapses carriage-return-rewritten frames (keep the text after the last `\r` on each physical line) and runs of consecutive progress-bar lines (percentages / bar characters) down to the final state.

**Files:**
- Create: `src/ladder/progress.rs`
- Modify: `src/ladder/mod.rs` (add `mod progress; pub use progress::collapse_progress;`)

- [ ] **Step 1: Write the failing test**

```rust
use regex::Regex;
use std::sync::OnceLock;

/// A line counts as a progress frame if it contains a percentage or a
/// bar segment (====>, ----, ###) alongside a number.
fn is_progress_line(line: &str) -> bool {
    static PAT: OnceLock<Regex> = OnceLock::new();
    let pat = PAT.get_or_init(|| {
        Regex::new(r"(\d{1,3}\s*%)|(\[[=\-#>\s]{4,}\])|([=#]{4,}>)").unwrap()
    });
    pat.is_match(line)
}

/// Keep only the final state of progress output:
/// 1. Within each physical line, keep the text after the last `\r`.
/// 2. For runs of >=2 consecutive progress lines, keep only the last.
pub fn collapse_progress(text: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut pending_progress: Option<&str> = None;
    for raw in text.lines() {
        let line = raw.rsplit('\r').next().unwrap_or(raw);
        if is_progress_line(line) {
            pending_progress = Some(line);
            continue;
        }
        if let Some(p) = pending_progress.take() {
            out.push(p);
        }
        out.push(line);
    }
    if let Some(p) = pending_progress {
        out.push(p);
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_text_after_last_carriage_return() {
        assert_eq!(collapse_progress("step 1\rstep 2\rdone"), "done");
    }

    #[test]
    fn collapses_percentage_run_to_last() {
        let input = "Downloading 10%\nDownloading 55%\nDownloading 100%\nresolved";
        assert_eq!(collapse_progress(input), "Downloading 100%\nresolved");
    }

    #[test]
    fn collapses_bar_run_to_last() {
        let input = "[====>     ] 4/10\n[========> ] 8/10\n[==========] 10/10\nok";
        assert_eq!(collapse_progress(input), "[==========] 10/10\nok");
    }

    #[test]
    fn plain_prose_unchanged() {
        let prose = "compiling foo\ncompiling bar\nfinished";
        assert_eq!(collapse_progress(prose), prose);
    }

    #[test]
    fn single_progress_line_amid_prose_is_kept() {
        let input = "coverage: 93%\nall files checked";
        assert_eq!(collapse_progress(input), "coverage: 93%\nall files checked");
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test ladder::progress`
Expected: 5 PASS (write tests first in the file, watch them fail to compile, then add the implementation above them).

- [ ] **Step 3: Commit**

```bash
git add src/ladder/progress.rs src/ladder/mod.rs
git commit -m "feat: collapse_progress safe rule"
```

### Task 4: `filter_levels` rule (aggressive tier)

Detects `timestamp LEVEL message` shaped logs. Fires only when the shape dominates (>= 10 leveled lines AND >= 50% of non-blank lines) so prose is never mangled. DEBUG/INFO/TRACE collapse into a count line; WARN/ERROR/FATAL kept verbatim with ±2 lines of context.

**Files:**
- Create: `src/ladder/levels.rs`
- Modify: `src/ladder/mod.rs` (add `mod levels; pub use levels::filter_levels;`)

- [ ] **Step 1: Write the failing test**

```rust
use regex::Regex;
use std::sync::OnceLock;

const MIN_LEVELED_LINES: usize = 10;
const MIN_LEVELED_RATIO: f64 = 0.5;
const CONTEXT_LINES: usize = 2;

fn level_of(line: &str) -> Option<&'static str> {
    static PAT: OnceLock<Regex> = OnceLock::new();
    let pat = PAT.get_or_init(|| {
        Regex::new(
            r"(?x)^\s*
              (?:\[?\d{4}-\d{2}-\d{2}[T\ ][\d:.,+Z\-]+\]?\s+)?   # optional timestamp
              \[?\b(TRACE|DEBUG|INFO|WARN|WARNING|ERROR|FATAL|CRITICAL)\b\]?",
        )
        .unwrap()
    });
    let caps = pat.captures(line)?;
    Some(match caps.get(1).unwrap().as_str() {
        "TRACE" => "TRACE",
        "DEBUG" => "DEBUG",
        "INFO" => "INFO",
        "WARN" | "WARNING" => "WARN",
        "ERROR" => "ERROR",
        "FATAL" | "CRITICAL" => "FATAL",
        _ => unreachable!(),
    })
}

fn is_noise(level: &str) -> bool {
    matches!(level, "TRACE" | "DEBUG" | "INFO")
}

/// Collapse DEBUG/INFO/TRACE lines to counts; keep WARN+ verbatim with
/// +-CONTEXT_LINES of surrounding context. No-op unless leveled lines
/// dominate the input (see MIN_* thresholds).
pub fn filter_levels(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let levels: Vec<Option<&'static str>> = lines.iter().map(|l| level_of(l)).collect();
    let non_blank = lines.iter().filter(|l| !l.trim().is_empty()).count();
    let leveled = levels.iter().flatten().count();
    if leveled < MIN_LEVELED_LINES
        || non_blank == 0
        || (leveled as f64) / (non_blank as f64) < MIN_LEVELED_RATIO
    {
        return text.to_string();
    }
    // Mark keepers: non-leveled lines, WARN+ lines, and context around WARN+.
    let mut keep = vec![false; lines.len()];
    for (i, lv) in levels.iter().enumerate() {
        match lv {
            None => keep[i] = true,
            Some(l) if !is_noise(l) => {
                let lo = i.saturating_sub(CONTEXT_LINES);
                let hi = (i + CONTEXT_LINES).min(lines.len() - 1);
                for k in keep.iter_mut().take(hi + 1).skip(lo) {
                    *k = true;
                }
            }
            Some(_) => {}
        }
    }
    let mut out: Vec<String> = Vec::new();
    let mut dropped: Vec<&'static str> = Vec::new();
    fn flush(dropped: &mut Vec<&'static str>, out: &mut Vec<String>) {
        if dropped.is_empty() {
            return;
        }
        let mut counts: Vec<(&str, usize)> = Vec::new();
        for l in dropped.iter() {
            match counts.iter_mut().find(|(name, _)| name == l) {
                Some((_, n)) => *n += 1,
                None => counts.push((l, 1)),
            }
        }
        let summary = counts
            .iter()
            .map(|(name, n)| format!("{n} {name}"))
            .collect::<Vec<_>>()
            .join(", ");
        out.push(format!("  (filtered {summary} lines, see raw_log)"));
        dropped.clear();
    }
    for (i, line) in lines.iter().enumerate() {
        if keep[i] {
            flush(&mut dropped, &mut out);
            out.push(line.to_string());
        } else {
            dropped.push(levels[i].unwrap());
        }
    }
    flush(&mut dropped, &mut out);
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leveled_log() -> String {
        let mut s = String::new();
        for i in 0..20 {
            s.push_str(&format!("2026-06-11T10:00:{i:02} INFO worker heartbeat {i}\n"));
        }
        s.push_str("2026-06-11T10:00:20 ERROR connection refused to db:5432\n");
        for i in 21..30 {
            s.push_str(&format!("2026-06-11T10:00:{i:02} DEBUG retry queue {i}\n"));
        }
        s
    }

    #[test]
    fn collapses_noise_keeps_error_with_context() {
        let out = filter_levels(&leveled_log());
        assert!(out.contains("ERROR connection refused"));
        assert!(out.contains("INFO worker heartbeat 18")); // -2 context
        assert!(out.contains("DEBUG retry queue 22")); // +2 context
        assert!(out.contains("filtered"));
        assert!(!out.contains("worker heartbeat 5")); // dropped noise
    }

    #[test]
    fn prose_without_levels_unchanged() {
        let prose = "Compiling cartoon v0.1.0\nFinished dev profile\n";
        assert_eq!(filter_levels(prose), prose);
    }

    #[test]
    fn few_leveled_lines_do_not_trigger() {
        let mixed = "INFO start\nplain a\nplain b\nplain c\nplain d\n";
        assert_eq!(filter_levels(mixed), mixed);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test ladder::levels`
Expected: 3 PASS.

- [ ] **Step 3: Commit**

```bash
git add src/ladder/levels.rs src/ladder/mod.rs
git commit -m "feat: filter_levels aggressive rule"
```

### Task 5: `collapse_near_dups` rule (aggressive tier)

Lines identical after normalizing numbers/hex ids collapse (when >= 3 consecutive) to the first line plus a count marker. Buffer the current run; on flush emit all lines verbatim when the run is short, else first line + marker — keep the implementation in this simple buffered shape.

**Files:**
- Create: `src/ladder/near_dups.rs`
- Modify: `src/ladder/mod.rs` (add `mod near_dups; pub use near_dups::collapse_near_dups;`)

- [ ] **Step 1: Write the failing test**

```rust
use regex::Regex;
use std::sync::OnceLock;

const MIN_RUN: usize = 3;

fn normalize(line: &str) -> String {
    static NUM: OnceLock<Regex> = OnceLock::new();
    static HEX: OnceLock<Regex> = OnceLock::new();
    let hex = HEX.get_or_init(|| Regex::new(r"\b[0-9a-fA-F]{8,}\b").unwrap());
    let num = NUM.get_or_init(|| Regex::new(r"\d+").unwrap());
    num.replace_all(&hex.replace_all(line, "#"), "#").into_owned()
}

/// Collapse runs of >= MIN_RUN consecutive lines that are identical after
/// numeric/id normalization into the first line + `  (xN similar)`.
/// Shorter runs are emitted verbatim.
pub fn collapse_near_dups(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut run: Vec<String> = Vec::new();
    let mut run_norm = String::new();

    fn flush(run: &mut Vec<String>, out: &mut Vec<String>) {
        if run.len() >= MIN_RUN {
            out.push(run[0].clone());
            out.push(format!("  (x{} similar)", run.len()));
        } else {
            out.append(run);
        }
        run.clear();
    }

    for raw in text.lines() {
        let norm = normalize(raw);
        if !run.is_empty() && norm == run_norm {
            run.push(raw.to_string());
        } else {
            flush(&mut run, &mut out);
            run_norm = norm;
            run.push(raw.to_string());
        }
    }
    flush(&mut run, &mut out);
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapses_numbered_run() {
        let input = "copied chunk 1 of 500\ncopied chunk 2 of 500\ncopied chunk 3 of 500\ncopied chunk 4 of 500\ndone";
        assert_eq!(
            collapse_near_dups(input),
            "copied chunk 1 of 500\n  (x4 similar)\ndone"
        );
    }

    #[test]
    fn run_of_two_kept_verbatim() {
        let input = "retry 1\nretry 2\nok";
        assert_eq!(collapse_near_dups(input), input);
    }

    #[test]
    fn distinct_lines_unchanged() {
        let input = "alpha\nbeta\ngamma";
        assert_eq!(collapse_near_dups(input), input);
    }

    #[test]
    fn hex_ids_normalize() {
        let input = "pulled layer a1b2c3d4e5f6\npulled layer 998877665544\npulled layer deadbeef0123\nok";
        assert_eq!(
            collapse_near_dups(input),
            "pulled layer a1b2c3d4e5f6\n  (x3 similar)\nok"
        );
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test ladder::near_dups`
Expected: 4 PASS.

- [ ] **Step 3: Commit**

```bash
git add src/ladder/near_dups.rs src/ladder/mod.rs
git commit -m "feat: collapse_near_dups aggressive rule"
```

### Task 6: `extract_diagnostics` rule (aggressive tier)

Compiler-shaped diagnostics (`file:line:col: severity: msg` — gcc/clang/rustc/tsc/eslint) get pulled into a TOON table; matched lines are removed from the body. Fires only at >= 3 diagnostics.

**Files:**
- Create: `src/ladder/diagnostics.rs`
- Modify: `src/ladder/mod.rs` (add `mod diagnostics; pub use diagnostics::extract_diagnostics;`)

- [ ] **Step 1: Write the failing test**

```rust
use regex::Regex;
use serde_json::json;
use std::sync::OnceLock;

const MIN_DIAGNOSTICS: usize = 3;

/// Pull `file:line[:col]: severity: msg` lines into a TOON diagnostics
/// table appended to the remaining text. No-op below MIN_DIAGNOSTICS.
pub fn extract_diagnostics(text: &str) -> String {
    static PAT: OnceLock<Regex> = OnceLock::new();
    let pat = PAT.get_or_init(|| {
        Regex::new(r"^(?P<loc>\S+?:\d+(?::\d+)?):?\s+(?P<sev>error|warning|note)\b[:\[]?\s*(?P<msg>.*)$")
            .unwrap()
    });
    let mut diags: Vec<(String, String, String)> = Vec::new();
    let mut rest: Vec<&str> = Vec::new();
    for line in text.lines() {
        match pat.captures(line) {
            Some(c) => diags.push((
                c["loc"].to_string(),
                c["sev"].to_string(),
                c["msg"].trim().to_string(),
            )),
            None => rest.push(line),
        }
    }
    if diags.len() < MIN_DIAGNOSTICS {
        return text.to_string();
    }
    let rows: Vec<_> = diags
        .iter()
        .map(|(loc, sev, msg)| json!({ "loc": loc, "severity": sev, "msg": msg }))
        .collect();
    let table = crate::toon::encode(&json!({ "diagnostics": rows }));
    let body = rest.join("\n");
    if body.trim().is_empty() {
        table
    } else {
        format!("{body}\n{table}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_gcc_shaped_diagnostics() {
        let input = "make: entering dir\nsrc/a.c:10:5: error: 'x' undeclared\nsrc/a.c:12:1: warning: unused variable 'y'\nsrc/b.c:3:9: error: expected ';'\nmake: *** [all] Error 1";
        let out = extract_diagnostics(input);
        assert!(out.contains("make: entering dir"));
        assert!(out.contains("diagnostics"));
        assert!(out.contains("src/a.c:10:5"));
        assert!(out.contains("make: *** [all] Error 1"));
    }

    #[test]
    fn below_threshold_unchanged() {
        let input = "src/a.c:10:5: error: oops\nall good otherwise";
        assert_eq!(extract_diagnostics(input), input);
    }

    #[test]
    fn prose_with_colons_unchanged() {
        let input = "note: this is prose\nsee: the manual\nhttp://example.com: a link";
        assert_eq!(extract_diagnostics(input), input);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test ladder::diagnostics`
Expected: 3 PASS. Check `src/toon/` for the exact `encode` signature before wiring (`toon::encode(&serde_json::Value)` per `src/app.rs:38`).

- [ ] **Step 3: Commit**

```bash
git add src/ladder/diagnostics.rs src/ladder/mod.rs
git commit -m "feat: extract_diagnostics aggressive rule"
```

### Task 7: `window_errors` rule (aggressive tier)

Last aggressive rule: bound total size. Keep head, tail, and windows around error keywords; elide the rest with explicit markers. No-op for inputs under the size threshold.

**Files:**
- Create: `src/ladder/window.rs`
- Modify: `src/ladder/mod.rs` (add `mod window; pub use window::window_errors;`)

- [ ] **Step 1: Write the failing test**

```rust
use regex::Regex;
use std::sync::OnceLock;

const HEAD_LINES: usize = 15;
const TAIL_LINES: usize = 15;
const ERROR_CONTEXT: usize = 3;
const MIN_TOTAL_LINES: usize = 80; // below this, windowing saves too little

fn is_error_line(line: &str) -> bool {
    static PAT: OnceLock<Regex> = OnceLock::new();
    let pat = PAT.get_or_init(|| {
        Regex::new(r"(?i)\b(error|err!|fail|failed|failure|exception|panic|fatal|traceback)\b")
            .unwrap()
    });
    pat.is_match(line)
}

/// Keep head + tail + windows around error keywords; replace elided spans
/// with `  (skipped K lines, see raw_log)`.
pub fn window_errors(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() < MIN_TOTAL_LINES {
        return text.to_string();
    }
    let mut keep = vec![false; lines.len()];
    for k in keep.iter_mut().take(HEAD_LINES.min(lines.len())) {
        *k = true;
    }
    for k in keep.iter_mut().rev().take(TAIL_LINES.min(lines.len())) {
        *k = true;
    }
    for (i, line) in lines.iter().enumerate() {
        if is_error_line(line) {
            let lo = i.saturating_sub(ERROR_CONTEXT);
            let hi = (i + ERROR_CONTEXT).min(lines.len() - 1);
            for k in keep.iter_mut().take(hi + 1).skip(lo) {
                *k = true;
            }
        }
    }
    let mut out: Vec<String> = Vec::new();
    let mut skipped = 0usize;
    for (i, line) in lines.iter().enumerate() {
        if keep[i] {
            if skipped > 0 {
                out.push(format!("  (skipped {skipped} lines, see raw_log)"));
                skipped = 0;
            }
            out.push(line.to_string());
        } else {
            skipped += 1;
        }
    }
    if skipped > 0 {
        out.push(format!("  (skipped {skipped} lines, see raw_log)"));
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn big_log() -> String {
        let mut s = String::new();
        for i in 0..200 {
            if i == 100 {
                s.push_str("step 100: ERROR widget exploded\n");
            } else {
                s.push_str(&format!("step {i}: ok\n"));
            }
        }
        s
    }

    #[test]
    fn keeps_head_tail_and_error_window() {
        let out = window_errors(&big_log());
        assert!(out.contains("step 0: ok"));
        assert!(out.contains("step 199: ok"));
        assert!(out.contains("ERROR widget exploded"));
        assert!(out.contains("step 98: ok")); // error context
        assert!(out.contains("skipped"));
        assert!(!out.contains("step 50: ok")); // elided middle
    }

    #[test]
    fn small_input_unchanged() {
        let small = "a\nb\nc";
        assert_eq!(window_errors(small), small);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test ladder::window`
Expected: 2 PASS.

- [ ] **Step 3: Commit**

```bash
git add src/ladder/window.rs src/ladder/mod.rs
git commit -m "feat: window_errors aggressive rule"
```

### Task 8: Ladder orchestrator

**Files:**
- Modify: `src/ladder/mod.rs`

- [ ] **Step 1: Write the failing tests, then the orchestrator**

Add to `src/ladder/mod.rs`:

```rust
/// Apply the ladder at the given level. Fixed rule order; each rule is a
/// pure fn(&str) -> String that no-ops when its pattern is absent.
pub fn compress(text: &str, level: CompressLevel) -> String {
    let safe = collapse_repeats(&collapse_blanks(&collapse_progress(&strip_ansi(text))));
    match level {
        CompressLevel::Safe => safe,
        CompressLevel::Aggressive => {
            window_errors(&extract_diagnostics(&collapse_near_dups(&filter_levels(&safe))))
        }
    }
}
```

Tests (same file):

```rust
#[test]
fn safe_level_skips_aggressive_rules() {
    // leveled log: aggressive would filter INFO lines, safe must not
    let mut log = String::new();
    for i in 0..15 {
        log.push_str(&format!("2026-06-11 INFO item {i}\n"));
    }
    let safe = compress(&log, CompressLevel::Safe);
    assert!(safe.contains("INFO item 3"));
    let aggressive = compress(&log, CompressLevel::Aggressive);
    assert!(!aggressive.contains("INFO item 3"));
}

#[test]
fn prose_survives_both_levels_unchanged() {
    let prose = "Compiling cartoon v0.1.0\nFinished release in 2.41s";
    assert_eq!(compress(prose, CompressLevel::Safe), prose);
    assert_eq!(compress(prose, CompressLevel::Aggressive), prose);
}
```

Note: the leveled-log test relies on `collapse_near_dups` NOT pre-collapsing the INFO run — `filter_levels` runs first by design, so leveled noise is counted before near-dup collapsing. If counts in the test prove brittle, vary the INFO message texts.

- [ ] **Step 2: Run tests**

Run: `cargo test ladder::`
Expected: all ladder tests PASS.

- [ ] **Step 3: Commit**

```bash
git add src/ladder/mod.rs
git commit -m "feat: ladder orchestrator with level-selected rule subsets"
```

### Task 9: Wire `transform` to the ladder; retire `heuristic.rs`

**Files:**
- Modify: `src/app.rs:180-188` (`transform`), `src/app.rs:6-32` (`run_wrap` signature)
- Delete: `src/heuristic.rs`
- Modify: `src/lib.rs` (remove `pub mod heuristic;`)
- Modify: `src/main.rs` (temporary: pass `CompressLevel::Safe`; real resolution lands in Task 10)

- [ ] **Step 1: Write the failing tests**

Add to `src/app.rs` tests:

```rust
use crate::ladder::CompressLevel;

#[test]
fn transform_safe_passthrough_when_no_rule_fires() {
    let (out, mode) = transform("plain prose line", CompressLevel::Safe);
    assert_eq!(out, "plain prose line");
    assert_eq!(mode, "passthrough");
}

#[test]
fn transform_safe_reports_safe_mode_when_rules_fire() {
    let (out, mode) = transform("\x1b[32mok\x1b[0m\n\n\n\nend", CompressLevel::Safe);
    assert_eq!(mode, "safe");
    assert!(out.contains("ok"));
}

#[test]
fn transform_json_still_wins() {
    let (_, mode) = transform("{\"a\": 1}", CompressLevel::Safe);
    assert_eq!(mode, "json");
}
```

- [ ] **Step 2: Implement**

Replace `transform` in `src/app.rs`:

```rust
pub fn transform(stdout: &str, level: crate::ladder::CompressLevel) -> (String, &'static str) {
    if let Some(json) = fallback::detect_json(stdout) {
        return (toon::encode(&json), "json");
    }
    let compressed = crate::ladder::compress(stdout, level);
    // The ladder's line-join drops a trailing newline; treat that as unchanged.
    if compressed == stdout || format!("{compressed}\n") == stdout {
        return (stdout.to_string(), "passthrough");
    }
    (compressed, level.as_str())
}
```

Update `run_wrap` signature: `heuristic_on: bool` → `level: crate::ladder::CompressLevel`; pass `level` to `transform` at the existing call site (src/app.rs:32). Remove the `heuristic` import from `src/app.rs:2`, delete `src/heuristic.rs`, drop `pub mod heuristic;` from `src/lib.rs`, and in `src/main.rs` pass `cartoon::ladder::CompressLevel::Safe` temporarily so the tree stays green (Task 10 replaces this with real resolution). Note for stats continuity: mode strings recorded by `stats::record_call` are now `safe`/`aggressive` instead of `heuristic`; old stat rows keep their historical labels — no migration.

- [ ] **Step 3: Run tests**

Run: `cargo test`
Expected: full suite PASS (old `heuristic` tests now live in `ladder::safe`).

- [ ] **Step 4: Commit**

```bash
git add src/app.rs src/lib.rs src/main.rs
git rm src/heuristic.rs
git commit -m "feat: transform uses compression ladder; retire heuristic module"
```

### Task 10: CLI flag, config keys, and precedence

**Files:**
- Modify: `src/cli.rs:11-35` (flags), `src/cli.rs:38-52` (`Mode::Wrap`), `src/cli.rs:71-90` (`parse_mode`)
- Modify: `src/config.rs`
- Modify: `src/main.rs:6-21`

- [ ] **Step 1: Write the failing tests**

`src/cli.rs` — add `compress` field; reword `heuristic` as deprecated alias:

```rust
/// Compression level for non-adapter output: safe (default) | aggressive
#[arg(long, value_name = "LEVEL")]
pub compress: Option<String>,

/// Deprecated alias for --compress=aggressive
#[arg(long)]
pub heuristic: bool,
```

`Mode::Wrap` carries `compress: Option<String>` alongside the existing `heuristic: bool` (resolution happens in `main.rs` where config is available). Test in `src/cli.rs`:

```rust
#[test]
fn compress_flag_parses() {
    let cli = Cli::parse_from(["cartoon", "--compress", "aggressive", "make"]);
    assert_eq!(cli.compress.as_deref(), Some("aggressive"));
}
```

`src/config.rs` — extend `Config` (keep `heuristic: bool` for backward compat; `true` maps to aggressive):

```rust
use std::collections::HashMap;

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct CompressCfg {
    pub level: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct CommandCfg {
    pub level: Option<String>,
}

// fields added to Config (with #[serde(default)] already on the struct):
//   pub compress: CompressCfg,
//   pub command: HashMap<String, CommandCfg>,
// and add `compress: CompressCfg::default(), command: HashMap::new()`
// to `impl Default for Config`.
```

Config tests:

```rust
#[test]
fn compress_level_parses() {
    let c: Config = toml::from_str("[compress]\nlevel = \"aggressive\"").unwrap();
    assert_eq!(c.compress.level.as_deref(), Some("aggressive"));
}

#[test]
fn per_command_level_parses() {
    let c: Config = toml::from_str("[command.docker]\nlevel = \"aggressive\"").unwrap();
    assert_eq!(c.command["docker"].level.as_deref(), Some("aggressive"));
}
```

- [ ] **Step 2: Implement level resolution**

Pure function in `src/config.rs` (tested there) implementing precedence — CLI `--compress` > CLI `--heuristic` > config `[command.<argv0>]` > config `[compress]` > legacy config `heuristic = true` > Safe:

```rust
pub fn resolve_level(
    flag: Option<&str>,
    heuristic_flag: bool,
    argv0: &str,
    cfg: &Config,
) -> anyhow::Result<crate::ladder::CompressLevel> {
    use crate::ladder::CompressLevel;
    let parse = |s: &str| s.parse::<CompressLevel>().map_err(|e| anyhow::anyhow!(e));
    if let Some(s) = flag {
        return parse(s);
    }
    if heuristic_flag {
        return Ok(CompressLevel::Aggressive);
    }
    if let Some(s) = cfg.command.get(argv0).and_then(|c| c.level.as_deref()) {
        return parse(s);
    }
    if let Some(s) = cfg.compress.level.as_deref() {
        return parse(s);
    }
    if cfg.heuristic {
        return Ok(CompressLevel::Aggressive);
    }
    Ok(CompressLevel::Safe)
}
```

Write one test per precedence rung: flag beats per-command config; `--heuristic` maps to Aggressive; per-command beats global; global beats legacy; legacy `heuristic = true` maps to Aggressive; default is Safe; invalid level string errors. In `main.rs`, replace `let heuristic_on = heuristic || cfg.heuristic;` (src/main.rs:14) with:

```rust
let level = match cartoon::config::resolve_level(
    compress.as_deref(),
    heuristic,
    &argv[0],
    &cfg,
) {
    Ok(l) => l,
    Err(e) => {
        eprintln!("cartoon: {e}");
        std::process::exit(2);
    }
};
```

and pass `level` into `run_wrap` (replacing the Task 9 temporary `CompressLevel::Safe`).

- [ ] **Step 3: Run tests**

Run: `cargo test && cargo clippy --all-targets`
Expected: full suite PASS, clippy clean.

- [ ] **Step 4: Commit**

```bash
git add src/cli.rs src/config.rs src/main.rs
git commit -m "feat: --compress flag, config levels, precedence resolution"
```

### Task 11: Golden corpus harness

**Files:**
- Create: `tests/corpus.rs`
- Create: `tests/corpus/npm-install-fail/log.txt`, `tests/corpus/npm-install-fail/manifest.toml`
- Create: `tests/corpus/cargo-build-fail/log.txt`, `tests/corpus/cargo-build-fail/manifest.toml`
- Modify: `Cargo.toml` (add `toml` to `[dev-dependencies]` if the integration test cannot see the runtime dep)

Manifest format:

```toml
# tests/corpus/npm-install-fail/manifest.toml
must_survive = [
  "npm ERR! code ERESOLVE",
  "npm ERR! Could not resolve dependency",
]
min_reduction_safe = 0.05        # fraction of o200k tokens removed at safe
min_reduction_aggressive = 0.30  # fraction at aggressive
```

- [ ] **Step 1: Capture fixtures**

Capture real logs (`npm install <conflicting-pkg> 2>&1 | tee log.txt`; a `cargo build` with several compile errors). Strip machine-specific content (home paths, usernames) before committing. Start with these two fixtures; grow the corpus over time (make, docker build, gradle, CI runs — passing and failing variants per the spec).

- [ ] **Step 2: Write the harness**

```rust
// tests/corpus.rs
use cartoon::ladder::{compress, CompressLevel};
use cartoon::stats::estimate_tokens;
use serde::Deserialize;

#[derive(Deserialize)]
struct Manifest {
    must_survive: Vec<String>,
    min_reduction_safe: f64,
    min_reduction_aggressive: f64,
}

fn reduction(original: &str, compressed: &str) -> f64 {
    let before = estimate_tokens(original, "o200k") as f64;
    let after = estimate_tokens(compressed, "o200k") as f64;
    if before == 0.0 {
        return 0.0;
    }
    (before - after) / before
}

#[test]
fn corpus_signal_retention_and_token_floor() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut checked = 0;
    for entry in std::fs::read_dir(&root).expect("tests/corpus exists") {
        let dir = entry.unwrap().path();
        if !dir.is_dir() {
            continue;
        }
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        let log = std::fs::read_to_string(dir.join("log.txt")).expect("log.txt");
        let manifest: Manifest =
            toml::from_str(&std::fs::read_to_string(dir.join("manifest.toml")).unwrap())
                .expect("manifest.toml");
        for (level, floor) in [
            (CompressLevel::Safe, manifest.min_reduction_safe),
            (CompressLevel::Aggressive, manifest.min_reduction_aggressive),
        ] {
            let out = compress(&log, level);
            for line in &manifest.must_survive {
                assert!(
                    out.contains(line),
                    "{name}@{level:?}: signal lost: {line}"
                );
            }
            let r = reduction(&log, &out);
            assert!(
                r >= floor,
                "{name}@{level:?}: reduction {r:.2} below floor {floor:.2}"
            );
        }
        checked += 1;
    }
    assert!(checked >= 2, "corpus must contain fixtures");
}
```

- [ ] **Step 3: Run**

Run: `cargo test --test corpus`
Expected: PASS. Tune per-fixture floors to what the captured logs actually achieve — floors document measured reality, they don't aspire.

- [ ] **Step 4: Commit**

```bash
git add tests/corpus.rs tests/corpus/ Cargo.toml
git commit -m "test: golden corpus with signal-retention and token-floor assertions"
```

### Task 12: Docs and disclosure

**Files:**
- Modify: `README.md` (Use section, Guarantees section, config example)
- Modify: `src/cli.rs` (after_help text mentions `--compress`)

- [ ] **Step 1: Update README**

- Use section: add `cartoon --compress=aggressive make` example; note the safe tier is the new default for non-adapter commands; mark `--heuristic` as a deprecated alias for `--compress=aggressive`.
- Guarantees section: replace the heuristic sentence with the spec's updated wording — *the safe tier preserves all non-redundant text; lossy tiers are opt-in and always leave a raw_log pointer to the unmodified output.*
- Config example:

```toml
[compress]
level = "safe"

[command.docker]
level = "aggressive"
```

- [ ] **Step 2: Verify and commit**

Run: `cargo test` (full suite) and `cargo run -- --help` (flag docs render).

```bash
git add README.md src/cli.rs
git commit -m "docs: compression ladder usage, config, and updated guarantees"
```

### Task 13: End-to-end test

**Files:**
- Create: `tests/e2e_ladder.rs` (follow the structure of the existing fast-mode e2e test; gate on unix like that test does if it uses `sh`)

- [ ] **Step 1: Write the test**

```rust
// tests/e2e_ladder.rs
#![cfg(unix)]
use std::process::Command;

fn cartoon_bin() -> &'static str {
    env!("CARGO_BIN_EXE_cartoon")
}

#[test]
fn safe_tier_compresses_and_mirrors_exit_code() {
    // printf emits ANSI + duplicate lines; safe tier must fire.
    let out = Command::new(cartoon_bin())
        .args(["sh", "-c", r"printf '\033[32mok\033[0m\nsame\nsame\nsame\n'; exit 3"])
        .output()
        .expect("run cartoon");
    assert_eq!(out.status.code(), Some(3), "exit code mirrored");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("(x3)"), "repeats collapsed: {stdout}");
    assert!(!stdout.contains("\x1b["), "ANSI stripped");
    assert!(stdout.contains("raw_log"), "disclosure footer present");
}

#[test]
fn raw_flag_bypasses_ladder() {
    let out = Command::new(cartoon_bin())
        .args(["--raw", "sh", "-c", r"printf 'same\nsame\n'"])
        .output()
        .expect("run cartoon");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout, "same\nsame\n", "byte-identical in raw mode");
}
```

- [ ] **Step 2: Run and commit**

Run: `cargo test --test e2e_ladder`
Expected: 2 PASS.

```bash
git add tests/e2e_ladder.rs
git commit -m "test: e2e ladder coverage (safe tier, raw bypass, exit codes)"
```

---

## Phase 2 outline — Drain template mining (own plan required)

**Gate:** Phase 1 merged AND golden-corpus numbers show chatty-log fixtures (service/CI logs) still above target token counts at the aggressive tier.

- Rust port of the Drain3 online template-clustering algorithm in `src/ladder/drain.rs`; no new heavy dependencies.
- Runs in the aggressive tier between `collapse_near_dups` and `extract_diagnostics`; only fires at >= 200 input lines.
- Output: TOON `templates[N]{count,template,sample_vars}` section; WARN/ERROR lines preserved verbatim ahead of it.
- Tunables (tree depth, similarity threshold) fixed constants first; promoted to config only if corpus results demand it.
- Same golden corpus gates it; add >= 2 chatty-service-log fixtures.

## Phase 3 outline — extractive model tier (own plan required)

**Gate:** Phases 1–2 merged AND corpus shows a measurable token margin left on the table.

- ONNX line-scorer interface (`model.path` config; pluggable). Default model from the LLMLingua-2 family via `cartoon model install`; weights never bundled in the binary.
- Extractive only: scores lines, budget-driven top-K selection; cannot invent text.
- `CompressLevel` gains a `Model` variant then; `--compress=model` without an installed model warns on stderr and falls back to aggressive.
- Runtime choice (`ort` vs `candle`) decided in that plan.
- Gated on the same golden corpus before the model becomes default-installable.
