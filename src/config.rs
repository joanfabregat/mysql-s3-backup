//! Configuration from the environment.
//!
//! The variable names, defaults and `DATABASE_URL` parsing match the v3 shell
//! script, so an existing deployment only changes its image tag.

use std::collections::HashMap;
use std::time::Duration;

use hyper::Uri;

use crate::heartbeat;

/// Whole-database attempts. Only a failed upload is retried; a failed or
/// timed-out dump is not, matching v3.
pub const MAX_ATTEMPTS: u32 = 2;

/// Deliberately not tunable: the engine check is a metadata lookup against
/// information_schema, not a data read. If it has not answered in 30s the
/// server is in no state to be dumped, and DUMP_TIMEOUT will say so a moment
/// later.
pub const ENGINE_CHECK_TIMEOUT: Duration = Duration::from_secs(30);

const MIB: usize = 1024 * 1024;

/// S3 rejects non-final parts under 5 MiB and any part over 5 GiB.
const MIN_PART_SIZE_MIB: usize = 5;
const MAX_PART_SIZE_MIB: usize = 5 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Server {
    Tcp { host: String, port: String },
    Socket(String),
}

#[derive(Debug, Clone)]
pub struct Config {
    pub databases: Vec<String>,
    pub user: String,
    pub password: Option<String>,
    pub server: Server,
    pub single_transaction: bool,
    pub bucket: String,
    pub prefix: String,
    pub endpoint_url: Option<String>,
    pub storage_class: Option<String>,
    pub part_size: usize,
    /// Wall-clock limit for one attempt at one database: dump and upload
    /// together, since they now run as one stream.
    pub dump_timeout: Duration,
    /// Limit for each S3 request attempt.
    pub upload_timeout: Duration,
    /// Time between TERM and KILL when stopping mysqldump.
    pub timeout_grace: Duration,
    /// Pinged once every database is uploaded.
    pub heartbeat_url: Option<Uri>,
}

struct Vars<'a>(&'a HashMap<String, String>);

impl Vars<'_> {
    /// The value if set and non-empty, like `${VAR:-}` in the shell.
    fn get(&self, name: &str) -> Option<&str> {
        self.0
            .get(name)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }

    /// The value if set, even when empty, like `${VAR-default}`.
    fn raw(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }
}

/// Connection fields as `DATABASE_URL` sets them. `None` leaves the matching
/// `MYSQL_*` variable in charge.
#[derive(Debug, Default, PartialEq, Eq)]
struct UrlParts {
    user: String,
    password: String,
    host: String,
    port: Option<String>,
    database: String,
    socket: Option<String>,
}

fn parse_database_url(url: &str) -> Result<UrlParts, String> {
    let rest = url
        .strip_prefix("mysql://")
        .ok_or("DATABASE_URL must use mysql:// scheme")?;
    let (userinfo, rest) = rest
        .split_once('@')
        .ok_or("DATABASE_URL must include a user: mysql://user[:password]@host[:port]/database")?;
    let (user, password) = userinfo.split_once(':').unwrap_or((userinfo, ""));
    let (hostport_path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (hostport, database) = hostport_path.split_once('/').unwrap_or((hostport_path, ""));
    let (host, port) = match hostport.split_once(':') {
        Some((host, port)) => (host, Some(port.to_owned())),
        None => (hostport, None),
    };
    let socket = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("unix_socket="))
        .map(str::to_owned);

    Ok(UrlParts {
        user: user.to_owned(),
        password: password.to_owned(),
        host: host.to_owned(),
        port,
        database: database.to_owned(),
        socket,
    })
}

fn parse_bool(name: &str, value: &str) -> Result<bool, String> {
    match value.to_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(format!(
            "{name} must be a boolean (1/0, true/false, yes/no, on/off), got '{value}'"
        )),
    }
}

fn parse_seconds(
    name: &str,
    value: Option<&str>,
    default: u64,
    allow_zero: bool,
) -> Result<Duration, String> {
    let Some(value) = value else {
        return Ok(Duration::from_secs(default));
    };
    match value.parse::<u64>() {
        Ok(0) if !allow_zero => Err(format!("{name} must be greater than 0")),
        Ok(secs) => Ok(Duration::from_secs(secs)),
        Err(_) => Err(format!(
            "{name} must be a whole number of seconds, got '{value}'"
        )),
    }
}

