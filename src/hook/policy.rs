//! Per-tool argument policy for the hook: which arguments may ride along
//! on an auto-approved rewrite. Every check here fails closed: a hit means
//! no rewrite, and the command goes through the normal permission prompt.
use super::{basename, env_assignment_name, RUNNERS};

/// How one tool spells the options that load code, config or a toolchain
/// from a path (or set properties that can), so `tool_args_ok` catches every
/// spelling. Any hit ends auto-wrap eligibility: no rewrite, normal prompt.
struct ArgPolicy {
    tools: &'static [&'static str],
    /// Long option names, normalized by `norm` (lowercase, no `-`/`_`):
    /// `--name`, `--name=v`, `--Name`, `--na-me`.
    long: &'static [&'static str],
    /// Like `long`, but denied when the normalized name merely *contains* the
    /// word (JS runners expose dotted config keys: `--coverage.customProviderModule`).
    long_contains: &'static [&'static str],
    /// Long names may be abbreviated to any unique prefix (argparse,
    /// getopt_long, Ruby OptionParser): `--confi` denies like `--config-file`.
    abbrev: bool,
    /// Single-dash options are long options too (Go's flag package,
    /// xcodebuild): `-exec` == `--exec`.
    single_dash_long: bool,
    /// Short options, case-sensitive, matched as a prefix of a single-dash
    /// token: `-c`, `-cfile`, `-c=file`, `-Dprop=v`, `-Xswiftc`, `-gs`.
    short: &'static [&'static str],
    /// Short flags bundle (`-qpfoo` = `-q -p foo`): a one-letter `short`
    /// anywhere in a single-dash cluster is a hit. Fails closed on a value
    /// glued to another flag (`-kcopy`), which is acceptable.
    bundled: bool,
    /// Exact tokens that are fine although a `short` prefix matches them.
    allow: &'static [&'static str],
    /// (hit, value check): a hit on this `long`/`long_contains`/`short`
    /// entry is fine when its value (glued or the next token) passes.
    exempt: &'static [(&'static str, ValueCheck)],
}

/// Whether an otherwise denied option's value is inert.
type ValueCheck = fn(&str) -> bool;

impl ArgPolicy {
    const NONE: ArgPolicy = ArgPolicy {
        tools: &[],
        long: &[],
        long_contains: &[],
        abbrev: false,
        single_dash_long: false,
        short: &[],
        bundled: false,
        allow: &[],
        exempt: &[],
    };
}

/// The JS runners (and the package-manager `test` scripts that usually run
/// them) share one policy: jest's yargs and vitest's cac both accept camel
/// and kebab case, and most config keys are also CLI flags.
const JS_LONG: &[&str] = &[
    "env",
    "envfile",
    "api",
    "ui",
    "open",
    "prefix",
    "registry",
    "call",
    "scriptshell",
    "nodeoptions",
];
const JS_LONG_CONTAINS: &[&str] = &[
    "config",
    "setup",
    "teardown",
    "runner",
    "transform",
    "resolver",
    "environment",
    "reporter",
    "processor",
    "plugin",
    "sequencer",
    "provider",
    "preset",
    "project",
    "root",
    "dir",
    "workspace",
    "module",
    "loader",
    "preload",
    "require",
    "import",
    "define",
    "tsconfig",
    "cwd",
    "inspect",
    "browser",
    "prettier",
    "haste",
    "pool",
];

fn js_builtin_reporter(v: &str) -> bool {
    [
        "default",
        "verbose",
        "dot",
        "json",
        "junit",
        "tap",
        "tap-flat",
        "basic",
        "summary",
        "tree",
        "github-actions",
        "hanging-process",
    ]
    .contains(&v)
}

fn js_builtin_pool(v: &str) -> bool {
    ["threads", "forks", "vmThreads", "vmForks"].contains(&v)
}

fn pytest_disable_plugin(v: &str) -> bool {
    v.starts_with("no:")
}

/// mvn `-D` properties common in a test loop that select or skip work and
/// can't point the build at outside code.
fn mvn_safe_property(v: &str) -> bool {
    let name = v.split('=').next().unwrap_or(v);
    [
        "skipTests",
        "maven.test.skip",
        "skipITs",
        "test",
        "it.test",
        "failIfNoTests",
        "surefire.failIfNoSpecifiedTests",
        "checkstyle.skip",
        "spotbugs.skip",
        "pmd.skip",
        "jacoco.skip",
        "enforcer.skip",
        "spotless.check.skip",
        "maven.javadoc.skip",
        "gpg.skip",
        "style.color",
    ]
    .contains(&name)
}

