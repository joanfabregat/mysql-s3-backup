//! End-to-end tests against a real S3-compatible server and, for the tests
//! that need one, a real MySQL or MariaDB server.
//!
//! Configured through the environment; CI sets all of it (see
//! .github/workflows/ci.yml):
//!
//! - `TEST_S3_ENDPOINT` plus `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`:
//!   the S3 server. Without it every test is skipped, unless
//!   `TEST_REQUIRE_INTEGRATION=1`, which turns a skip into a failure.
//! - `TEST_MYSQL_HOST`, `TEST_MYSQL_PORT`, `TEST_MYSQL_USER`,
//!   `TEST_MYSQL_PASSWORD`: a server the user can create databases on. The
//!   `mariadb` (or `mysql`) client must be on PATH to seed and restore.
//! - `TEST_IMAGE`: run a Docker image (with `--network host`) instead of the
//!   binary built by Cargo. Tests that replace mysqldump with a fake are
//!   skipped in that mode.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aws_sdk_s3::Client;
use aws_sdk_s3::config::Region;

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Returns false, so the caller can skip, when `needed` is not configured.
fn configured(needed: &str) -> bool {
    if var(needed).is_some() {
        return true;
    }
    if var("TEST_REQUIRE_INTEGRATION").is_some() {
        panic!("{needed} is not set but TEST_REQUIRE_INTEGRATION is");
    }
    eprintln!("skipping: {needed} is not set");
    false
}

fn image() -> Option<String> {
    var("TEST_IMAGE")
}

/// A name unique to this test run, usable as a database and a bucket name.
fn unique(label: &str) -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    format!(
        "it{}{}{}{label}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
        nanos % 100_000
    )
}

async fn s3() -> Client {
    let shared = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .load()
        .await;
    let config = aws_sdk_s3::config::Builder::from(&shared)
        .endpoint_url(var("TEST_S3_ENDPOINT").unwrap())
        .force_path_style(true)
        .build();
    Client::from_conf(config)
}

async fn create_bucket(client: &Client, label: &str) -> String {
    let bucket = unique(label).to_lowercase().replace('_', "-");
    client.create_bucket().bucket(&bucket).send().await.unwrap();
    bucket
}

async fn keys(client: &Client, bucket: &str) -> Vec<String> {
    let out = client
        .list_objects_v2()
        .bucket(bucket)
        .send()
        .await
        .unwrap();
    let mut keys: Vec<String> = out
        .contents()
        .iter()
        .filter_map(|o| o.key().map(str::to_owned))
        .collect();
    keys.sort();
    keys
}

async fn pending_uploads(client: &Client, bucket: &str) -> usize {
    let out = client
        .list_multipart_uploads()
        .bucket(bucket)
        .send()
        .await
        .unwrap();
    out.uploads().len()
}

async fn download_gunzipped(client: &Client, bucket: &str, key: &str) -> Vec<u8> {
    let object = client
        .get_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await
        .unwrap();
    let gz = object.body.collect().await.unwrap().into_bytes();
    let mut raw = Vec::new();
    flate2::read::GzDecoder::new(&gz[..])
        .read_to_end(&mut raw)
        .unwrap();
    raw
}

/// Timestamped key, as `<prefix><YYYY-MM-DDTHHMMSSZ>.sql.gz`.
fn assert_backup_key(key: &str, prefix: &str) {
    let rest = key
        .strip_prefix(prefix)
        .unwrap_or_else(|| panic!("{key} does not start with {prefix}"));
    let stamp = rest
        .strip_suffix(".sql.gz")
        .unwrap_or_else(|| panic!("{key} is not a .sql.gz"));
    assert_eq!(stamp.len(), "2026-10-13T120000Z".len(), "{key}");
    assert!(
        stamp.ends_with('Z') && stamp.as_bytes()[10] == b'T',
        "{key}"
    );
}

/// Environment for one backup run: the S3 server, credentials, and `extra`.
fn backup_env(bucket: &str, extra: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = vec![
        ("S3_BUCKET".into(), bucket.into()),
        ("S3_ENDPOINT_URL".into(), var("TEST_S3_ENDPOINT").unwrap()),
        ("S3_STORAGE_CLASS".into(), String::new()),
        ("AWS_REGION".into(), "us-east-1".into()),
    ];
    for name in ["AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY", "HOME"] {
        if let Some(value) = var(name) {
            env.push((name.into(), value));
        }
    }
    env.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    env
}

