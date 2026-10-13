//! Running mysqldump and streaming its output to S3.

use std::fmt;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use flate2::Compression;
use flate2::write::GzEncoder;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, ChildStdout, Command};
use tokio::sync::{Semaphore, watch};
use tokio::task::JoinSet;
use tokio::time::timeout;

use crate::config::{Config, ENGINE_CHECK_TIMEOUT, Server};
use crate::s3::{Bucket, Upload};

/// Parts uploading at once. With the one being filled, memory holds at most
/// three parts: 48 MiB at the default 16 MiB part size.
const PARALLEL_PARTS: usize = 2;

const READ_CHUNK: usize = 64 * 1024;

/// Room for the compressed output of the read that pushes a part past its
/// size, so the buffer never reallocates to double its capacity.
const PART_SLACK: usize = 256 * 1024;

/// Finds the first of `names` on PATH. Alpine's mariadb-client installs
/// `mariadb` and `mariadb-dump` and has so far also shipped `mysql` and
/// `mysqldump` compatibility symlinks; resolving whichever exists avoids
/// betting on those symlinks outliving a package update.
pub fn find_program(names: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    names.iter().find_map(|name| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(name))
            .find(|candidate| {
                candidate
                    .metadata()
                    .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            })
    })
}

/// Shared by mysqldump and by the engine check, so the check always talks to
/// the same server, as the same user, as the dump it is warning about.
fn connection_args(cfg: &Config) -> Vec<String> {
    let mut args = vec!["-u".to_owned(), cfg.user.clone()];
    match &cfg.server {
        Server::Socket(socket) => args.extend(["--socket".to_owned(), socket.clone()]),
        Server::Tcp { host, port } => {
            args.extend(["-h".to_owned(), host.clone(), "-P".to_owned(), port.clone()])
        }
    }
    args
}

/// A client command with the connection arguments. The password goes through
/// MYSQL_PWD rather than `--password=` so it does not show up in `ps`.
fn client_command(program: PathBuf, cfg: &Config) -> Command {
    let mut command = Command::new(program);
    command
        .args(connection_args(cfg))
        .stdin(Stdio::null())
        .kill_on_drop(true);
    if let Some(password) = &cfg.password {
        command.env("MYSQL_PWD", password);
    }
    command
}

/// Names the tables in `db` that --single-transaction cannot snapshot
/// consistently.
///
/// Advisory only: every failure path here warns and returns. A backup must not
/// be abandoned because a diagnostic query could not run -- that would turn a
/// missing warning into a missing backup, which is strictly worse than the
/// risk it warns about.
pub async fn warn_non_transactional_tables(cfg: &Config, db: &str) {
    let Some(client) = find_program(&["mariadb", "mysql"]) else {
        eprintln!(
            "Warning: no MySQL client found, skipping the non-transactional table check for '{db}'"
        );
        return;
    };

    // The database name is passed as an argument and read back with
    // DATABASE() rather than interpolated into the SQL, so a name containing
    // a quote cannot break out of the statement.
    let mut command = client_command(client, cfg);
    command
        .args(["--batch", "--skip-column-names", db, "-e"])
        .arg(
            "SELECT CONCAT(TABLE_NAME, ' (', IFNULL(ENGINE, 'unknown engine'), ')') \
             FROM information_schema.TABLES \
             WHERE TABLE_SCHEMA = DATABASE() \
               AND TABLE_TYPE = 'BASE TABLE' \
               AND (ENGINE IS NULL OR ENGINE <> 'InnoDB') \
             ORDER BY TABLE_NAME",
        )
        .stderr(Stdio::inherit());

    let output = match timeout(ENGINE_CHECK_TIMEOUT, command.output()).await {
        Ok(Ok(output)) if output.status.success() => output,
        _ => {
            eprintln!("Warning: could not check '{db}' for non-transactional tables, continuing");
            return;
        }
    };

    let tables = String::from_utf8_lossy(&output.stdout);
    let tables: Vec<&str> = tables.lines().filter(|l| !l.is_empty()).collect();
    if tables.is_empty() {
        return;
    }
    eprintln!(
        "Warning: MYSQL_SINGLE_TRANSACTION is enabled but '{db}' has non-transactional tables:"
    );
    for table in tables {
        eprintln!("Warning:   {table}");
    }
    eprintln!(
        "Warning: these are dumped outside the transaction and a concurrent write can tear them."
    );
    eprintln!(
        "Warning: convert them to InnoDB, or unset MYSQL_SINGLE_TRANSACTION to lock instead."
    );
}