const ARG_POLICIES: &[ArgPolicy] = &[
    ArgPolicy {
        tools: &["go"],
        single_dash_long: true,
        long: &[
            "exec",
            "toolexec",
            "overlay",
            "modfile",
            "vettool",
            "ldflags",
            "gcflags",
            "asmflags",
            "gccgoflags",
            "compiler",
            "pkgdir",
        ],
        short: &["C"],
        ..ArgPolicy::NONE
    },
    ArgPolicy {
        tools: &["cargo"],
        long: &["config", "configfile", "toolconfigfile"],
        short: &["Z", "C"],
        bundled: true,
        ..ArgPolicy::NONE
    },
    ArgPolicy {
        tools: &["make"],
        long: &["file", "makefile", "directory", "includedir", "eval"],
        abbrev: true,
        short: &["f", "C", "I", "E"],
        bundled: true,
        ..ArgPolicy::NONE
    },
    ArgPolicy {
        tools: &["jest", "vitest", "npm", "pnpm", "yarn", "bun"],
        long: JS_LONG,
        long_contains: JS_LONG_CONTAINS,
        short: &["c", "r", "C"],
        bundled: true,
        exempt: &[("reporter", js_builtin_reporter), ("pool", js_builtin_pool)],
        ..ArgPolicy::NONE
    },
    ArgPolicy {
        tools: &["eslint"],
        long: &[
            "config",
            "rulesdir",
            "resolvepluginsrelativeto",
            "plugin",
            "parser",
            "inspectconfig",
            "mcp",
        ],
        short: &["c"],
        ..ArgPolicy::NONE
    },
    ArgPolicy {
        tools: &["mypy"],
        long: &[
            "configfile",
            "pythonexecutable",
            "installtypes",
            "customtypesheddir",
            "shadowfile",
        ],
        abbrev: true,
        ..ArgPolicy::NONE
    },
    ArgPolicy {
        // pytest disables argparse abbreviations (`--confcut` is an error).
        tools: &["pytest"],
        long: &[
            "confcutdir",
            "rootdir",
            "overrideini",
            "configfile",
            "inifile",
            "basetemp",
            "pdbcls",
            "covconfig",
            "tx",
            "rsyncdir",
        ],
        short: &["p", "c", "o"],
        bundled: true,
        exempt: &[("p", pytest_disable_plugin)],
        ..ArgPolicy::NONE
    },
    ArgPolicy {
        tools: &["phpunit"],
        long: &[
            "bootstrap",
            "configuration",
            "includepath",
            "extension",
            "printer",
            "loader",
            "generateconfiguration",
            "migrateconfiguration",
        ],
        abbrev: true,
        short: &["c", "d"],
        ..ArgPolicy::NONE
    },
    ArgPolicy {
        tools: &["rspec"],
        long: &["require", "options", "defaultpath"],
        abbrev: true,
        short: &["r", "I", "O"],
        bundled: true,
        ..ArgPolicy::NONE
    },
    ArgPolicy {
        tools: &["gradle", "gradlew"],
        long: &[
            "initscript",
            "settingsfile",
            "buildfile",
            "systemprop",
            "projectprop",
            "gradleuserhome",
            "includebuild",
            "scan",
        ],
        short: &["I", "c", "b", "D", "P", "g"],
        ..ArgPolicy::NONE
    },
    ArgPolicy {
        tools: &["mvn", "mvnw"],
        long: &[
            "settings",
            "globalsettings",
            "toolchains",
            "globaltoolchains",
            "file",
            "define",
        ],
        short: &["s", "gs", "t", "gt", "f", "D"],
        allow: &["-fae", "-ff", "-fn"],
        exempt: &[("D", mvn_safe_property), ("define", mvn_safe_property)],
        ..ArgPolicy::NONE
    },
    ArgPolicy {
        tools: &["swift"],
        long: &[
            "toolchain",
            "sdk",
            "swiftsdk",
            "swiftsdkspath",
            "destination",
            "disablesandbox",
            "packagepath",
            "scratchpath",
            "buildpath",
            "cachepath",
            "configpath",
            "securitypath",
            "pkgconfigpath",
            "netrcfile",
        ],
        short: &["X"],
        ..ArgPolicy::NONE
    },
    ArgPolicy {
        tools: &["xcodebuild"],
        single_dash_long: true,
        long: &[
            "xcconfig",
            "toolchain",
            "xctestrun",
            "skipmacrovalidation",
            "skippackagepluginvalidation",
        ],
        ..ArgPolicy::NONE
    },
    ArgPolicy {
        tools: &["pre-commit"],
        long: &["config"],
        abbrev: true,
        short: &["c"],
        bundled: true,
        ..ArgPolicy::NONE
    },
];

