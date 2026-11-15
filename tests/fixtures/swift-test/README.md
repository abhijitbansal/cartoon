`compile-failure.log` is a hand-authored log (not captured from a real run), modelled on `swift build` failure output, for the build-diagnostics fallback test in `src/adapters/swift_test.rs`. The `.xml` files are xunit output.

`compile-failure-color.log` is the same kind of log with the ANSI colour sequences Swift 6.4.2 writes even to a pipe (`path:line:col: ESC[1;31merror: ESC[1;39m...`, measured in the final review: 13 sequences in `swift build 2>&1`, none with `NO_COLOR=1`). Hand-constructed with real ESC bytes, not captured.
