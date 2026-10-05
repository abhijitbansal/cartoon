//! `npm test` / `pnpm test` / `yarn test` / `bun run test` whose
//! package.json `test` script is a plain jest or vitest invocation: the
//! jest/vitest adapter's machine-output flags are forwarded to the script
//! (npm needs a `--` separator; pnpm, yarn and bun pass trailing args
//! through as they are) and the run is parsed by that adapter.
//!
//! Deliberately conservative — anything else declines (and gets the generic
//! ladder, as before): a script with shell syntax (`&&`, `|`, quotes, `$`),
//! a `pretest`/`posttest` hook (its output would vanish behind the report),
//! package-manager options before the script name, a user-chosen reporter,
//! or a `vitest` that would start in watch mode.
use super::jest::Jest;
use super::vitest::Vitest;
use super::{basename, Adapter, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::{bail, Result};
use serde_json::Value;
use std::io::IsTerminal;

pub struct PkgScript;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Pm {
    Npm,
    Pnpm,
    Yarn,
    Bun,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Runner {
    Jest,
    Vitest,
}

/// What a detected invocation runs: the script's command (env prefix
/// dropped) plus the user's trailing args.
#[derive(Debug, PartialEq)]
struct Plan {
    pm: Pm,
    runner: Runner,
    /// The user typed `npm test -- …`: the separator is already there.
    has_separator: bool,
    /// Script command + user args, as the test runner will see them.
    inner: Vec<String>,
}

impl Adapter for PkgScript {
    fn name(&self) -> &'static str {
        "pkg-script"
    }
    fn matches(&self) -> &'static str {
        "npm test | npm run test | pnpm test | yarn test | bun run test (test script = jest / vitest)"
    }
    fn detect(&self, argv: &[String]) -> bool {
        plan_here(argv).is_some()
    }
    fn prepare(&self, argv: Vec<String>) -> Prepared {
        let Some(plan) = plan_here(&argv) else {
            return Prepared {
                argv,
                artifact: None,
            };
        };
        let inner = match plan.runner {
            Runner::Jest => Jest.prepare(plan.inner.clone()),
            Runner::Vitest => Vitest.prepare(plan.inner.clone()),
        };
        // Both adapters append their flags after the args they were given
        // (`plan` never contains a `--`): forward exactly those.
        let added = match inner.argv.get(..plan.inner.len()) {
            Some(head) if head == plan.inner.as_slice() => inner.argv[plan.inner.len()..].to_vec(),
            _ => Vec::new(),
        };
        let mut argv = argv;
        if plan.pm == Pm::Npm && !plan.has_separator && !added.is_empty() {
            argv.push("--".into());
        }
        argv.extend(added);
        Prepared {
            argv,
            artifact: inner.artifact,
        }
    }
    fn parse(&self, captured: &Captured, prepared: &Prepared) -> Result<ParseOutcome> {
        // The injected flags say which runner the script ran.
        if prepared.argv.iter().any(|a| a == "--testLocationInResults") {
            // The package manager frames the script's stdout (`> pkg test`
            // header, yarn's `Done in 0.4s.`): keep only jest's JSON line.
            let c = Captured {
                stdout: jest_json_line(&captured.stdout)
                    .unwrap_or(&captured.stdout)
                    .to_string(),
                stderr: captured.stderr.clone(),
                status: captured.status,
            };
            Jest.parse(&c, prepared)
        } else if prepared.argv.iter().any(|a| a == "--reporter=json") {
            Vitest.parse(captured, prepared)
        } else {
            bail!("no test runner flags were forwarded to the script");
        }
    }
}

/// The last stdout line that is jest's `--json` document.
fn jest_json_line(stdout: &str) -> Option<&str> {
    stdout.lines().rev().find(|l| {
        let t = l.trim();
        t.starts_with('{')
            && serde_json::from_str::<Value>(t).is_ok_and(|v| v.get("testResults").is_some())
    })
}