/// `npm ci` installs exactly the lockfile; these flags only quiet it down or
/// narrow it. Anything else (`--registry`, `--prefix`, ...) is not wrapped.
const NPM_CI_FLAGS: &[&str] = &[
    "--ignore-scripts",
    "--no-audit",
    "--no-fund",
    "--no-progress",
    "--silent",
    "--quiet",
    "-q",
    "--prefer-offline",
    "--offline",
    "--include=dev",
    "--omit=dev",
    "--omit=optional",
    "--omit=peer",
    "--loglevel=error",
    "--loglevel=warn",
    "--loglevel=silent",
];

/// dotnet/MSBuild switches (any of `-x`, `--x`, `/x`, value after `:` or
/// `=`) that set properties, load loggers/adapters/settings, or change the
/// package source or environment.
const DOTNET_DENY: &[&str] = &[
    "p",
    "property",
    "rp",
    "restoreproperty",
    "l",
    "logger",
    "dl",
    "distributedlogger",
    "s",
    "settings",
    "a",
    "testadapterpath",
    "e",
    "environment",
    "source",
    "configfile",
    "collect",
];

/// Xcode build settings safe to pass on the command line (`NAME=value`);
/// any other upper-case setting (`CC=`, `SWIFT_EXEC=`, `OTHER_LDFLAGS=`)
/// can swap the compiler or linker.
const XCODE_SAFE_SETTINGS: &[&str] = &[
    "CODE_SIGNING_ALLOWED",
    "CODE_SIGNING_REQUIRED",
    "CODE_SIGN_IDENTITY",
    "ONLY_ACTIVE_ARCH",
    "ENABLE_TESTABILITY",
    "COMPILER_INDEX_STORE_ENABLE",
    "SWIFT_TREAT_WARNINGS_AS_ERRORS",
    "GCC_TREAT_WARNINGS_AS_ERRORS",
];

/// Gradle tasks (the last `:`-separated segment) that only compile, test or
/// check. Any task starting with `test` or `compile`, or ending with `Test`,
/// qualifies too. Gradle also accepts camel-case abbreviations (`tDUT`);
/// those match none of this and are not wrapped.
const GRADLE_TASKS: &[&str] = &[
    "test",
    "build",
    "check",
    "assemble",
    "clean",
    "lint",
    "compileJava",
    "compileKotlin",
    "compileTestJava",
    "compileTestKotlin",
    "classes",
    "testClasses",
];

/// Words that name an outbound side effect (publishing, deploying,
/// installing onto a device, pushing an image, signing). A gradle task or
/// make target containing one, in any case, is never wrapped.
const SIDE_EFFECT_WORDS: &[&str] = &[
    "publish",
    "deploy",
    "upload",
    "install",
    "release",
    "push",
    "connected",
    "docker",
    "sign",
    "bintray",
    "sonatype",
    "notarize",
];

/// gradle options whose next token is a value (a test filter, an excluded
/// task, a console mode), not a task to run.
const GRADLE_VALUE_FLAGS: &[&str] = &[
    "--tests",
    "-x",
    "--exclude-task",
    "--console",
    "--warning-mode",
    "--max-workers",
];

/// Maven lifecycle phases up to `verify`. `install` and `deploy` come after
/// it and copy artifacts out of the project; `site` is not a test loop.
const MVN_PHASES: &[&str] = &[
    "clean",
    "validate",
    "compile",
    "test-compile",
    "test",
    "package",
    "verify",
];

/// mvn options whose next token is a value (a module list, a thread count),
/// not a phase. `-P <profile>` is deliberately absent: a profile can bind
/// extra plugins to a phase, so its value is judged like a phase and fails.
const MVN_VALUE_FLAGS: &[&str] = &[
    "-pl",
    "--projects",
    "-rf",
    "--resume-from",
    "-T",
    "--threads",
];

