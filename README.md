# mysql-s3-backup

[![CI](https://github.com/joanfabregat/mysql-s3-backup/actions/workflows/ci.yml/badge.svg)](https://github.com/joanfabregat/mysql-s3-backup/actions/workflows/ci.yml)
[![Build and push Docker image](https://github.com/joanfabregat/mysql-s3-backup/actions/workflows/docker-image.yml/badge.svg)](https://github.com/joanfabregat/mysql-s3-backup/actions/workflows/docker-image.yml)
[![GitHub release](https://img.shields.io/github/v/release/joanfabregat/mysql-s3-backup?include_prereleases&sort=semver)](https://github.com/joanfabregat/mysql-s3-backup/releases)
[![Docker Pulls](https://img.shields.io/docker/pulls/joanfabregat/mysql-s3-backup)](https://hub.docker.com/r/joanfabregat/mysql-s3-backup)
[![Docker Image Size](https://img.shields.io/docker/image-size/joanfabregat/mysql-s3-backup/latest)](https://hub.docker.com/r/joanfabregat/mysql-s3-backup)
[![Rust](https://img.shields.io/badge/rust-1.94.1%2B-orange?logo=rust)](Cargo.toml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

A container that backs up MySQL and MariaDB databases to Amazon S3 or any S3-compatible provider (Backblaze B2, Garage, MinIO, …). It runs `mysqldump`, gzips the output, and streams it to the bucket as a multipart upload, without writing the dump to disk.

## How it works

A small Rust binary drives the backup. `mysqldump` (from Alpine's `mariadb-client`) still produces the dump; the binary handles the rest:

1. Runs `mysqldump` for one database and reads its output through a pipe.
2. Compresses it with gzip into a buffer the size of one upload part (16 MiB by default).
3. Each time the buffer fills, sends it to S3 as the next part of a multipart upload, two parts at a time.
4. Sends `CompleteMultipartUpload` only after `mysqldump` has exited 0.

Until step 4, S3 holds only invisible parts, not an object. If `mysqldump` fails, stalls past its timeout, or the container is stopped, the binary sends `AbortMultipartUpload` and the parts are discarded. **A truncated dump never becomes an object**, which matters when the bucket is under Object Lock and a bad object cannot be deleted.

Memory, not disk, bounds the work: at most three parts are held at once (one filling, two uploading), so 48 MiB of buffers at the default part size, whatever the database size. If the upload is slower than the dump, reading pauses and `mysqldump` waits on the pipe; the log reports how long, because with the default locking mode the table locks stay held while it waits (see [Dump consistency and locking](#dump-consistency-and-locking)).

## Features

- Streams dumps to S3 with no local disk use, and never leaves a partial object behind
- Optional `--single-transaction` dumps that do not block writes, with a warning that names tables it cannot snapshot
- TCP or Unix socket connections, individual variables or a `DATABASE_URL`
- Several databases per run, through a comma-separated `MYSQL_DATABASE`
- Any S3-compatible provider through `S3_ENDPOINT_URL`, with path-style addressing
- A Content-MD5 on every part, checked by the provider
- Wall-clock timeouts that fit inside a Kubernetes Job's `activeDeadlineSeconds`
- Clean shutdown on `SIGTERM`: `mysqldump` is stopped and the upload aborted
- A built-in success ping for heartbeat monitors such as UptimeRobot, with no sidecar
- The database password goes through `MYSQL_PWD`, never the command line
- Multi-architecture images (`linux/amd64`, `linux/arm64`), with a binary built by `cargo-auditable` so image scanners see its Rust dependencies

## Configuration

Everything is configured through environment variables.

### Database

Either a `DATABASE_URL`:

```
DATABASE_URL=mysql://user:password@hostname:3306/database
DATABASE_URL=mysql://user:password@/database?unix_socket=/run/mysqld/mysqld.sock
```

or individual variables:

| Variable | Default | Description |
|---|---|---|
| `MYSQL_HOST` | | Server host name. Required unless `MYSQL_SOCKET` is set. |
| `MYSQL_PORT` | `3306` | Server port. |
| `MYSQL_SOCKET` | | Unix socket path. Takes precedence over `MYSQL_HOST`. |
| `MYSQL_USER` | | Required. |
| `MYSQL_PASSWORD` | | Passed to `mysqldump` through `MYSQL_PWD`. |
| `MYSQL_DATABASE` | | Required. A comma-separated list backs up each database in turn. |
| `MYSQL_SINGLE_TRANSACTION` | `0` | Dump with `--single-transaction`. Accepts `1`/`0`, `true`/`false`, `yes`/`no`, `on`/`off`; anything else is rejected at startup. See [Dump consistency and locking](#dump-consistency-and-locking). |

`DATABASE_URL` overrides the user, password, host and database variables. The port and socket from the URL apply only when present in it. The URL is not percent-decoded: write the password as it is.

### S3

| Variable | Default | Description |
|---|---|---|
| `S3_BUCKET` | | Required. |
| `S3_PREFIX` | `/` | Key prefix, for example `backups/mysql`. |
| `S3_ENDPOINT_URL` | | Endpoint of an S3-compatible provider. Enables path-style addressing. |
| `S3_STORAGE_CLASS` | `STANDARD_IA` | Set it to an empty string for providers that reject storage classes (B2, Garage, MinIO). |
| `S3_PART_SIZE_MIB` | `16` | Upload part size, 5 to 5120. S3 allows 10,000 parts, so the default handles dumps up to about 156 GiB compressed. Memory use is about three times this value. |
| `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` | | Credentials. Any source the AWS SDK supports also works: IRSA, EKS Pod Identity, ECS task roles, instance profiles, `AWS_PROFILE`. |
| `AWS_REGION` or `AWS_DEFAULT_REGION` | `us-east-1` | Bucket region. |

### Timeouts

| Variable | Default | Description |
|---|---|---|
| `DUMP_TIMEOUT` | `600` | Seconds allowed for one attempt at one database, dump and upload together. |
| `UPLOAD_TIMEOUT` | `300` | Seconds allowed for each S3 request attempt. |
| `TIMEOUT_GRACE` | `30` | Seconds between `SIGTERM` and `SIGKILL` when stopping `mysqldump`. |

### Heartbeat

| Variable | Default | Description |
|---|---|---|
| `HEARTBEAT_URL` | | An `http://` or `https://` URL to GET once every database is uploaded, for a heartbeat monitor (UptimeRobot, Healthchecks.io, Better Stack) that alerts when the ping does not arrive. |

The ping is sent only when the whole run succeeded, so a failed, timed-out or interrupted backup alerts through the missing ping. Each attempt has 10 seconds, and a failed ping is retried twice, 2 seconds apart. **A ping that still fails does not fail the backup**: the run logs a warning and exits 0, because the backup is stored and a non-zero exit would make a Kubernetes Job dump everything again. The monitor alerts on the missing ping anyway. The URL is never logged, since it usually carries the monitor's token; keep it in a Secret.

## Timeouts, retries and Kubernetes deadlines

Without timeouts, a stalled dump or upload hangs forever, and on Kubernetes a CronJob with `concurrencyPolicy: Forbid` then skips every later run while looking idle. A Job-level `activeDeadlineSeconds` is the outer net; these timeouts are the inner one, and they say which step stalled.

- **A failed dump is not retried.** `mysqldump` exiting non-zero, or an attempt running past `DUMP_TIMEOUT`, ends the run with exit code 1.
- **A failed upload is retried once, with a fresh dump**, 5 seconds later. The SDK already retries each S3 request up to 3 times; this covers what that cannot.
- With `MYSQL_SINGLE_TRANSACTION` enabled, a 30-second engine check runs before each database.

Worst case per database: `2 × DUMP_TIMEOUT`, plus 30 s with `MYSQL_SINGLE_TRANSACTION`. At the defaults that is 1,230 s, and the total is that times the number of databases, plus 34 s with `HEARTBEAT_URL`. **Keep it inside the Job's `activeDeadlineSeconds`**, or the deadline always fires first and these timeouts never get to say what stalled. For scale, a 3.0 GiB gzipped dump (9.4 GB raw) took 155 s to an S3-compatible server on the same LAN, so 600 s is about 4× that. Raise `DUMP_TIMEOUT` for a larger database, and raise `activeDeadlineSeconds` to match.

## Dump consistency and locking

By default `mysqldump` runs with `--lock-tables` (part of `--opt`): it takes `LOCK TABLES … READ LOCAL` over every table in a database and holds it for that database's whole dump. **Writes to those tables block for as long as the dump takes**, including any time spent waiting on a slow upload. In exchange, every table comes from the same point in time, whatever its storage engine.

`MYSQL_SINGLE_TRANSACTION=1` adds `--single-transaction`, which reads inside a transaction instead of locking. Writes are never blocked, and the backup user no longer needs `LOCK TABLES`.

The catch: **`--single-transaction` is only a consistent snapshot of transactional tables.** InnoDB tables are safe. MyISAM, Aria and MEMORY tables are read outside the transaction, so a write landing mid-dump can tear them silently. That is why it is off by default.

| | `MYSQL_SINGLE_TRANSACTION=0` (default) | `MYSQL_SINGLE_TRANSACTION=1` |
|---|---|---|
| Writes during the dump | Blocked | Not blocked |
| InnoDB consistency | Yes | Yes |
| MyISAM / Aria / MEMORY consistency | Yes | **No, can be torn** |
| Grants needed | `SELECT, SHOW VIEW, TRIGGER, LOCK TABLES` | `SELECT, SHOW VIEW, TRIGGER` |

**All-InnoDB deployment:** enable it. **Mixed engines:** convert the remaining tables to InnoDB (it has supported `FULLTEXT` since MySQL 5.6, the usual reason a MyISAM table is still around); until then, leaving it off trades a known cost for a known guarantee.

When enabled, the binary queries `information_schema` before each dump and logs every non-InnoDB table, so a mixed schema shows up in the backup log rather than in a failed restore:

```
Dumping MySQL database: mixeddb
Warning: MYSQL_SINGLE_TRANSACTION is enabled but 'mixeddb' has non-transactional tables:
Warning:   doc_recherche_index (MyISAM)
Warning: these are dumped outside the transaction and a concurrent write can tear them.
Warning: convert them to InnoDB, or unset MYSQL_SINGLE_TRANSACTION to lock instead.
```

The check is advisory: if it cannot run, it warns and the backup proceeds.

**Neither mode gives a consistent snapshot across databases.** Each database in a comma-separated `MYSQL_DATABASE` gets its own `mysqldump`, so its own lock or transaction window.

## Usage

### Docker

```bash
docker run --rm \
  -e DATABASE_URL="mysql://backup:password@db.example.com:3306/app" \
  -e S3_BUCKET="my-backup-bucket" \
  -e S3_PREFIX="mysql/daily" \
  -e AWS_ACCESS_KEY_ID="..." \
  -e AWS_SECRET_ACCESS_KEY="..." \
  -e AWS_REGION="us-west-1" \
  joanfabregat/mysql-s3-backup:v4
```

### Docker Compose

```yaml
services:
  mysql-backup:
    image: joanfabregat/mysql-s3-backup:v4
    environment:
      MYSQL_HOST: db
      MYSQL_USER: backup
      MYSQL_PASSWORD: password
      MYSQL_DATABASE: app,analytics
      S3_BUCKET: my-backup-bucket
      S3_PREFIX: backups/mysql
      AWS_ACCESS_KEY_ID: ...
      AWS_SECRET_ACCESS_KEY: ...
      AWS_REGION: us-west-1
```

### Backblaze B2 and other S3-compatible providers

```yaml
    environment:
      S3_ENDPOINT_URL: https://s3.us-west-004.backblazeb2.com
      S3_STORAGE_CLASS: ""
      AWS_REGION: us-west-004
```

### Kubernetes CronJob

```yaml
apiVersion: batch/v1
kind: CronJob
metadata:
  name: mysql-backup
spec:
  schedule: "0 2 * * *"
  concurrencyPolicy: Forbid
  jobTemplate:
    spec:
      activeDeadlineSeconds: 3600
      backoffLimit: 0
      template:
        spec:
          restartPolicy: Never
          containers:
            - name: mysql-backup
              image: joanfabregat/mysql-s3-backup:v4
              resources:
                requests: { cpu: 250m, memory: 96Mi }
                limits: { memory: 192Mi }
              envFrom:
                - secretRef:
                    name: mysql-backup
              env:
                - name: MYSQL_SINGLE_TRANSACTION
                  value: "true"
                - name: S3_PREFIX
                  value: mysql/daily
```

No ephemeral-storage request is needed: nothing is written to disk.

## Objects

One run uses one UTC timestamp for every database:

```
<S3_PREFIX>/<YYYY-MM-DDTHHMMSSZ>.sql.gz                 one database
<S3_PREFIX>/<database>/<YYYY-MM-DDTHHMMSSZ>.sql.gz      several databases
```

Keys are identical to v3's, so existing lifecycle rules and restore scripts keep working. Restore with:

```bash
aws s3 cp s3://my-backup-bucket/mysql/daily/2026-10-13T020000Z.sql.gz - | gunzip | mariadb app
```

**Add a lifecycle rule with `AbortIncompleteMultipartUpload`** (one day is enough). Every failure path aborts its upload, but a container killed with `SIGKILL` cannot, and its parts are billed as storage until something removes them.

## Logs and exit codes

A run with `HEARTBEAT_URL` set logs:

```
mysql-s3-backup 4.0.0
Dumping MySQL database: app
Streaming dump to S3: s3://my-backup-bucket/mysql/daily/2026-10-13T020000Z.sql.gz
Uploaded successfully: 924.3 MiB compressed (4121.7 MiB raw) in 58 part(s), 124.6s.
mysqldump of 'app' waited 3.2s on the upload.
Backup complete.
Heartbeat sent.
```

| Exit code | Meaning |
|---|---|
| `0` | Every database was backed up. |
| `1` | Invalid configuration, or a database failed. Databases after it are not attempted. |
| `130`, `143` | Stopped by `SIGINT` or `SIGTERM`; the current upload was aborted. |

## Upgrading from v3

v4 replaces the shell script and `s5cmd` with the Rust binary. The environment variables and object keys are unchanged. What changed:

- **No temporary files.** Remove the ephemeral-storage sizing for `/tmp`; set a memory limit instead (about three times `S3_PART_SIZE_MIB` plus a few MiB).
- **`DUMP_TIMEOUT` now covers the dump and the upload together**, since they run as one stream. `UPLOAD_TIMEOUT` now applies to each S3 request. Recheck the budget in [Timeouts, retries and Kubernetes deadlines](#timeouts-retries-and-kubernetes-deadlines).
- **An upload failure retries the whole database** (2 attempts) instead of re-sending the file (3 attempts).
- **The entrypoint is `/usr/local/bin/mysql-s3-backup`.** `/app/backup.sh`, `bash`, `gzip` and `s5cmd` are gone from the image.
- **Checksums:** every part carries a Content-MD5. The AWS SDK's CRC checksums are only sent where S3 requires them, because not every compatible provider supports them.
- **New:** `S3_PART_SIZE_MIB`, and `HEARTBEAT_URL`, which replaces a sidecar container that pinged a monitor after the backup.

## Security

- Prefer workload identity (IRSA, EKS Pod Identity, instance roles) over static keys.
- Give the database user only what the dump needs: `SELECT, SHOW VIEW, TRIGGER, LOCK TABLES`, or `SELECT, SHOW VIEW, TRIGGER` with `MYSQL_SINGLE_TRANSACTION=1`.
- Keep credentials in a secret store, and encrypt the bucket.
- Consider Object Lock on the bucket; incomplete dumps never become objects, so it cannot lock in a bad one.

## Troubleshooting

1. **`Error: dump of '…' failed: mysqldump failed (exit 2)`**: `mysqldump`'s own message is printed just above. Usually a connection, grant or unknown-database error.
2. **`Access denied … when using LOCK TABLES`**: grant `LOCK TABLES`, or set `MYSQL_SINGLE_TRANSACTION=1`.
3. **Writes stall during the backup**: `mysqldump`'s default locking. See [Dump consistency and locking](#dump-consistency-and-locking). If the log shows a long wait on the upload, the upload is extending the lock: use `MYSQL_SINGLE_TRANSACTION=1`, or a faster link to the bucket.
4. **`exceeded 600s (DUMP_TIMEOUT)`**: raise `DUMP_TIMEOUT` and the Job's `activeDeadlineSeconds` together.
5. **`InvalidStorageClass` or similar on an S3-compatible provider**: set `S3_STORAGE_CLASS=""`.
6. **Container killed for memory**: raise the memory limit, or lower `S3_PART_SIZE_MIB`.

## Development

The code is a single binary crate: `src/config.rs` (environment), `src/dump.rs` (`mysqldump` and the streaming pipeline), `src/s3.rs` (multipart upload), `src/heartbeat.rs` (the success ping).

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test                     # unit tests; integration tests skip themselves
docker build -t mysql-s3-backup .
```

The integration tests in `tests/integration.rs` need an S3-compatible server and, for most of them, a MySQL or MariaDB server and the `mariadb` client on `PATH`:

```bash
TEST_S3_ENDPOINT=http://127.0.0.1:7070 AWS_ACCESS_KEY_ID=... AWS_SECRET_ACCESS_KEY=... \
TEST_MYSQL_HOST=127.0.0.1 TEST_MYSQL_USER=root TEST_MYSQL_PASSWORD=... \
TEST_REQUIRE_INTEGRATION=1 cargo test --test integration
```

Set `TEST_IMAGE=<image>` to run them through a Docker image instead of the Cargo-built binary. CI runs them against MySQL 8.4, MariaDB 11.4 and MariaDB 11.8 with [versitygw](https://github.com/versity/versitygw) as the S3 server, and once more through the built image.

## License

[MIT](LICENSE)