fn db_env() -> Vec<(&'static str, String)> {
    vec![
        ("MYSQL_HOST", var("TEST_MYSQL_HOST").unwrap()),
        (
            "MYSQL_PORT",
            var("TEST_MYSQL_PORT").unwrap_or_else(|| "3306".into()),
        ),
        ("MYSQL_USER", var("TEST_MYSQL_USER").unwrap()),
        (
            "MYSQL_PASSWORD",
            var("TEST_MYSQL_PASSWORD").unwrap_or_default(),
        ),
    ]
}

/// The backup command: the Cargo-built binary, or `docker run` of TEST_IMAGE.
/// `fake_bin` goes first on PATH, to replace mysqldump.
fn backup_command(env: &[(String, String)], fake_bin: Option<&Path>) -> Command {
    let mut command = match image() {
        Some(image) => {
            assert!(
                fake_bin.is_none(),
                "fake mysqldump is not supported with TEST_IMAGE"
            );
            let mut command = Command::new("docker");
            command.args(["run", "--rm", "--network", "host"]);
            for (name, _) in env {
                if name != "HOME" {
                    command.args(["-e", name]);
                }
            }
            command.arg(image);
            command
        }
        None => {
            let mut command = Command::new(env!("CARGO_BIN_EXE_mysql-s3-backup"));
            command.env_clear();
            let path = var("PATH").unwrap_or_default();
            let path = match fake_bin {
                Some(dir) => format!("{}:{path}", dir.display()),
                None => path,
            };
            command.env("PATH", path);
            command
        }
    };
    command.envs(env.iter().map(|(k, v)| (k, v)));
    command
}