/// make targets that build, test or check. `test-*`/`test_*` (and the same
/// for `check`, `lint`, `build`) qualify too. Bare `make` runs the default
/// goal and stays allowed (see the module doc).
const MAKE_TARGETS: &[&str] = &[
    "all",
    "build",
    "test",
    "tests",
    "check",
    "lint",
    "ci",
    "unit",
    "typecheck",
    "verify",
    "compile",
    "debug",
];
const MAKE_TARGET_PREFIXES: &[&str] = &["test", "check", "lint", "build"];

/// make options that may take a number as the next token (`-j 4`,
/// `-l 2.5`). Any other bare word after a flag is a target.
const MAKE_NUMERIC_FLAGS: &[&str] = &["-j", "--jobs", "-l", "--load-average", "--max-load"];

/// Resolve the tool a noisy command actually runs (`npx jest` → jest,
/// `python -m pytest` → pytest, uv's `-m pytest` → pytest) and check its
/// arguments against `tool_args_ok`.
pub(super) fn args_ok(base: &str, rest: &[&str]) -> bool {
    if RUNNERS.contains(&base) {
        return rest
            .split_first()
            .is_some_and(|(t, r)| tool_args_ok(basename(t), r));
    }
    let module = if base.starts_with("python") {
        rest.get(1..) // past `-m`
    } else if base == "-m" || base == "--module" {
        Some(rest)
    } else {
        return tool_args_ok(base, rest);
    };
    module
        .and_then(|m| m.split_first())
        .is_some_and(|(m, r)| tool_args_ok(m, r))
}

/// Per-tool argument policy: the `ARG_POLICIES` deny-lists plus the
/// allowlists that need more than a flag name.
pub(super) fn tool_args_ok(tool: &str, args: &[&str]) -> bool {
    let first_positional = || args.iter().copied().find(|a| !a.starts_with('-'));
    let special = match tool {
        // Only a toolchain chosen by the project, and only nextest's
        // read-mostly sub-subcommands (`cargo nextest self update` replaces
        // the binary).
        "cargo" => {
            !args.iter().any(|a| a.starts_with('+'))
                && (args.first() != Some(&"nextest")
                    || matches!(args.get(1), Some(&"run" | &"r" | &"list")))
        }
        // `make CC=/tmp/x` / `SHELL=` / `MAKE=` override the Makefile.
        "make" => !args.iter().any(|a| env_assignment_name(a).is_some()),
        // `install`, `autoupdate`, `try-repo <url>`, `gc`, ... are not a
        // dev-loop run (try-repo runs hooks from an arbitrary repo).
        "pre-commit" => first_positional().is_none_or(|p| p == "run"),
        // Without `--ignore-scripts`, `npm ci` runs every dependency's
        // install scripts: third-party code from the registry.
        "npm" if args.first() == Some(&"ci") => {
            args[1..].contains(&"--ignore-scripts")
                && args[1..].iter().all(|a| NPM_CI_FLAGS.contains(a))
        }
        // A goal with `:` (`org.x:plugin:1.0:goal`) downloads and runs an
        // arbitrary plugin; lifecycle phases have none.
        "mvn" | "mvnw" => args.iter().all(|a| a.starts_with('-') || !a.contains(':')),
        "dotnet" => dotnet_args_ok(args),
        "xcodebuild" => args.iter().all(|a| xcode_setting_ok(a)),
        _ => true,
    };
    special
        && targets_ok(tool, args)
        && !watch_mode(tool, args)
        && ARG_POLICIES
            .iter()
            .filter(|p| p.tools.contains(&tool))
            .all(|p| p.allows(args))
}

/// Every gradle task, mvn phase or make target in `args` passes
/// `side_effect_free_target`. The value of a listed option (`--tests X`,
/// `-pl core`, `make -j 4`) is not a target; any other non-flag token is.
fn targets_ok(tool: &str, args: &[&str]) -> bool {
    let (value_flags, value_ok): (&[&str], fn(&str) -> bool) = match tool {
        "gradle" | "gradlew" => (GRADLE_VALUE_FLAGS, |_| true),
        "mvn" | "mvnw" => (MVN_VALUE_FLAGS, |_| true),
        "make" => (MAKE_NUMERIC_FLAGS, |v| {
            v.chars().all(|c| c.is_ascii_digit() || c == '.')
        }),
        _ => return true,
    };
    let mut rest = args.iter();
    while let Some(a) = rest.next() {
        if !a.starts_with('-') {
            if !side_effect_free_target(tool, a) {
                return false;
            }
            continue;
        }
        let takes_value = value_flags.contains(a)
            && rest
                .as_slice()
                .first()
                .is_some_and(|v| !v.starts_with('-') && value_ok(v));
        if takes_value {
            rest.next();
        }
    }
    true
}