pub struct Summary {
    pub parts: i32,
    pub raw_bytes: u64,
    pub compressed_bytes: u64,
    pub elapsed: Duration,
    /// Time mysqldump spent blocked because the upload had not kept up. While
    /// it is blocked, its table locks (or transaction) stay open.
    pub upload_wait: Duration,
}

#[derive(Debug)]
pub enum Failure {
    /// mysqldump could not run or did not exit 0. Not retried.
    Dump(String),
    /// S3 rejected or dropped a request. Retried with a fresh dump.
    Upload(anyhow::Error),
    /// The attempt ran past DUMP_TIMEOUT. Not retried.
    TimedOut,
    /// A TERM or INT arrived; carries the signal number.
    Interrupted(i32),
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::Dump(message) => f.write_str(message),
            Failure::Upload(e) => write!(f, "{e:#}"),
            Failure::TimedOut => f.write_str("timed out"),
            Failure::Interrupted(signal) => write!(f, "interrupted by signal {signal}"),
        }
    }
}

fn describe_exit(status: ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(code), _) => format!("exit {code}"),
        (None, Some(signal)) => format!("killed by signal {signal}"),
        (None, None) => status.to_string(),
    }
}

/// Sends TERM, then KILL if mysqldump has not exited after `grace`.
async fn stop(child: &mut Child, grace: Duration) {
    if let Some(pid) = child.id() {
        // SAFETY: kill(2) with a PID we spawned and have not yet reaped, so it
        // cannot have been recycled for another process.
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
        if timeout(grace, child.wait()).await.is_ok() {
            return;
        }
    }
    let _ = child.kill().await;
}

/// One attempt at one database: dump, compress and upload as a single stream,
/// bounded by DUMP_TIMEOUT. On any failure the multipart upload is aborted.
pub async fn backup_once(
    cfg: &Config,
    bucket: &Bucket,
    db: &str,
    key: &str,
    shutdown: &mut watch::Receiver<Option<i32>>,
) -> Result<Summary, Failure> {
    if let Some(signal) = *shutdown.borrow() {
        return Err(Failure::Interrupted(signal));
    }

    let program = find_program(&["mariadb-dump", "mysqldump"])
        .ok_or_else(|| Failure::Dump("neither mariadb-dump nor mysqldump is on PATH".into()))?;
    let upload = bucket.start_upload(key).await.map_err(Failure::Upload)?;

    let mut command = client_command(program, cfg);
    command.arg("--no-tablespaces");
    if cfg.single_transaction {
        // --single-transaction turns --lock-tables off by itself, so there is
        // nothing else to unset here.
        command.arg("--single-transaction");
    }
    command
        .arg(db)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            upload.abort().await;
            return Err(Failure::Dump(format!("could not start mysqldump: {e}")));
        }
    };
    let stdout = child.stdout.take().expect("stdout is piped");

    let started = Instant::now();
    let outcome = tokio::select! {
        result = timeout(cfg.dump_timeout, stream(stdout, &mut child, &upload, cfg.part_size)) => {
            result.unwrap_or(Err(Failure::TimedOut))
        }
        signal = wait_for_signal(shutdown) => Err(Failure::Interrupted(signal)),
    };

    match outcome {
        Ok(mut summary) => {
            summary.elapsed = started.elapsed();
            Ok(summary)
        }
        Err(failure) => {
            stop(&mut child, cfg.timeout_grace).await;
            upload.abort().await;
            Err(failure)
        }
    }
}

async fn wait_for_signal(shutdown: &mut watch::Receiver<Option<i32>>) -> i32 {
    match shutdown.wait_for(Option::is_some).await {
        Ok(signal) => signal.expect("waited for Some"),
        // The sender lives for the whole process; if it is gone, never fire.
        Err(_) => std::future::pending().await,
    }
}