fn run_backup(env: &[(String, String)], fake_bin: Option<&Path>) -> Output {
    let output = backup_command(env, fake_bin).output().unwrap();
    eprintln!("--- stdout\n{}", String::from_utf8_lossy(&output.stdout));
    eprintln!("--- stderr\n{}", String::from_utf8_lossy(&output.stderr));
    output
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Writes `script` as an executable `mariadb-dump` in a fresh directory.
fn fake_mysqldump(script: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(unique("fake"));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mariadb-dump");
    std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    dir
}

fn client_program() -> &'static str {
    let found = |name: &str| {
        Command::new(name)
            .arg("--version")
            .stdout(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    if found("mariadb") { "mariadb" } else { "mysql" }
}

/// Runs SQL through the command-line client and returns its batch output.
fn sql(database: Option<&str>, input: &[u8]) -> String {
    let mut command = Command::new(client_program());
    command
        .args(["--batch", "--skip-column-names"])
        .args(["-h", &var("TEST_MYSQL_HOST").unwrap()])
        .args([
            "-P",
            &var("TEST_MYSQL_PORT").unwrap_or_else(|| "3306".into()),
        ])
        .args(["-u", &var("TEST_MYSQL_USER").unwrap()])
        .env("MYSQL_PWD", var("TEST_MYSQL_PASSWORD").unwrap_or_default())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(database) = database {
        command.arg(database);
    }
    let mut child = command.spawn().unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "SQL failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// Creates a database with InnoDB data that exercises escaping: UTF-8 beyond
/// the BMP, binary bytes, NULLs, quotes, and a view.
fn seed(db: &str) {
    sql(
        None,
        format!(
            "CREATE DATABASE `{db}` CHARACTER SET utf8mb4;
             USE `{db}`;
             CREATE TABLE widgets (
               id INT PRIMARY KEY AUTO_INCREMENT,
               name VARCHAR(100) CHARACTER SET utf8mb4,
               payload BLOB,
               created DATETIME
             ) ENGINE=InnoDB;
             INSERT INTO widgets (name, payload, created) VALUES
               ('héllo wörld 🐸', UNHEX('00FF0A0D5C27'), '2026-01-01 00:00:00'),
               ('it''s \"quoted\"', NULL, NULL);
             INSERT INTO widgets (name, payload)
               WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM seq WHERE n < 900)
               SELECT CONCAT('widget ', n), UNHEX(MD5(n)) FROM seq;
             CREATE VIEW widget_names AS SELECT id, name FROM widgets;"
        )
        .as_bytes(),
    );
}

fn checksum(db: &str) -> String {
    let out = sql(
        Some(db),
        b"CHECKSUM TABLE widgets; SELECT COUNT(*) FROM widgets;",
    );
    // "<db>.widgets\t<sum>\n<count>\n": drop the database name.
    out.split_once('\t')
        .map(|(_, rest)| rest.to_owned())
        .unwrap()
}

fn drop_database(db: &str) {
    sql(None, format!("DROP DATABASE IF EXISTS `{db}`").as_bytes());
}

#[tokio::test]
async fn backup_restores_to_an_identical_database() {
    if !configured("TEST_S3_ENDPOINT") || !configured("TEST_MYSQL_HOST") {
        return;
    }
    let client = s3().await;
    let bucket = create_bucket(&client, "roundtrip").await;
    let db = unique("rt");
    seed(&db);

    let mut extra: Vec<(&str, String)> = db_env();
    extra.push(("MYSQL_DATABASE", db.clone()));
    extra.push(("S3_PREFIX", "nightly".into()));
    let extra: Vec<(&str, &str)> = extra.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let output = run_backup(&backup_env(&bucket, &extra), None);
    assert!(output.status.success());
    assert!(stdout(&output).contains("Backup complete."));

    let keys = keys(&client, &bucket).await;
    assert_eq!(keys.len(), 1, "{keys:?}");
    assert_backup_key(&keys[0], "nightly/");
    assert_eq!(pending_uploads(&client, &bucket).await, 0);

    let dump = download_gunzipped(&client, &bucket, &keys[0]).await;
    let restored = format!("{db}_r");
    sql(
        None,
        format!("CREATE DATABASE `{restored}` CHARACTER SET utf8mb4").as_bytes(),
    );
    sql(Some(&restored), &dump);
    assert_eq!(checksum(&restored), checksum(&db));
    let view = sql(
        Some(&restored),
        b"SELECT name FROM widget_names WHERE id = 1",
    );
    assert_eq!(view.trim(), "héllo wörld 🐸");

    drop_database(&db);
    drop_database(&restored);
}

#[tokio::test]
async fn database_url_and_multiple_databases() {
    if !configured("TEST_S3_ENDPOINT") || !configured("TEST_MYSQL_HOST") {
        return;
    }
    let client = s3().await;
    let bucket = create_bucket(&client, "multi").await;
    let (first, second) = (unique("a"), unique("b"));
    seed(&first);
    seed(&second);

    let url = format!(
        "mysql://{}:{}@{}:{}/ {first} , {second} ",
        var("TEST_MYSQL_USER").unwrap(),
        var("TEST_MYSQL_PASSWORD").unwrap_or_default(),
        var("TEST_MYSQL_HOST").unwrap(),
        var("TEST_MYSQL_PORT").unwrap_or_else(|| "3306".into()),
    );
    let output = run_backup(&backup_env(&bucket, &[("DATABASE_URL", &url)]), None);
    assert!(output.status.success());

    let keys = keys(&client, &bucket).await;
    assert_eq!(keys.len(), 2, "{keys:?}");
    assert_backup_key(&keys[0], &format!("{first}/"));
    assert_backup_key(&keys[1], &format!("{second}/"));
    // One timestamp per run, shared by every database.
    assert_eq!(keys[0].rsplit('/').next(), keys[1].rsplit('/').next());

    drop_database(&first);
    drop_database(&second);
}

#[tokio::test]
async fn single_transaction_names_non_transactional_tables() {
    if !configured("TEST_S3_ENDPOINT") || !configured("TEST_MYSQL_HOST") {
        return;
    }
    let client = s3().await;
    let bucket = create_bucket(&client, "engines").await;
    let db = unique("eng");
    seed(&db);
    sql(
        Some(&db),
        b"CREATE TABLE legacy_index (id INT PRIMARY KEY) ENGINE=MyISAM",
    );

    let mut extra: Vec<(&str, String)> = db_env();
    extra.push(("MYSQL_DATABASE", db.clone()));
    extra.push(("MYSQL_SINGLE_TRANSACTION", "true".into()));
    let extra: Vec<(&str, &str)> = extra.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let output = run_backup(&backup_env(&bucket, &extra), None);
    assert!(output.status.success());
    let err = stderr(&output);
    assert!(
        err.contains(&format!("'{db}' has non-transactional tables:")),
        "{err}"
    );
    assert!(err.contains("Warning:   legacy_index (MyISAM)"), "{err}");
    assert!(!err.contains("widgets"), "{err}");
    assert_eq!(keys(&client, &bucket).await.len(), 1);

    drop_database(&db);
}

#[tokio::test]
async fn missing_database_fails_without_an_object() {
    if !configured("TEST_S3_ENDPOINT") || !configured("TEST_MYSQL_HOST") {
        return;
    }
    let client = s3().await;
    let bucket = create_bucket(&client, "missing").await;

    let mut extra: Vec<(&str, String)> = db_env();
    extra.push(("MYSQL_DATABASE", unique("absent")));
    let extra: Vec<(&str, &str)> = extra.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let output = run_backup(&backup_env(&bucket, &extra), None);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("mysqldump failed (exit"),
        "{}",
        stderr(&output)
    );
    assert!(keys(&client, &bucket).await.is_empty());
    assert_eq!(pending_uploads(&client, &bucket).await, 0);
}

/// A local HTTP server for `HEARTBEAT_URL`. It answers every request with
/// `status` and records each request line.
fn heartbeat_server(status: u16) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&requests);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).is_ok_and(|n| n == 1) {
                head.push(byte[0]);
            }
            let line = String::from_utf8_lossy(&head)
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned();
            seen.lock().unwrap().push(line);
            let _ = write!(
                stream,
                "HTTP/1.1 {status} Status\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
        }
    });
    (url, requests)
}