/// True when `target` (a gradle task path, mvn phase or make target) only
/// builds, tests or checks, with no outbound side effect such as publishing,
/// deploying, installing or pushing. Other tools have no target concept.
pub(super) fn side_effect_free_target(tool: &str, target: &str) -> bool {
    let names_side_effect = |t: &str| {
        let lower = t.to_ascii_lowercase();
        SIDE_EFFECT_WORDS.iter().any(|w| lower.contains(w))
    };
    match tool {
        "gradle" | "gradlew" => {
            // `:app:testDebugUnitTest` is judged by its task, not its module.
            let task = target.rsplit(':').next().unwrap_or(target);
            // `ends_with("Test")` also covers `...UnitTest`.
            let known = GRADLE_TASKS.contains(&task)
                || task.starts_with("test")
                || task.starts_with("compile")
                || task.ends_with("Test");
            known && !names_side_effect(task)
        }
        "mvn" | "mvnw" => MVN_PHASES.contains(&target),
        "make" => {
            let known = MAKE_TARGETS.contains(&target)
                || MAKE_TARGET_PREFIXES.iter().any(|p| {
                    target
                        .strip_prefix(p)
                        .is_some_and(|r| r.starts_with(['-', '_']))
                });
            known && !names_side_effect(target)
        }
        _ => true,
    }
}

/// Watch and interactive modes never return (or open a browser), so they
/// are not a one-shot dev-loop run.
fn watch_mode(tool: &str, args: &[&str]) -> bool {
    let has_long = |names: &[&str]| args.iter().any(|a| long_named(a, names));
    match tool {
        // `npm|pnpm|yarn|bun test -- --watch` hands the flag to the runner.
        "jest" | "npm" | "pnpm" | "yarn" | "bun" => has_long(&["watch", "watchall"]),
        "vitest" => {
            has_long(&["watch"]) || args.iter().any(|a| matches!(*a, "-w" | "watch" | "dev"))
        }
        "tsc" => has_long(&["watch"]) || args.contains(&"-w"),
        "pytest" => has_long(&["looponfail"]) || args.iter().any(|a| pytest_loop_on_fail(a)),
        "gradle" | "gradlew" => has_long(&["continuous"]) || args.contains(&"-t"),
        "cargo" => args.first() == Some(&"doc") && args.contains(&"--open"),
        _ => false,
    }
}

/// `a` is `--name` or `--name=v` with a `norm`ed name in `names`, unless the
/// value is `false` (`--watchAll=false` turns watching off).
fn long_named(a: &str, names: &[&str]) -> bool {
    let Some(body) = a.strip_prefix("--") else {
        return false;
    };
    let (name, value) = match body.split_once('=') {
        Some((n, v)) => (n, Some(v)),
        None => (body, None),
    };
    names.contains(&norm(name).as_str()) && value != Some("false")
}

/// pytest-xdist's `-f` (loop on failure), alone or bundled after boolean
/// short flags (`-xf`, `-vvf`). `-rf...` is not it: `-r` takes the rest of
/// the cluster as report characters.
fn pytest_loop_on_fail(a: &str) -> bool {
    a.strip_prefix('-')
        .filter(|b| !b.starts_with('-'))
        .is_some_and(|b| {
            b.trim_start_matches(['x', 'q', 'v', 's', 'l'])
                .starts_with('f')
        })
}

impl ArgPolicy {
    /// True when no argument hits this policy (or every hit is exempt).
    fn allows(&self, args: &[&str]) -> bool {
        let mut i = 0;
        while i < args.len() {
            let a = args[i];
            i += 1;
            if self.allow.contains(&a) {
                continue;
            }
            let Some((hit, glued)) = self.hit(a) else {
                continue;
            };
            let Some((_, value_ok)) = self.exempt.iter().find(|(e, _)| *e == hit) else {
                return false;
            };
            let value = match glued {
                Some(v) => Some(v),
                None => {
                    i += 1;
                    args.get(i - 1).copied()
                }
            };
            if !value.is_some_and(value_ok) {
                return false;
            }
        }
        true
    }

