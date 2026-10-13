# mysql-s3-backup

## Code Execution

Never run code locally. Use the `/run-rust` skill for every Cargo command (build, test, clippy, fmt). The integration tests need an S3 server and a database: run versitygw and MariaDB with `run-podman --group`, then the test binary in that group (see `tests/integration.rs` for the variables).

## Before Committing

All of these must pass with no warnings:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```