/// Backs up a seeded database (or a missing one) with `HEARTBEAT_URL` set.
async fn backup_with_heartbeat(url: &str, database_exists: bool) -> Output {
    let client = s3().await;
    let bucket = create_bucket(&client, "heartbeat").await;
    let db = unique("hb");
    if database_exists {
        seed(&db);
    }
    let mut extra: Vec<(&str, String)> = db_env();
    extra.push(("MYSQL_DATABASE", db.clone()));
    extra.push(("HEARTBEAT_URL", url.to_owned()));
    let extra: Vec<(&str, &str)> = extra.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let output = run_backup(&backup_env(&bucket, &extra), None);
    if database_exists {
        drop_database(&db);
    }
    output
}

#[tokio::test]
async fn heartbeat_follows_a_successful_backup() {
    if !configured("TEST_S3_ENDPOINT") || !configured("TEST_MYSQL_HOST") {
        return;
    }
    let (url, requests) = heartbeat_server(200);
    let output = backup_with_heartbeat(&format!("{url}/ping/secret-token?s=1"), true).await;
    assert!(output.status.success());
    assert_eq!(
        *requests.lock().unwrap(),
        ["GET /ping/secret-token?s=1 HTTP/1.1"]
    );
    assert!(stdout(&output).contains("Heartbeat sent."));
    // The URL usually carries the monitor's token: it must not be logged.
    assert!(!stdout(&output).contains("secret-token"));
    assert!(!stderr(&output).contains("secret-token"));
}

#[tokio::test]
async fn failed_backup_sends_no_heartbeat() {
    if !configured("TEST_S3_ENDPOINT") || !configured("TEST_MYSQL_HOST") {
        return;
    }
    let (url, requests) = heartbeat_server(200);
    let output = backup_with_heartbeat(&url, false).await;
    assert_eq!(output.status.code(), Some(1));
    assert!(requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn heartbeat_failure_does_not_fail_the_backup() {
    if !configured("TEST_S3_ENDPOINT") || !configured("TEST_MYSQL_HOST") {
        return;
    }
    let (url, requests) = heartbeat_server(503);
    let output = backup_with_heartbeat(&url, true).await;
    assert!(output.status.success());
    assert_eq!(requests.lock().unwrap().len(), 3);
    let err = stderr(&output);
    assert!(
        err.contains("Warning: heartbeat attempt 3 of 3 failed: HTTP 503"),
        "{err}"
    );
    assert!(
        err.contains("Warning: the heartbeat was not delivered; the backup itself succeeded"),
        "{err}"
    );
}

fn fake_env<'a>(extra: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    let mut env = vec![
        ("MYSQL_HOST", "127.0.0.1"),
        ("MYSQL_USER", "fake"),
        ("MYSQL_DATABASE", "fake"),
        ("S3_PART_SIZE_MIB", "5"),
    ];
    env.extend_from_slice(extra);
    env
}

#[tokio::test]
async fn large_dump_is_split_into_parts() {
    if !configured("TEST_S3_ENDPOINT") || image().is_some() {
        return;
    }
    let client = s3().await;
    let bucket = create_bucket(&client, "parts").await;
    let expected = std::env::temp_dir().join(unique("expected"));
    // Base64 of random bytes gzips to about 76%: roughly 15 MB, three parts.
    let fake = fake_mysqldump(&format!(
        "head -c 15000000 /dev/urandom | base64 | tee '{}'",
        expected.display()
    ));

    let output = run_backup(&backup_env(&bucket, &fake_env(&[])), Some(&fake));
    assert!(output.status.success());
    assert!(
        stdout(&output).contains("in 3 part(s)"),
        "{}",
        stdout(&output)
    );

    let keys = keys(&client, &bucket).await;
    assert_eq!(keys.len(), 1);
    let dump = download_gunzipped(&client, &bucket, &keys[0]).await;
    assert!(
        dump == std::fs::read(&expected).unwrap(),
        "downloaded dump differs from what mysqldump wrote"
    );
}