    /// The denied option `a` spells, with its glued value if any.
    fn hit<'a>(&self, a: &'a str) -> Option<(&'static str, Option<&'a str>)> {
        if let Some(body) = a.strip_prefix("--") {
            return self.long_hit(body);
        }
        let body = a.strip_prefix('-').filter(|b| !b.is_empty())?;
        if self.single_dash_long {
            if let Some(h) = self.long_hit(body) {
                return Some(h);
            }
        }
        let glued = |r: &'a str| Some(r.strip_prefix('=').unwrap_or(r)).filter(|v| !v.is_empty());
        if let Some(s) = self.short.iter().find(|s| body.starts_with(**s)) {
            return Some((*s, glued(&body[s.len()..])));
        }
        if self.bundled {
            for (at, c) in body.char_indices() {
                if let Some(s) = self.short.iter().find(|s| s.len() == 1 && s.starts_with(c)) {
                    return Some((*s, glued(&body[at + c.len_utf8()..])));
                }
            }
        }
        None
    }

    fn long_hit<'a>(&self, body: &'a str) -> Option<(&'static str, Option<&'a str>)> {
        let (name, value) = match body.split_once('=') {
            Some((n, v)) => (n, Some(v)),
            None => (body, None),
        };
        let name = norm(name);
        if name.is_empty() {
            return None; // `--`: end of options
        }
        let long = self
            .long
            .iter()
            .find(|d| **d == name || (self.abbrev && d.starts_with(&name)));
        let contains = || self.long_contains.iter().find(|d| name.contains(**d));
        long.or_else(contains).map(|d| (*d, value))
    }
}

