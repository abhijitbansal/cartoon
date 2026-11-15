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
        "npm" if args.first() == Some(&"ci") => args[1..].iter().all(|a| NPM_CI_FLAGS.contains(a)),
        // A goal with `:` (`org.x:plugin:1.0:goal`) downloads and runs an
        // arbitrary plugin; lifecycle phases have none.
        "mvn" | "mvnw" => args.iter().all(|a| a.starts_with('-') || !a.contains(':')),
        "dotnet" => dotnet_args_ok(args),
        "xcodebuild" => args.iter().all(|a| xcode_setting_ok(a)),
        _ => true,
    };
    special
        && ARG_POLICIES
            .iter()
            .filter(|p| p.tools.contains(&tool))
            .all(|p| p.allows(args))
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