fn parse_part_size(value: Option<&str>) -> Result<usize, String> {
    let Some(value) = value else {
        return Ok(16 * MIB);
    };
    match value.parse::<usize>() {
        Ok(mib) if (MIN_PART_SIZE_MIB..=MAX_PART_SIZE_MIB).contains(&mib) => Ok(mib * MIB),
        _ => Err(format!(
            "S3_PART_SIZE_MIB must be a whole number from {MIN_PART_SIZE_MIB} to {MAX_PART_SIZE_MIB}, got '{value}'"
        )),
    }
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let vars = std::env::vars_os()
            .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
            .collect();
        Self::from_vars(&vars)
    }

    pub fn from_vars(vars: &HashMap<String, String>) -> Result<Self, String> {
        let vars = Vars(vars);

        let single_transaction = match vars.get("MYSQL_SINGLE_TRANSACTION") {
            Some(value) => parse_bool("MYSQL_SINGLE_TRANSACTION", value)?,
            None => false,
        };

        let mut user = vars.get("MYSQL_USER").map(str::to_owned);
        let mut password = vars.get("MYSQL_PASSWORD").map(str::to_owned);
        let mut host = vars.get("MYSQL_HOST").map(str::to_owned);
        let mut port = vars.get("MYSQL_PORT").unwrap_or("3306").to_owned();
        let mut database = vars.get("MYSQL_DATABASE").map(str::to_owned);
        let mut socket = vars.get("MYSQL_SOCKET").map(str::to_owned);

        if let Some(url) = vars.get("DATABASE_URL") {
            let parts = parse_database_url(url)?;
            let non_empty = |s: String| Some(s).filter(|s| !s.is_empty());
            user = non_empty(parts.user);
            password = non_empty(parts.password);
            host = non_empty(parts.host);
            database = non_empty(parts.database);
            if let Some(p) = parts.port {
                port = p;
            }
            if let Some(s) = parts.socket {
                socket = non_empty(s);
            }
        }

        let database = database.ok_or("Missing required environment variable MYSQL_DATABASE")?;
        let user = user.ok_or("Missing required environment variable MYSQL_USER")?;
        let server = match (socket, host) {
            (Some(socket), _) => Server::Socket(socket),
            (None, Some(host)) => Server::Tcp { host, port },
            (None, None) => {
                return Err(
                    "Missing required environment variable MYSQL_HOST or MYSQL_SOCKET".into(),
                );
            }
        };
        let bucket = vars
            .get("S3_BUCKET")
            .ok_or("Missing required environment variable S3_BUCKET")?
            .to_owned();

        let databases: Vec<String> = database
            .split(',')
            .map(str::trim)
            .filter(|db| !db.is_empty())
            .map(str::to_owned)
            .collect();
        if databases.is_empty() {
            return Err(format!(
                "MYSQL_DATABASE has no database names: '{database}'"
            ));
        }

        Ok(Config {
            databases,
            user,
            password,
            server,
            single_transaction,
            bucket,
            prefix: vars.get("S3_PREFIX").unwrap_or("/").to_owned(),
            endpoint_url: vars.get("S3_ENDPOINT_URL").map(str::to_owned),
            // Set-but-empty disables the storage class, for providers that
            // reject STANDARD_IA.
            storage_class: Some(vars.raw("S3_STORAGE_CLASS").unwrap_or("STANDARD_IA"))
                .filter(|c| !c.is_empty())
                .map(str::to_owned),
            part_size: parse_part_size(vars.get("S3_PART_SIZE_MIB"))?,
            dump_timeout: parse_seconds("DUMP_TIMEOUT", vars.get("DUMP_TIMEOUT"), 600, false)?,
            upload_timeout: parse_seconds(
                "UPLOAD_TIMEOUT",
                vars.get("UPLOAD_TIMEOUT"),
                300,
                false,
            )?,
            timeout_grace: parse_seconds("TIMEOUT_GRACE", vars.get("TIMEOUT_GRACE"), 30, true)?,
            heartbeat_url: vars
                .get("HEARTBEAT_URL")
                .map(heartbeat::parse_url)
                .transpose()?,
        })
    }

    /// The object key for `db`. Kept byte-for-byte compatible with v3, double
    /// slashes from a trailing-slash prefix included, so existing lifecycle
    /// rules and restore tooling keep matching.
    pub fn object_key(&self, db: &str, timestamp: &str) -> String {
        let prefix = self.prefix.strip_prefix('/').unwrap_or(&self.prefix);
        let key = if self.databases.len() > 1 {
            format!("{prefix}/{db}/{timestamp}.sql.gz")
        } else {
            format!("{prefix}/{timestamp}.sql.gz")
        };
        match key.strip_prefix('/') {
            Some(stripped) => stripped.to_owned(),
            None => key,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn base() -> Vec<(&'static str, &'static str)> {
        vec![
            ("MYSQL_HOST", "db"),
            ("MYSQL_USER", "backup"),
            ("MYSQL_PASSWORD", "secret"),
            ("MYSQL_DATABASE", "app"),
            ("S3_BUCKET", "bucket"),
        ]
    }

    fn with(extra: &[(&str, &str)]) -> HashMap<String, String> {
        let mut all = vars(&base());
        all.extend(vars(extra));
        all
    }

    #[test]
    fn defaults() {
        let cfg = Config::from_vars(&with(&[])).unwrap();
        assert_eq!(
            cfg.server,
            Server::Tcp {
                host: "db".into(),
                port: "3306".into()
            }
        );
        assert_eq!(cfg.password.as_deref(), Some("secret"));
        assert_eq!(cfg.databases, ["app"]);
        assert_eq!(cfg.prefix, "/");
        assert_eq!(cfg.storage_class.as_deref(), Some("STANDARD_IA"));
        assert_eq!(cfg.part_size, 16 * MIB);
        assert_eq!(cfg.dump_timeout, Duration::from_secs(600));
        assert_eq!(cfg.upload_timeout, Duration::from_secs(300));
        assert_eq!(cfg.timeout_grace, Duration::from_secs(30));
        assert!(!cfg.single_transaction);
        assert!(cfg.endpoint_url.is_none());
        assert!(cfg.heartbeat_url.is_none());
    }

    #[test]
    fn heartbeat_url() {
        let cfg = Config::from_vars(&with(&[("HEARTBEAT_URL", "https://hb.example/x")])).unwrap();
        assert_eq!(cfg.heartbeat_url.unwrap().host(), Some("hb.example"));
        let err = Config::from_vars(&with(&[("HEARTBEAT_URL", "hb.example/x")])).unwrap_err();
        assert!(err.starts_with("HEARTBEAT_URL must be"), "{err}");
    }

    #[test]
    fn empty_storage_class_disables_it() {
        let cfg = Config::from_vars(&with(&[("S3_STORAGE_CLASS", "")])).unwrap();
        assert_eq!(cfg.storage_class, None);
    }

    #[test]
    fn empty_variables_count_as_unset() {
        let cfg = Config::from_vars(&with(&[
            ("MYSQL_PORT", ""),
            ("S3_PREFIX", ""),
            ("MYSQL_PASSWORD", ""),
        ]))
        .unwrap();
        assert_eq!(
            cfg.server,
            Server::Tcp {
                host: "db".into(),
                port: "3306".into()
            }
        );
        assert_eq!(cfg.prefix, "/");
        assert_eq!(cfg.password, None);
    }

    #[test]
    fn socket_wins_over_host() {
        let cfg = Config::from_vars(&with(&[("MYSQL_SOCKET", "/run/mysqld.sock")])).unwrap();
        assert_eq!(cfg.server, Server::Socket("/run/mysqld.sock".into()));
    }

    #[test]
    fn single_transaction_values() {
        for value in ["1", "true", "YES", "On"] {
            let cfg = Config::from_vars(&with(&[("MYSQL_SINGLE_TRANSACTION", value)])).unwrap();
            assert!(cfg.single_transaction, "{value}");
        }
        for value in ["0", "false", "No", "OFF"] {
            let cfg = Config::from_vars(&with(&[("MYSQL_SINGLE_TRANSACTION", value)])).unwrap();
            assert!(!cfg.single_transaction, "{value}");
        }
        let err = Config::from_vars(&with(&[("MYSQL_SINGLE_TRANSACTION", "maybe")])).unwrap_err();
        assert_eq!(
            err,
            "MYSQL_SINGLE_TRANSACTION must be a boolean (1/0, true/false, yes/no, on/off), got 'maybe'"
        );
    }

    #[test]
    fn missing_variables() {
        let cases = [
            (
                "MYSQL_DATABASE",
                "Missing required environment variable MYSQL_DATABASE",
            ),
            (
                "MYSQL_USER",
                "Missing required environment variable MYSQL_USER",
            ),
            (
                "MYSQL_HOST",
                "Missing required environment variable MYSQL_HOST or MYSQL_SOCKET",
            ),
            (
                "S3_BUCKET",
                "Missing required environment variable S3_BUCKET",
            ),
        ];
        for (name, message) in cases {
            let mut v = with(&[]);
            v.remove(name);
            assert_eq!(Config::from_vars(&v).unwrap_err(), message);
        }
    }

    #[test]
    fn database_list_is_trimmed() {
        let cfg = Config::from_vars(&with(&[("MYSQL_DATABASE", " app , analytics ,,")])).unwrap();
        assert_eq!(cfg.databases, ["app", "analytics"]);
        let err = Config::from_vars(&with(&[("MYSQL_DATABASE", " , ")])).unwrap_err();
        assert_eq!(err, "MYSQL_DATABASE has no database names: ' , '");
    }

    #[test]
    fn timeouts_and_part_size() {
        let cfg = Config::from_vars(&with(&[
            ("DUMP_TIMEOUT", "1200"),
            ("UPLOAD_TIMEOUT", "60"),
            ("TIMEOUT_GRACE", "0"),
            ("S3_PART_SIZE_MIB", "64"),
        ]))
        .unwrap();
        assert_eq!(cfg.dump_timeout, Duration::from_secs(1200));
        assert_eq!(cfg.upload_timeout, Duration::from_secs(60));
        assert_eq!(cfg.timeout_grace, Duration::ZERO);
        assert_eq!(cfg.part_size, 64 * MIB);

        assert!(Config::from_vars(&with(&[("DUMP_TIMEOUT", "0")])).is_err());
        assert!(Config::from_vars(&with(&[("DUMP_TIMEOUT", "10m")])).is_err());
        assert!(Config::from_vars(&with(&[("S3_PART_SIZE_MIB", "4")])).is_err());
        assert!(Config::from_vars(&with(&[("S3_PART_SIZE_MIB", "5121")])).is_err());
    }

    #[test]
    fn database_url_overrides_individual_variables() {
        let cfg =
            Config::from_vars(&with(&[("DATABASE_URL", "mysql://u:p%40ss@h:3307/d1,d2")])).unwrap();
        assert_eq!(cfg.user, "u");
        // Not percent-decoded, as in v3.
        assert_eq!(cfg.password.as_deref(), Some("p%40ss"));
        assert_eq!(
            cfg.server,
            Server::Tcp {
                host: "h".into(),
                port: "3307".into()
            }
        );
        assert_eq!(cfg.databases, ["d1", "d2"]);
    }

    #[test]
    fn database_url_variants() {
        assert_eq!(
            parse_database_url("mysql://u@h/d").unwrap(),
            UrlParts {
                user: "u".into(),
                host: "h".into(),
                database: "d".into(),
                ..Default::default()
            }
        );
        // The password may contain colons: only the first one splits.
        assert_eq!(
            parse_database_url("mysql://u:a:b@h/d").unwrap().password,
            "a:b"
        );
        assert_eq!(
            parse_database_url("mysql://u:p@/d?charset=utf8&unix_socket=/s.sock").unwrap(),
            UrlParts {
                user: "u".into(),
                password: "p".into(),
                database: "d".into(),
                socket: Some("/s.sock".into()),
                ..Default::default()
            }
        );
        assert_eq!(
            parse_database_url("postgres://u@h/d").unwrap_err(),
            "DATABASE_URL must use mysql:// scheme"
        );
        assert!(parse_database_url("mysql://h/d").is_err());
    }

    #[test]
    fn database_url_keeps_port_and_socket_from_env_when_absent() {
        let cfg = Config::from_vars(&with(&[
            ("MYSQL_PORT", "3310"),
            ("DATABASE_URL", "mysql://u:p@h/d"),
        ]))
        .unwrap();
        assert_eq!(
            cfg.server,
            Server::Tcp {
                host: "h".into(),
                port: "3310".into()
            }
        );

        let cfg = Config::from_vars(&with(&[
            ("MYSQL_SOCKET", "/env.sock"),
            ("DATABASE_URL", "mysql://u:p@/d"),
        ]))
        .unwrap();
        assert_eq!(cfg.server, Server::Socket("/env.sock".into()));
    }

    #[test]
    fn object_keys_match_v3() {
        let ts = "2026-10-13T120000Z";
        let key = |prefix: &str, dbs: &str| {
            let cfg = Config::from_vars(&with(&[("S3_PREFIX", prefix), ("MYSQL_DATABASE", dbs)]))
                .unwrap();
            cfg.object_key(&cfg.databases[0], ts)
        };
        assert_eq!(key("/", "app"), "2026-10-13T120000Z.sql.gz");
        assert_eq!(
            key("backups/mysql", "app"),
            "backups/mysql/2026-10-13T120000Z.sql.gz"
        );
        assert_eq!(key("/backups", "app"), "backups/2026-10-13T120000Z.sql.gz");
        assert_eq!(key("backups/", "app"), "backups//2026-10-13T120000Z.sql.gz");
        assert_eq!(key("/", "app,other"), "app/2026-10-13T120000Z.sql.gz");
        assert_eq!(
            key("daily", "app,other"),
            "daily/app/2026-10-13T120000Z.sql.gz"
        );
    }
}