/// `plan` against the package.json in the working directory.
fn plan_here(argv: &[String]) -> Option<Plan> {
    // Cheap argv check first: most commands never touch the filesystem.
    split_pm(argv)?;
    let pkg = std::fs::read_to_string("package.json").ok()?;
    // vitest's own default: watch only when stdin is a terminal and not CI.
    let one_shot = !std::io::stdin().is_terminal() || env_truthy(std::env::var("CI").ok());
    plan(argv, &pkg, one_shot)
}

fn env_truthy(v: Option<String>) -> bool {
    v.is_some_and(|v| !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false"))
}

/// npm options that only change npm's own logging.
const NPM_QUIET_FLAGS: &[&str] = &["--silent", "-s", "--quiet", "-q"];

/// The package manager, the user's trailing args (forwarded to the script)
/// and whether an npm `--` separator was typed. None when this is not a
/// plain `test` script run.
fn split_pm(argv: &[String]) -> Option<(Pm, &[String], bool)> {
    let first = argv.first()?;
    let pm = match basename(first) {
        "npm" | "npm.cmd" => Pm::Npm,
        "pnpm" | "pnpm.cmd" => Pm::Pnpm,
        "yarn" | "yarn.cmd" => Pm::Yarn,
        "bun" | "bun.exe" => Pm::Bun,
        _ => return None,
    };
    let rest = &argv[1..];
    let s = |i: usize| rest.get(i).map(String::as_str);
    // `bun test` is bun's own runner, not the package script.
    let after: &[String] = match (pm, s(0), s(1)) {
        (Pm::Npm, Some("run" | "run-script" | "rum" | "urn"), Some("test")) => &rest[2..],
        (Pm::Npm, Some("test" | "t" | "tst"), _) => &rest[1..],
        (Pm::Pnpm, Some("run"), Some("test")) => &rest[2..],
        (Pm::Pnpm, Some("test" | "t" | "tst"), _) => &rest[1..],
        (Pm::Yarn, Some("run"), Some("test")) => &rest[2..],
        (Pm::Yarn, Some("test"), _) => &rest[1..],
        (Pm::Bun, Some("run"), Some("test")) => &rest[2..],
        _ => return None,
    };
    if pm == Pm::Npm {
        // Before `--` everything is npm's: only its logging flags are known
        // not to change what runs.
        let sep = after.iter().position(|a| a == "--");
        let own = &after[..sep.unwrap_or(after.len())];
        if !own.iter().all(|a| NPM_QUIET_FLAGS.contains(&a.as_str())) {
            return None;
        }
        return match sep {
            Some(i) if !after[i + 1..].iter().any(|a| a == "--") => {
                Some((pm, &after[i + 1..], true))
            }
            Some(_) => None,
            None => Some((pm, &[], false)),
        };
    }
    // pnpm passes a literal `--` to the script; yarn 1 strips it. Not
    // worth modelling: decline.
    if after.iter().any(|a| a == "--") {
        return None;
    }
    Some((pm, after, false))
}

/// Script tokens that need no shell interpretation.
fn plain_token(t: &str) -> bool {
    !t.is_empty()
        && t.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_./:=@,+-".contains(c))
}

/// `NAME=value` env prefix token.
fn env_assignment(t: &str) -> Option<(&str, &str)> {
    let (name, value) = t.split_once('=')?;
    let ok = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && !name.starts_with(|c: char| c.is_ascii_digit());
    ok.then_some((name, value))
}

/// vitest subcommands that are not a single batch run.
const VITEST_NON_RUN: &[&str] = &[
    "watch",
    "dev",
    "bench",
    "init",
    "list",
    "typecheck",
    "related",
];

/// The pure decision: `argv` against package.json's text. `one_shot`: would
/// a bare `vitest` run once here (stdin not a terminal, or CI set)?
fn plan(argv: &[String], package_json: &str, one_shot: bool) -> Option<Plan> {
    let (pm, tail, has_separator) = split_pm(argv)?;
    let pkg: Value = serde_json::from_str(package_json).ok()?;
    let scripts = pkg.get("scripts")?;
    // A pre/post hook runs too, and its output would be hidden.
    if scripts.get("pretest").is_some() || scripts.get("posttest").is_some() {
        return None;
    }
    let script = scripts.get("test")?.as_str()?;
    let tokens: Vec<&str> = script.split_whitespace().collect();
    if !tokens.iter().all(|t| plain_token(t)) {
        return None;
    }
    let mut ci_in_script = false;
    let mut i = 0;
    while let Some((name, value)) = tokens.get(i).and_then(|t| env_assignment(t)) {
        if name == "CI" {
            ci_in_script = env_truthy(Some(value.to_string()));
        }
        i += 1;
    }
    let mut inner: Vec<String> = tokens[i..].iter().map(|t| t.to_string()).collect();
    if inner.is_empty() || inner.iter().any(|t| t == "--") {
        return None;
    }
    inner.extend(tail.iter().cloned());
    // A reporter / output file the user picked: their output, not ours.
    if inner.iter().any(|a| {
        let name = a.split('=').next().unwrap_or(a);
        name == "--reporter" || name == "--json" || name.starts_with("--outputFile")
    }) {
        return None;
    }
    let runner = if Jest.detect(&inner) {
        Runner::Jest
    } else if Vitest.detect(&inner)
        || (is_bare_vitest(&inner)
            && (one_shot || ci_in_script || inner.iter().any(|a| a == "--run")))
    {
        // `vitest run`, or a bare `vitest` that runs once here.
        Runner::Vitest
    } else {
        return None;
    };
    Some(Plan {
        pm,
        runner,
        has_separator,
        inner,
    })
}

/// `vitest [filters] [flags]` (or via npx/bunx/pnpx) with no subcommand
/// other than `run` and no watch flag: a batch run unless vitest decides
/// on watch mode.
fn is_bare_vitest(inner: &[String]) -> bool {
    let rest = match inner {
        [first, rest @ ..] if basename(first) == "vitest" => rest,
        [first, second, rest @ ..]
            if matches!(basename(first), "npx" | "bunx" | "pnpx")
                && basename(second) == "vitest" =>
        {
            rest
        }
        _ => return false,
    };
    !rest.iter().any(|a| {
        VITEST_NON_RUN.contains(&a.as_str()) || a == "-w" || a.split('=').next() == Some("--watch")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    fn pkg(test: &str) -> String {
        serde_json::json!({ "scripts": { "test": test } }).to_string()
    }

    fn runner_for(cmd: &[&str], test: &str) -> Option<Runner> {
        plan(&argv(cmd), &pkg(test), true).map(|p| p.runner)
    }

    #[test]
    fn package_manager_forms() {
        for cmd in [
            &["npm", "test"][..],
            &["npm", "t"],
            &["npm", "run", "test"],
            &["npm", "run-script", "test"],
            &["npm", "test", "--silent"],
            &["npm", "test", "--", "src/a.test.js"],
            &["pnpm", "test"],
            &["pnpm", "run", "test", "src/a.test.js"],
            &["yarn", "test"],
            &["yarn", "run", "test"],
            &["bun", "run", "test"],
        ] {
            assert_eq!(runner_for(cmd, "jest"), Some(Runner::Jest), "{cmd:?}");
        }
        for cmd in [
            // npm config flags / workspaces change what runs.
            &["npm", "test", "--workspace", "a"][..],
            &["npm", "--prefix", "x", "test"],
            &["npm", "test", "src"],
            &["pnpm", "--filter", "a", "test"],
            &["pnpm", "test", "--", "x"],
            &["yarn", "workspace", "a", "test"],
            // bun's own runner, not the package script.
            &["bun", "test"],
            &["npm", "ci"],
            &["npm", "run", "lint"],
            &["npx", "jest"],
        ] {
            assert_eq!(runner_for(cmd, "jest"), None, "{cmd:?}");
        }
    }

    #[test]
    fn script_shapes() {
        let npm = &["npm", "test"][..];
        assert_eq!(runner_for(npm, "jest"), Some(Runner::Jest));
        assert_eq!(runner_for(npm, "jest --coverage src"), Some(Runner::Jest));
        assert_eq!(runner_for(npm, "npx jest"), Some(Runner::Jest));
        assert_eq!(runner_for(npm, "CI=true jest"), Some(Runner::Jest));
        assert_eq!(runner_for(npm, "vitest run"), Some(Runner::Vitest));
        assert_eq!(runner_for(npm, "vitest --run"), Some(Runner::Vitest));
        assert_eq!(runner_for(npm, "npx vitest run src"), Some(Runner::Vitest));
        for script in [
            "jest && echo done",
            "jest; echo",
            "jest | tee out",
            "jest \"src\"",
            "jest $ARGS",
            "cross-env NODE_ENV=test jest",
            "node_modules/.bin/jest --watch",
            "jest --watchAll",
            "mocha",
            "tsc && jest",
            "vitest watch",
            "vitest bench",
            "vitest run --watch",
            "jest --json --outputFile=r.json",
            "vitest run --reporter=junit",
            "",
        ] {
            assert_eq!(runner_for(npm, script), None, "{script:?}");
        }
    }

    #[test]
    fn user_args_are_checked_too() {
        assert_eq!(runner_for(&["npm", "test", "--", "--watch"], "jest"), None);
        assert_eq!(
            runner_for(&["pnpm", "test", "--reporter=dot"], "vitest run"),
            None
        );
        assert_eq!(
            runner_for(&["pnpm", "test", "-t", "adds"], "vitest run"),
            Some(Runner::Vitest)
        );
    }

    #[test]
    fn bare_vitest_only_when_it_would_not_watch() {
        let cmd = argv(&["npm", "test"]);
        assert_eq!(
            plan(&cmd, &pkg("vitest"), true).map(|p| p.runner),
            Some(Runner::Vitest)
        );
        // stdin is a terminal and CI is unset: vitest would watch forever.
        assert_eq!(plan(&cmd, &pkg("vitest"), false), None);
        assert_eq!(
            plan(&cmd, &pkg("CI=true vitest"), false).map(|p| p.runner),
            Some(Runner::Vitest)
        );
        assert_eq!(plan(&cmd, &pkg("CI=false vitest"), false), None);
        // `vitest run` never watches.
        assert!(plan(&cmd, &pkg("vitest run"), false).is_some());
    }

    #[test]
    fn pre_and_post_hooks_decline() {
        let cmd = argv(&["npm", "test"]);
        let with_pre = r#"{"scripts":{"pretest":"eslint .","test":"jest"}}"#;
        let with_post = r#"{"scripts":{"posttest":"echo ok","test":"jest"}}"#;
        assert_eq!(plan(&cmd, with_pre, true), None);
        assert_eq!(plan(&cmd, with_post, true), None);
        assert_eq!(plan(&cmd, "{}", true), None);
        assert_eq!(plan(&cmd, "not json", true), None);
    }

    #[test]
    fn plan_carries_script_and_user_args() {
        let p = plan(
            &argv(&["npm", "test", "--", "-t", "adds"]),
            &pkg("NODE_ENV=test jest src"),
            true,
        )
        .unwrap();
        assert_eq!(p.inner, argv(&["jest", "src", "-t", "adds"]));
        assert!(p.has_separator);
    }

    #[test]
    fn jest_json_line_skips_package_manager_framing() {
        let out = "\n> demo@1.0.0 test\n> jest\n\n{\"testResults\":[],\"numTotalTests\":0}\nDone in 0.4s.\n";
        assert_eq!(
            jest_json_line(out),
            Some("{\"testResults\":[],\"numTotalTests\":0}")
        );
        assert_eq!(jest_json_line("{\"other\":1}\n"), None);
    }
}
