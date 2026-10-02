cartoon 0.6.0, Linux x86_64; 600 tests per suite, 20 failing. Tokens: o200k, stdout + stderr.

| suite | baseline | command | baseline tokens | cartoon tokens | saved | exit (raw/cartoon) |
|---|---|---|---:|---:|---:|---|
| pytest | verbose | `pytest -v tests` | 14,204 | 2,458 | **82.7%** | 1/1 |
| pytest | quiet | `pytest -q --tb=short tests` | 1,887 | 1,879 | **0.4%** | 1/1 |
| unittest | verbose | `python3 -m unittest -v` | 14,400 | 2,023 | **86.0%** | 1/1 |
| unittest | quiet | `python3 -m unittest` | 1,840 | 1,840 | **0.0%** | 1/1 |
| cargo test | verbose | `cargo test` | 16,327 | 9,265 | **43.3%** | 101/101 |
| cargo test | quiet | `cargo test -q` | 11,095 | 9,217 | **16.9%** | 101/101 |
| go test | verbose | `go test -v ./...` | 11,893 | 1,304 | **89.0%** | 1/1 |
| go test | quiet | `go test ./...` | 733 | 1,305 | **-78.0%** | 1/1 |
