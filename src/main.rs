//! Dumps MySQL/MariaDB databases with mysqldump and streams them, gzipped, to
//! S3 or an S3-compatible provider. Configured entirely from the environment;
//! see the README.

mod config;
mod dump;
mod s3;

use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_smithy_types::DateTime;
use aws_smithy_types::date_time::Format;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::watch;

use crate::config::{Config, MAX_ATTEMPTS};
use crate::dump::{Failure, Summary, backup_once, warn_non_transactional_tables};
use crate::s3::Bucket;

const RETRY_DELAY: Duration = Duration::from_secs(5);

/// `2026-10-13T120000Z`: RFC 3339 in UTC to the second, without colons so it
/// is safe in object keys and sorts lexically.
fn timestamp(now: SystemTime) -> String {
    let secs = now
        .duration_since(UNIX_EPOCH)
        .expect("clock is after 1970")
        .as_secs();
    DateTime::from_secs(secs as i64)
        .fmt(Format::DateTime)
        .expect("a current time formats")
        .replace(':', "")
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn report(db: &str, summary: &Summary) {
    println!(
        "Uploaded successfully: {:.1} MiB compressed ({:.1} MiB raw) in {} part(s), {:.1}s.",
        mib(summary.compressed_bytes),
        mib(summary.raw_bytes),
        summary.parts,
        summary.elapsed.as_secs_f64(),
    );
    println!(
        "mysqldump of '{db}' waited {:.1}s on the upload.",
        summary.upload_wait.as_secs_f64()
    );
}

/// Forwards the first TERM or INT into `tx`, so the running attempt can stop
/// mysqldump and abort its upload instead of dying with parts left behind.
fn watch_signals(tx: watch::Sender<Option<i32>>) {
    let mut term = signal(SignalKind::terminate()).expect("can install a TERM handler");
    let mut int = signal(SignalKind::interrupt()).expect("can install an INT handler");
    tokio::spawn(async move {
        let signal = tokio::select! {
            _ = term.recv() => libc::SIGTERM,
            _ = int.recv() => libc::SIGINT,
        };
        eprintln!("Received signal {signal}, stopping");
        let _ = tx.send(Some(signal));
    });
}

/// Backs up one database, retrying a failed upload with a fresh dump.
async fn backup_database(
    cfg: &Config,
    bucket: &Bucket,
    db: &str,
    key: &str,
    shutdown: &mut watch::Receiver<Option<i32>>,
) -> Result<(), ExitCode> {
    let mut attempt = 1;
    loop {
        match backup_once(cfg, bucket, db, key, shutdown).await {
            Ok(summary) => {
                report(db, &summary);
                return Ok(());
            }
            Err(Failure::Upload(e)) if attempt < MAX_ATTEMPTS => {
                eprintln!("Upload attempt {attempt} failed: {e:#}");
                eprintln!(
                    "Retrying with a fresh dump in {}s...",
                    RETRY_DELAY.as_secs()
                );
                attempt += 1;
                tokio::select! {
                    _ = tokio::time::sleep(RETRY_DELAY) => {}
                    _ = shutdown.changed() => {}
                }
            }
            Err(Failure::Upload(e)) => {
                eprintln!("Upload attempt {attempt} failed: {e:#}");
                eprintln!("Error: S3 upload failed after {MAX_ATTEMPTS} attempts");
                return Err(ExitCode::FAILURE);
            }
            Err(Failure::TimedOut) => {
                eprintln!(
                    "Error: dump and upload of '{db}' exceeded {}s (DUMP_TIMEOUT) and were stopped",
                    cfg.dump_timeout.as_secs()
                );
                return Err(ExitCode::FAILURE);
            }
            Err(Failure::Dump(message)) => {
                eprintln!("Error: dump of '{db}' failed: {message}");
                return Err(ExitCode::FAILURE);
            }
            Err(Failure::Interrupted(signal)) => {
                eprintln!(
                    "Error: backup of '{db}' was interrupted; the incomplete upload was aborted"
                );
                return Err(ExitCode::from(128 + signal as u8));
            }
        }
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> ExitCode {
    let cfg = match Config::from_env() {
        Ok(cfg) => cfg,
        Err(message) => {
            eprintln!("Error: {message}");
            return ExitCode::FAILURE;
        }
    };
    println!("mysql-s3-backup {}", env!("CARGO_PKG_VERSION"));

    let (tx, mut shutdown) = watch::channel(None);
    watch_signals(tx);

    let bucket = Bucket::new(&cfg).await;
    let timestamp = timestamp(SystemTime::now());

    for db in &cfg.databases {
        println!("Dumping MySQL database: {db}");
        if cfg.single_transaction {
            warn_non_transactional_tables(&cfg, db).await;
        }

        let key = cfg.object_key(db, &timestamp);
        println!("Streaming dump to S3: s3://{}/{key}", cfg.bucket);
        if let Err(code) = backup_database(&cfg, &bucket, db, &key, &mut shutdown).await {
            return code;
        }
    }

    println!("Backup complete.");
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_format() {
        let t = UNIX_EPOCH + Duration::from_millis(1_791_892_800_250);
        assert_eq!(timestamp(t), "2026-10-13T120000Z");
    }
}