/// Option-name normalization: case and `-`/`_` don't distinguish options
/// for any tool here (yargs/cac accept both camel and kebab case).
fn norm(name: &str) -> String {
    name.chars()
        .filter(|c| *c != '-' && *c != '_')
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// dotnet: no MSBuild response files (`@x.rsp`), no inline runsettings
/// after `--`, and none of `DOTNET_DENY` in any `-x`/`--x`/`/x` spelling.
fn dotnet_args_ok(args: &[&str]) -> bool {
    args.iter().all(|a| {
        if *a == "--" || a.starts_with('@') {
            return false;
        }
        let body = a
            .strip_prefix("--")
            .or_else(|| a.strip_prefix('-'))
            .or_else(|| a.strip_prefix('/'));
        let Some(body) = body else {
            return true;
        };
        let name = body.split([':', '=']).next().unwrap_or(body);
        !DOTNET_DENY.contains(&norm(name).as_str())
    })
}

/// An upper-case `NAME=value` (or `NAME[sdk=*]=value`) argument to
/// xcodebuild is a build setting; only `XCODE_SAFE_SETTINGS` may be set.
fn xcode_setting_ok(a: &str) -> bool {
    let Some((lhs, _)) = a.split_once('=') else {
        return true;
    };
    let name = lhs.split('[').next().unwrap_or(lhs);
    let is_setting = !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    !is_setting || XCODE_SAFE_SETTINGS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::super::wrap_command_in;
    use std::path::Path;

    fn wrap(cmd: &str) -> Option<String> {
        let root = Path::new("/proj");
        wrap_command_in(cmd, &[], Some(root)).map(|(w, _)| w)
    }

    #[test]
    fn gradle_publish_not_wrapped() {
        for c in [
            "gradle build publish",
            "gradle test publishToMavenCentral",
            "./gradlew assembleRelease uploadArchives",
            "gradle test --continuous",
        ] {
            assert!(wrap(c).is_none(), "{c}");
        }
    }

    #[test]
    fn gradle_test_tasks_wrapped() {
        for c in [
            "gradle test",
            "gradle clean build",
            "./gradlew testDebugUnitTest",
            "gradle :app:test check",
        ] {
            assert!(wrap(c).is_some(), "{c}");
        }
    }

    #[test]
    fn gradle_module_paths_judged_by_last_segment() {
        assert!(wrap("gradle :lib:testDebugUnitTest").is_some());
        assert!(wrap("gradle :app:publish").is_none());
        assert!(wrap("gradle connectedAndroidTest").is_none());
    }

    #[test]
    fn gradle_continuous_and_side_effect_words_rejected() {
        for c in [
            "gradle test -t",
            "gradle test --continuous",
            "gradle installDebug",
            "gradle testReleaseSigning",
            "gradle dockerBuildImage",
            "gradle assembleDebug",
            "gradle compileJava jibDockerBuild",
            "gradle test --tests Foo publish",
        ] {
            assert!(wrap(c).is_none(), "{c}");
        }
    }

    #[test]
    fn gradle_flag_values_are_not_tasks() {
        for c in [
            "./gradlew test --tests 'com.x.FooTest'",
            "./gradlew test --tests '*Foo*'",
            "gradle test -x lint",
            "gradle test --console plain --warning-mode all",
            "gradle test --continue",
            "./gradlew build --offline",
        ] {
            assert!(wrap(c).is_some(), "{c}");
        }
    }

    #[test]
    fn mvn_deploy_install_not_wrapped() {
        for c in ["mvn test deploy", "mvn package install", "mvnw verify site"] {
            assert!(wrap(c).is_none(), "{c}");
        }
        assert!(wrap("mvn clean verify").is_some());
    }

    #[test]
    fn mvn_lifecycle_phases_and_module_selection_wrapped() {
        for c in [
            "mvn validate",
            "mvn clean test-compile",
            "mvn test -pl core -am",
            "mvn verify -T 4",
            "mvn package -B -ntp -fae",
        ] {
            assert!(wrap(c).is_some(), "{c}");
        }
        for c in ["mvn test -P release", "mvn site", "mvn release:prepare"] {
            assert!(wrap(c).is_none(), "{c}");
        }
    }

    #[test]
    fn make_side_effect_targets_not_wrapped() {
        for c in [
            "make deploy",
            "make release",
            "make install",
            "make test && make deploy",
            "make push-image",
        ] {
            assert!(wrap(c).is_none(), "{c}");
        }
        for c in [
            "make",
            "make test",
            "make -j4 build",
            "make lint check",
            "make test-unit",
        ] {
            assert!(wrap(c).is_some(), "{c}");
        }
    }

    #[test]
    fn make_flags_unchanged_and_prefixed_targets_still_screened() {
        for c in [
            "make -j 4 test",
            "make -k -j8 check",
            "make -s -n build",
            "make --jobs 2 ci",
            "make check_all typecheck",
        ] {
            assert!(wrap(c).is_some(), "{c}");
        }
        for c in [
            "make build-and-deploy",
            "make test-publish",
            "make testing",
            "make docs",
            "make -j deploy",
        ] {
            assert!(wrap(c).is_none(), "{c}");
        }
    }

    #[test]
    fn side_effect_segment_poisons_the_whole_command() {
        assert!(wrap("pytest && make deploy").is_none());
        assert!(wrap("pytest tests && gradle publish").is_none());
    }

    #[test]
    fn npm_ci_needs_ignore_scripts() {
        assert!(wrap("npm ci").is_none());
        assert!(wrap("npm ci --ignore-scripts").is_some());
        assert!(wrap("npm ci --no-audit --ignore-scripts --no-fund").is_some());
        assert!(wrap("npm ci --no-audit --no-fund").is_none());
    }

    #[test]
    fn watch_modes_not_wrapped() {
        for c in [
            "jest --watchAll",
            "jest --watch",
            "vitest --watch",
            "vitest watch",
            "tsc -w",
            "npm test -- --watch",
            "pytest -f",
            "cargo doc --open",
        ] {
            assert!(wrap(c).is_none(), "{c}");
        }
    }

    #[test]
    fn watch_mode_spellings_not_wrapped() {
        for c in [
            "npx jest --watch-all",
            "vitest -w",
            "vitest dev",
            "npx vitest --watch=true",
            "tsc --watch",
            "tsc -b --watch",
            "pnpm test -- --watchAll",
            "yarn test --watch",
            "npm run test -- --watch",
            "bun test --watch",
            "pytest --looponfail",
            "pytest -xf",
            "python -m pytest -vf",
            "cargo doc --no-deps --open",
        ] {
            assert!(wrap(c).is_none(), "{c}");
        }
    }

    #[test]
    fn one_shot_runs_with_watch_like_flags_still_wrapped() {
        for c in [
            "yarn test --watchAll=false",
            "jest --watchman",
            "jest -w 4",
            "pytest -rfE",
            "pytest --lf",
            "vitest run",
            "tsc --noEmit",
            "cargo doc --no-deps",
        ] {
            assert!(wrap(c).is_some(), "{c}");
        }
    }
}