#[tokio::test]
async fn dump_failure_mid_stream_aborts_the_upload() {
    if !configured("TEST_S3_ENDPOINT") || image().is_some() {
        return;
    }
    let client = s3().await;
    let bucket = create_bucket(&client, "midfail").await;
    // Enough output for at least one part to be uploaded before the failure.
    let fake = fake_mysqldump(
        "head -c 15000000 /dev/urandom | base64\necho 'Got error: 2013: Lost connection' >&2\nexit 3",
    );

    let output = run_backup(&backup_env(&bucket, &fake_env(&[])), Some(&fake));
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("Error: dump of 'fake' failed: mysqldump failed (exit 3)"));
    assert!(keys(&client, &bucket).await.is_empty());
    assert_eq!(pending_uploads(&client, &bucket).await, 0);
}

#[tokio::test]
async fn stalled_dump_times_out() {
    if !configured("TEST_S3_ENDPOINT") || image().is_some() {
        return;
    }
    let client = s3().await;
    let bucket = create_bucket(&client, "stall").await;
    let fake = fake_mysqldump("echo 'CREATE TABLE t (id INT);'\nexec sleep 300");

    let started = Instant::now();
    let output = run_backup(
        &backup_env(
            &bucket,
            &fake_env(&[("DUMP_TIMEOUT", "2"), ("TIMEOUT_GRACE", "2")]),
        ),
        Some(&fake),
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "took {:?}",
        started.elapsed()
    );
    assert!(
        stderr(&output).contains("exceeded 2s (DUMP_TIMEOUT)"),
        "{}",
        stderr(&output)
    );
    assert!(keys(&client, &bucket).await.is_empty());
    assert_eq!(pending_uploads(&client, &bucket).await, 0);
}

#[tokio::test]
async fn sigterm_aborts_the_upload() {
    if !configured("TEST_S3_ENDPOINT") || image().is_some() {
        return;
    }
    let client = s3().await;
    let bucket = create_bucket(&client, "sigterm").await;
    let fake = fake_mysqldump("head -c 15000000 /dev/urandom | base64\nexec sleep 300");

    let child: Child = backup_command(&backup_env(&bucket, &fake_env(&[])), Some(&fake))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Wait for the upload to start, so there are parts to abort.
    let deadline = Instant::now() + Duration::from_secs(30);
    while pending_uploads(&client, &bucket).await == 0 {
        assert!(Instant::now() < deadline, "the upload never started");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    // SAFETY: kill(2) on the child we spawned and have not reaped.
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
    }
    let output = child.wait_with_output().unwrap();
    eprintln!("--- stderr\n{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(output.status.code(), Some(143));
    assert!(stderr(&output).contains("the incomplete upload was aborted"));
    assert!(keys(&client, &bucket).await.is_empty());
    assert_eq!(pending_uploads(&client, &bucket).await, 0);
}

#[tokio::test]
async fn upload_failure_is_retried_then_reported() {
    if !configured("TEST_S3_ENDPOINT") || image().is_some() {
        return;
    }
    let fake = fake_mysqldump("echo 'CREATE TABLE t (id INT);'");
    let output = run_backup(
        &backup_env(&unique("nobucket"), &fake_env(&[])),
        Some(&fake),
    );
    assert_eq!(output.status.code(), Some(1));
    let err = stderr(&output);
    assert!(err.contains("Upload attempt 1 failed"), "{err}");
    assert!(err.contains("Retrying with a fresh dump"), "{err}");
    assert!(
        err.contains("Error: S3 upload failed after 2 attempts"),
        "{err}"
    );
}

#[test]
fn invalid_configuration_exits_before_doing_anything() {
    if image().is_some() {
        return;
    }
    let output = Command::new(env!("CARGO_BIN_EXE_mysql-s3-backup"))
        .env_clear()
        .env("MYSQL_HOST", "db")
        .env("MYSQL_USER", "u")
        .env("MYSQL_DATABASE", "d")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "Error: Missing required environment variable S3_BUCKET\n"
    );
}
