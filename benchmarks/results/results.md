cartoon 0.6.0, Linux x86_64; 600 tests per suite, 20 failing. Tokens: o200k, stdout + stderr.

| suite | baseline | command | baseline tokens | cartoon tokens | saved | exit (raw/cartoon) |
|---|---|---|---:|---:|---:|---|
| pytest | verbose | `pytest -v tests` | 14,217 | 2,464 | **82.7%** | 1/1 |
| pytest | quiet | `pytest -q --tb=short tests` | 1,895 | 1,891 | **0.2%** | 1/1 |
| unittest | verbose | `python3 -m unittest -v` | 14,380 | 1,549 | **89.2%** | 1/1 |
| unittest | quiet | `python3 -m unittest` | 1,820 | 1,550 | **14.8%** | 1/1 |
| cargo test | verbose | `cargo test` | 16,327 | 3,083 | **81.1%** | 101/101 |
| cargo test | quiet | `cargo test -q` | 11,101 | 3,082 | **72.2%** | 101/101 |
| go test | verbose | `go test -v ./...` | 11,893 | 795 | **93.3%** | 1/1 |
| go test | quiet | `go test ./...` | 424 | 423 | **0.2%** | 1/1 |