async fn stream(
    mut stdout: ChildStdout,
    child: &mut Child,
    upload: &Upload,
    part_size: usize,
) -> Result<Summary, Failure> {
    let permits = Arc::new(Semaphore::new(PARALLEL_PARTS));
    let mut in_flight = JoinSet::new();
    let mut completed = Vec::new();
    let mut encoder = GzEncoder::new(
        Vec::with_capacity(part_size + PART_SLACK),
        Compression::default(),
    );
    let mut buf = vec![0u8; READ_CHUNK];
    let mut summary = Summary {
        parts: 0,
        raw_bytes: 0,
        compressed_bytes: 0,
        elapsed: Duration::ZERO,
        upload_wait: Duration::ZERO,
    };

    // Waits for an upload slot, then sends `body` as the next part. Waiting
    // here stops reading, which blocks mysqldump on the pipe: memory stays
    // bounded and the time is recorded as upload_wait.
    macro_rules! submit {
        ($body:expr) => {{
            let body: Vec<u8> = $body;
            let waiting = Instant::now();
            let permit = permits
                .clone()
                .acquire_owned()
                .await
                .expect("semaphore is never closed");
            summary.upload_wait += waiting.elapsed();
            summary.parts += 1;
            summary.compressed_bytes += body.len() as u64;
            let number = summary.parts;
            let upload = upload.clone();
            in_flight.spawn(async move {
                let _permit = permit;
                upload.upload_part(number, body).await
            });
        }};
    }

    loop {
        let n = stdout
            .read(&mut buf)
            .await
            .map_err(|e| Failure::Dump(format!("could not read mysqldump output: {e}")))?;
        if n == 0 {
            break;
        }
        summary.raw_bytes += n as u64;
        encoder
            .write_all(&buf[..n])
            .expect("writing to a Vec cannot fail");

        if encoder.get_ref().len() >= part_size {
            let body = std::mem::replace(
                encoder.get_mut(),
                Vec::with_capacity(part_size + PART_SLACK),
            );
            submit!(body);
        }

        // Surface an upload failure now rather than after the whole dump.
        while let Some(joined) = in_flight.try_join_next() {
            completed.push(
                joined
                    .expect("upload tasks do not panic")
                    .map_err(Failure::Upload)?,
            );
        }
    }

    let status = child
        .wait()
        .await
        .map_err(|e| Failure::Dump(format!("could not wait for mysqldump: {e}")))?;
    if !status.success() {
        return Err(Failure::Dump(format!(
            "mysqldump failed ({})",
            describe_exit(status)
        )));
    }

    // Only now is the dump known to be complete. The last part may be under
    // the 5 MiB minimum; S3 allows that for the final part.
    let last = encoder.finish().expect("writing to a Vec cannot fail");
    submit!(last);

    while let Some(joined) = in_flight.join_next().await {
        completed.push(
            joined
                .expect("upload tasks do not panic")
                .map_err(Failure::Upload)?,
        );
    }
    upload.complete(completed).await.map_err(Failure::Upload)?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn config(extra: &[(&str, &str)]) -> Config {
        let mut vars: HashMap<String, String> = [
            ("MYSQL_USER", "backup"),
            ("MYSQL_HOST", "db"),
            ("MYSQL_DATABASE", "app"),
            ("S3_BUCKET", "bucket"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        vars.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        Config::from_vars(&vars).unwrap()
    }

    #[test]
    fn tcp_connection_args() {
        assert_eq!(
            connection_args(&config(&[])),
            ["-u", "backup", "-h", "db", "-P", "3306"]
        );
    }

    #[test]
    fn socket_connection_args() {
        let cfg = config(&[("MYSQL_SOCKET", "/run/mysqld/mysqld.sock")]);
        assert_eq!(
            connection_args(&cfg),
            ["-u", "backup", "--socket", "/run/mysqld/mysqld.sock"]
        );
    }

    #[test]
    fn password_is_not_an_argument() {
        let cfg = config(&[("MYSQL_PASSWORD", "hunter2")]);
        assert!(!connection_args(&cfg).iter().any(|a| a.contains("hunter2")));
        let command = client_command(PathBuf::from("/bin/true"), &cfg);
        let pwd = command
            .as_std()
            .get_envs()
            .find(|(k, _)| *k == "MYSQL_PWD")
            .and_then(|(_, v)| v);
        assert_eq!(pwd, Some(std::ffi::OsStr::new("hunter2")));
    }

    #[test]
    fn exit_descriptions() {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(describe_exit(ExitStatus::from_raw(2 << 8)), "exit 2");
        assert_eq!(describe_exit(ExitStatus::from_raw(9)), "killed by signal 9");
    }
}
