use serde::Deserialize;
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::time::Duration;

/// Top-level configuration for Harux.
///
/// Load priority: `harux.toml` (next to binary, then CWD) → env vars → built-in defaults.
/// Unknown keys are an error (`deny_unknown_fields` on every section), so a
/// typo such as `max_mesages` stops the start instead of silently leaving
/// the default in place (#233).
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub server: ServerConfig,
    pub logging: LoggingConfig,
    pub store: StoreConfig,
    pub mllp: MllpConfig,
    /// Sending from the web UI and the API (Milestone 4). Off by default.
    pub send: SendConfig,
    /// The file the configuration was read from, if any — logged at startup.
    #[serde(skip)]
    pub source: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    pub mllp_port: u16,
    pub web_port: u16,
    /// Address the MLLP listener binds to (#237). `0.0.0.0` = every IPv4
    /// interface, `::` = every interface incl. IPv6, `127.0.0.1` = this
    /// machine only.
    pub mllp_bind: IpAddr,
    /// Address the web UI binds to (#237). `127.0.0.1` keeps it on this
    /// machine, e.g. behind a reverse proxy that adds TLS and login.
    pub web_bind: IpAddr,
    pub shutdown_timeout_secs: u64,
    /// Host names (without port) the web UI may be reached under, in
    /// addition to IP literals and `localhost`, which are always allowed.
    /// Requests with any other `Host` are refused, which blocks DNS
    /// rebinding (#227). Add the DNS name users type into the browser.
    pub allowed_hosts: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoggingConfig {
    pub level: String,
    pub file: Option<String>,
    /// How many daily log files (`<file>.YYYY-MM-DD`, today's included) to
    /// keep; older ones are deleted. `0` keeps all (#240).
    pub max_files: usize,
    /// Set at startup when `file` could not be opened and Harux logs to
    /// stdout only (#234), so the effective configuration says so.
    #[serde(skip)]
    pub file_unusable: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StoreConfig {
    pub max_messages: usize,
    pub max_memory_mb: usize,
    /// Maximum share (in percent) of `max_messages` / `max_memory_mb` that
    /// bookmarked messages may occupy while the store is under eviction
    /// pressure. Bookmarks beyond this budget are evicted oldest-first so that
    /// bookmarking can never lift the memory cap. Clamped to
    /// [`MAX_BOOKMARK_BUDGET_PERCENT`].
    pub bookmark_budget_percent: usize,
}

/// Upper bound for `store.bookmark_budget_percent`. Must stay below the
/// eviction low-watermark (90 %) so an eviction pass can always reach its
/// target by evicting non-bookmarked messages alone.
pub const MAX_BOOKMARK_BUDGET_PERCENT: usize = 80;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MllpConfig {
    pub max_message_size_mb: usize,
    pub read_timeout_secs: u64,
    pub write_timeout_secs: u64,
    pub max_connections: usize,
}

/// `[send]`: sending stored messages to other MLLP receivers (#272).
///
/// Off unless `enabled` is set. Targets are not configured here — every user
/// keeps a list of their own in the browser and the request names the
/// destination; this section only switches sending on and narrows it.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SendConfig {
    /// With it on, everyone who can open the web UI can make Harux connect
    /// to any destination it reaches (or to the ones listed in
    /// `allowed_destinations`) and send an HL7 message there.
    pub enabled: bool,
    /// Offer Harux's own MLLP listener as a target ("This Harux").
    pub loopback_target: bool,
    pub connect_timeout_secs: u64,
    pub response_timeout_secs: u64,
    /// Limits of one run (several messages in one request).
    pub max_run_messages: usize,
    /// Share of `store.max_memory_mb` the messages of one run may take
    /// together: every one of them is stored again as a sent message.
    pub max_run_store_percent: usize,
    pub max_concurrent_runs: usize,
    /// Where users may send. Empty = any host and port. Entries are
    /// `host:port` or `host:from-to` (`[v6]:port` for IPv6 addresses).
    pub allowed_destinations: Vec<String>,
    /// `[send.tls]`: TLS for outgoing connections (#273).
    pub tls: SendTlsConfig,
}

/// `[send.tls]` (#273). Read at start when sending is on; a file that can't
/// be read, holds no certificate or a key that doesn't belong to its
/// certificate stops the start. Relative paths are relative to the working
/// directory, like `logging.file`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SendTlsConfig {
    /// PEM file with CA certificates trusted in addition to the operating
    /// system's store, e.g. the hospital's own CA.
    pub ca_file: Option<String>,
    /// Let users switch the certificate check off for a target of theirs:
    /// encrypted, but not verified.
    pub allow_unverified: bool,
    /// Harux's own certificate (PEM, chain allowed), shown to a target that
    /// asks the sender for one. One for the whole server, never per user.
    pub client_cert_file: Option<String>,
    /// The private key of `client_cert_file` (PEM). Never logged or returned.
    pub client_key_file: Option<String>,
}

/// One `send.allowed_destinations` entry: a host (normalized by
/// [`normalize_host`]) and a port range. Nothing is resolved for the check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DestinationRule {
    pub host: String,
    pub ports: std::ops::RangeInclusive<u16>,
}

impl DestinationRule {
    /// Parse `host:port` or `host:from-to`. A bare host is refused: it would
    /// allow every port on that machine.
    pub fn parse(entry: &str) -> Result<Self, String> {
        let entry = entry.trim();
        let (host, ports) = if let Some(rest) = entry.strip_prefix('[') {
            let (host, after) = rest
                .split_once(']')
                .ok_or("an IPv6 address in [ ] needs the closing ]")?;
            let ports = after
                .strip_prefix(':')
                .ok_or("names no port; write host:port or host:from-to")?;
            (host, ports)
        } else {
            let (host, ports) = entry
                .rsplit_once(':')
                .ok_or("names no port; write host:port or host:from-to")?;
            if host.contains(':') {
                return Err("write an IPv6 address in brackets, e.g. [::1]:2575".into());
            }
            (host, ports)
        };
        let host = normalize_host(host).ok_or("is not a host name or IP address")?;
        let port = |p: &str| {
            p.trim()
                .parse::<u16>()
                .ok()
                .filter(|&p| p > 0)
                .ok_or_else(|| format!("{p:?} is not a port (1-65535)"))
        };
        let ports = match ports.split_once('-') {
            Some((from, to)) => {
                let (from, to) = (port(from)?, port(to)?);
                if from > to {
                    return Err(format!("the port range {from}-{to} is reversed"));
                }
                from..=to
            }
            None => {
                let p = port(ports)?;
                p..=p
            }
        };
        Ok(Self { host, ports })
    }

    /// Whether `host` (already normalized) and `port` fall under this rule.
    pub fn allows(&self, host: &str, port: u16) -> bool {
        self.host == host && self.ports.contains(&port)
    }
}

impl std::fmt::Display for DestinationRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        if self.ports.start() == self.ports.end() {
            write!(f, "{host}:{}", self.ports.start())
        } else {
            write!(f, "{host}:{}-{}", self.ports.start(), self.ports.end())
        }
    }
}

/// The one form a host is compared in (#272): an IP address in canonical
/// form (`::ffff:10.0.0.1` is `10.0.0.1`, brackets removed), a name in
/// lower case without its trailing dot. `None` for anything that is neither
/// — including names whose last label is all digits (`2130706433`,
/// `10.0.0.01`), which a resolver may read as an address.
pub fn normalize_host(host: &str) -> Option<String> {
    let h = host.trim();
    let h = h
        .strip_prefix('[')
        .and_then(|r| r.strip_suffix(']'))
        .unwrap_or(h);
    if let Ok(ip) = h.parse::<IpAddr>() {
        return Some(ip.to_canonical().to_string());
    }
    let name = h.strip_suffix('.').unwrap_or(h);
    let labels_ok = !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && l.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                && !l.starts_with('-')
                && !l.ends_with('-')
        });
    let numeric_tld = name
        .rsplit('.')
        .next()
        .is_some_and(|l| l.bytes().all(|b| b.is_ascii_digit()));
    (labels_ok && !numeric_tld).then(|| name.to_ascii_lowercase())
}

// --- Defaults ---

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            mllp_port: 2575,
            web_port: 8080,
            mllp_bind: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            web_bind: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            shutdown_timeout_secs: 10,
            allowed_hosts: Vec::new(),
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            file: None,
            max_files: 14,
            file_unusable: false,
        }
    }
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            max_messages: 10_000,
            max_memory_mb: 512,
            bookmark_budget_percent: 50,
        }
    }
}

impl Default for MllpConfig {
    fn default() -> Self {
        Self {
            max_message_size_mb: 10,
            read_timeout_secs: 60,
            write_timeout_secs: 30,
            max_connections: 100,
        }
    }
}

impl Default for SendConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            loopback_target: true,
            connect_timeout_secs: 5,
            response_timeout_secs: 30,
            max_run_messages: 1000,
            max_run_store_percent: 25,
            max_concurrent_runs: 4,
            allowed_destinations: Vec::new(),
            tls: SendTlsConfig::default(),
        }
    }
}

// --- Convenience methods ---

impl StoreConfig {
    pub fn max_memory_bytes(&self) -> usize {
        self.max_memory_mb * 1024 * 1024
    }

    /// Effective bookmark budget in percent, clamped to
    /// [`MAX_BOOKMARK_BUDGET_PERCENT`].
    pub fn bookmark_budget_percent(&self) -> usize {
        self.bookmark_budget_percent
            .min(MAX_BOOKMARK_BUDGET_PERCENT)
    }
}

impl ServerConfig {
    pub fn mllp_addr(&self) -> std::net::SocketAddr {
        std::net::SocketAddr::new(self.mllp_bind, self.mllp_port)
    }

    pub fn web_addr(&self) -> std::net::SocketAddr {
        std::net::SocketAddr::new(self.web_bind, self.web_port)
    }
}

impl MllpConfig {
    pub fn max_message_size(&self) -> usize {
        self.max_message_size_mb * 1024 * 1024
    }

    pub fn read_timeout(&self) -> Duration {
        Duration::from_secs(self.read_timeout_secs)
    }

    pub fn write_timeout(&self) -> Duration {
        Duration::from_secs(self.write_timeout_secs)
    }
}

impl SendConfig {
    /// The parsed `allowed_destinations` (validated at load, so the first
    /// bad entry is an error only for a configuration built by hand).
    pub fn destination_rules(&self) -> Result<Vec<DestinationRule>, String> {
        self.allowed_destinations
            .iter()
            .map(|e| {
                DestinationRule::parse(e)
                    .map_err(|why| format!("send.allowed_destinations entry {e:?} {why}"))
            })
            .collect()
    }
}

impl Config {
    /// The loopback target (#272): Harux's own MLLP listener. A listener on
    /// every interface is reached on `127.0.0.1` (`0.0.0.0`) or `::1` (`::`).
    /// `None` when `send.loopback_target` is off.
    pub fn loopback_destination(&self) -> Option<std::net::SocketAddr> {
        if !self.send.loopback_target {
            return None;
        }
        let ip = match self.server.mllp_bind {
            IpAddr::V4(v4) if v4.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(v6) if v6.is_unspecified() => IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
            ip => ip,
        };
        Some(std::net::SocketAddr::new(ip, self.server.mllp_port))
    }
}

// --- Loading ---

impl Config {
    /// Load configuration: built-in defaults, overridden by `harux.toml`,
    /// overridden by environment variables (`MLLP_PORT`, `WEB_PORT`,
    /// `RUST_LOG`).
    ///
    /// Fails closed (#233): a config file that can't be read or parsed, an
    /// unknown key, an invalid `MLLP_PORT` / `WEB_PORT`, or a value out of
    /// range is an error naming the file, key and reason — the caller stops
    /// the start. Running on other ports or limits than configured is worse
    /// than not running: the feed goes nowhere, and under a service manager
    /// nobody reads a warning on stderr.
    pub fn load() -> Result<Self, String> {
        let file = match find_config_path() {
            Some(path) => {
                let contents = std::fs::read_to_string(&path)
                    .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
                Some((path, contents))
            }
            None => None,
        };
        Self::from_sources(file, |key| std::env::var(key).ok())
    }

    /// [`Config::load`] without touching the file system or the process
    /// environment — `file` is `(path, contents)`, `env` looks up variables.
    pub fn from_sources(
        file: Option<(PathBuf, String)>,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, String> {
        let mut config = match file {
            Some((path, contents)) => {
                let mut cfg: Config = toml::from_str(&contents)
                    .map_err(|e| format!("invalid configuration in {}: {e}", path.display()))?;
                cfg.source = Some(path);
                cfg
            }
            None => Config::default(),
        };

        // Env-var overrides (highest priority)
        for (key, target) in [
            ("MLLP_PORT", &mut config.server.mllp_port),
            ("WEB_PORT", &mut config.server.web_port),
        ] {
            if let Some(val) = env(key) {
                *target = val.trim().parse::<u16>().map_err(|_| {
                    format!("environment variable {key}={val:?} is not a port number (1-65535)")
                })?;
            }
        }
        if let Some(val) = env("RUST_LOG") {
            config.logging.level = val;
        }

        config.validate()?;
        Ok(config)
    }

    /// Range checks for values that parse but can't work. All problems are
    /// reported at once.
    fn validate(&self) -> Result<(), String> {
        let mut problems = Vec::new();
        let mut at_least_1 = |name: &str, value: u64| {
            if value == 0 {
                problems.push(format!("{name} must be at least 1"));
            }
        };
        at_least_1("server.mllp_port", self.server.mllp_port.into());
        at_least_1("server.web_port", self.server.web_port.into());
        at_least_1("store.max_messages", self.store.max_messages as u64);
        at_least_1("store.max_memory_mb", self.store.max_memory_mb as u64);
        at_least_1(
            "mllp.max_message_size_mb",
            self.mllp.max_message_size_mb as u64,
        );
        at_least_1("mllp.read_timeout_secs", self.mllp.read_timeout_secs);
        at_least_1("mllp.write_timeout_secs", self.mllp.write_timeout_secs);
        at_least_1("mllp.max_connections", self.mllp.max_connections as u64);
        at_least_1("send.connect_timeout_secs", self.send.connect_timeout_secs);
        at_least_1(
            "send.response_timeout_secs",
            self.send.response_timeout_secs,
        );
        at_least_1("send.max_run_messages", self.send.max_run_messages as u64);
        at_least_1(
            "send.max_concurrent_runs",
            self.send.max_concurrent_runs as u64,
        );
        if !(1..=100).contains(&self.send.max_run_store_percent) {
            problems.push(format!(
                "send.max_run_store_percent ({}) must be between 1 and 100",
                self.send.max_run_store_percent
            ));
        }
        let tls = &self.send.tls;
        let set = |v: &Option<String>| v.as_deref().is_some_and(|f| !f.trim().is_empty());
        if set(&tls.client_cert_file) != set(&tls.client_key_file) {
            problems.push(
                "send.tls.client_cert_file and send.tls.client_key_file must be set together"
                    .to_string(),
            );
        }
        for entry in &self.send.allowed_destinations {
            if let Err(why) = DestinationRule::parse(entry) {
                problems.push(format!("send.allowed_destinations entry {entry:?} {why}"));
            }
        }
        // Two listeners can share a port only on different addresses; an
        // unspecified address (0.0.0.0, ::) overlaps every other one.
        let (m, w) = (self.server.mllp_addr(), self.server.web_addr());
        // `::ffff:127.0.0.1` is 127.0.0.1, hence the canonical form.
        if m.port() == w.port()
            && (m.ip().to_canonical() == w.ip().to_canonical()
                || m.ip().is_unspecified()
                || w.ip().is_unspecified())
        {
            problems.push(format!(
                "server.mllp_port and server.web_port are both {} on overlapping \
                 addresses ({} and {})",
                m.port(),
                m.ip(),
                w.ip()
            ));
        }
        if let Some(file) = self.logging.file.as_deref().filter(|f| !f.is_empty()) {
            // "logs/" or "logs/." would log to ./logs.YYYY-MM-DD next to the
            // directory: the text after the last separator must be a name.
            // (`Path` normalizes a trailing "." away, so check the text.)
            let last = file.rsplit(['/', '\\']).next().unwrap_or_default();
            let names_a_file = !matches!(last, "" | "." | "..");
            if !names_a_file {
                problems.push(format!(
                    "logging.file {file:?} must name a file, e.g. \"logs/harux.log\""
                ));
            }
        }
        if self.store.max_memory_mb != 0 && self.store.max_memory_mb < self.mllp.max_message_size_mb
        {
            problems.push(format!(
                "store.max_memory_mb ({}) is smaller than mllp.max_message_size_mb ({}) — \
                 a message of the allowed size could never be kept",
                self.store.max_memory_mb, self.mllp.max_message_size_mb
            ));
        }
        if let Err(e) = check_log_level(&self.logging.level) {
            problems.push(e);
        }
        if problems.is_empty() {
            Ok(())
        } else {
            let source = self.source.as_ref().map_or_else(
                || "defaults/environment".to_string(),
                |p| p.display().to_string(),
            );
            Err(format!(
                "invalid configuration ({source}): {}",
                problems.join("; ")
            ))
        }
    }
}

/// Levels a log directive may name.
const LOG_LEVELS: [&str; 6] = ["trace", "debug", "info", "warn", "error", "off"];

/// `logging.level` (or `RUST_LOG`, which overrides it) must be a level or a
/// list of `target=level` directives. `tracing`'s own parser accepts a bare
/// word as a *target* name, so a typo like `warm` would silently switch all
/// logging off — checked here instead (#233 review).
fn check_log_level(spec: &str) -> Result<(), String> {
    let bad = |what: &str| {
        format!(
            "logging.level / RUST_LOG {spec:?}: {what} is not a log level \
             (trace, debug, info, warn, error, off) or target=level directive"
        )
    };
    let directives: Vec<&str> = spec
        .split(',')
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .collect();
    if directives.is_empty() {
        return Err(bad("an empty value"));
    }
    for d in directives {
        let level = match d.rsplit_once('=') {
            Some((_, level)) => level.trim(),
            // `target[span]` forms are left to tracing's parser.
            None if d.contains('[') => continue,
            None => d,
        };
        if !LOG_LEVELS.iter().any(|l| l.eq_ignore_ascii_case(level)) {
            return Err(bad(&format!("{level:?}")));
        }
    }
    tracing_subscriber::EnvFilter::try_new(spec).map_err(|e| bad(&e.to_string()))?;
    Ok(())
}

/// Search for `harux.toml`: first next to the binary, then in the CWD.
fn find_config_path() -> Option<PathBuf> {
    const FILE_NAME: &str = "harux.toml";

    // 1. Next to the binary
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(FILE_NAME);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    // 2. Current working directory
    if let Ok(cwd) = std::env::current_dir() {
        let candidate = cwd.join(FILE_NAME);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    None
}

impl std::fmt::Display for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "  MLLP listener:      {}", self.server.mllp_addr())?;
        writeln!(f, "  Web listener:       {}", self.server.web_addr())?;
        writeln!(
            f,
            "  Shutdown timeout:   {}s",
            self.server.shutdown_timeout_secs
        )?;
        writeln!(
            f,
            "  Allowed web hosts:  IP addresses, localhost{}",
            self.server
                .allowed_hosts
                .iter()
                .map(|h| format!(", {h}"))
                .collect::<String>()
        )?;
        writeln!(f, "  Log level:          {}", self.logging.level)?;
        if let Some(file) = self
            .logging
            .file
            .as_deref()
            .filter(|_| self.logging.file_unusable)
        {
            writeln!(
                f,
                "  File log:           {file} (unusable, logging to stdout only)"
            )?;
        } else if let Some(file) = &self.logging.file {
            let keep = match self.logging.max_files {
                0 => "all kept".to_string(),
                n => format!("last {n} kept"),
            };
            writeln!(
                f,
                "  File log:           {file} (daily files {file}.YYYY-MM-DD, {keep})"
            )?;
        } else {
            writeln!(f, "  File log:           disabled")?;
        }
        writeln!(f, "  Max messages:       {}", self.store.max_messages)?;
        writeln!(f, "  Max memory:         {} MB", self.store.max_memory_mb)?;
        writeln!(
            f,
            "  Bookmark budget:    {}%",
            self.store.bookmark_budget_percent()
        )?;
        writeln!(
            f,
            "  Max message size:   {} MB",
            self.mllp.max_message_size_mb
        )?;
        writeln!(f, "  Read timeout:       {}s", self.mllp.read_timeout_secs)?;
        writeln!(f, "  Write timeout:      {}s", self.mllp.write_timeout_secs)?;
        writeln!(f, "  Max connections:    {}", self.mllp.max_connections)?;
        if !self.send.enabled {
            return write!(f, "  Sending:            off (send.enabled = false)");
        }
        let limit = match self.send.allowed_destinations.len() {
            0 => "to any destination".to_string(),
            _ => format!(
                "only to {}",
                self.send.destination_rules().map_or_else(
                    |e| e,
                    |r| r
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            ),
        };
        writeln!(f, "  Sending:            ON, {limit}")?;
        let tls = &self.send.tls;
        writeln!(
            f,
            "  Send TLS:           trusts the OS store{}; unverified {}; client certificate {}",
            tls.ca_file
                .as_deref()
                .map_or(String::new(), |c| format!(" and {c}")),
            if tls.allow_unverified {
                "allowed"
            } else {
                "not allowed"
            },
            // The certificate's file, never the key's.
            tls.client_cert_file.as_deref().unwrap_or("none")
        )?;
        match self.loopback_destination() {
            Some(addr) => writeln!(f, "  Loopback target:    {addr}")?,
            None => writeln!(f, "  Loopback target:    off")?,
        }
        write!(
            f,
            "  Send limits:        connect {}s, response {}s, {} runs at a time, \
             {} messages / {}% of the store per run",
            self.send.connect_timeout_secs,
            self.send.response_timeout_secs,
            self.send.max_concurrent_runs,
            self.send.max_run_messages,
            self.send.max_run_store_percent
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults() {
        let config = Config::default();
        assert_eq!(config.server.mllp_port, 2575);
        assert_eq!(config.server.web_port, 8080);
        assert_eq!(config.server.shutdown_timeout_secs, 10);
        assert_eq!(config.logging.level, "info");
        assert_eq!(config.logging.file, None);
        assert_eq!(config.logging.max_files, 14);
        assert_eq!(config.server.mllp_addr().to_string(), "0.0.0.0:2575");
        assert_eq!(config.server.web_addr().to_string(), "0.0.0.0:8080");
        assert_eq!(config.store.max_messages, 10_000);
        assert_eq!(config.store.max_memory_mb, 512);
        assert_eq!(config.store.bookmark_budget_percent, 50);
        assert_eq!(config.mllp.max_message_size_mb, 10);
        assert_eq!(config.mllp.read_timeout_secs, 60);
        assert_eq!(config.mllp.write_timeout_secs, 30);
        assert_eq!(config.mllp.max_connections, 100);
    }

    #[test]
    fn test_convenience_methods() {
        let store = StoreConfig::default();
        assert_eq!(store.max_memory_bytes(), 512 * 1024 * 1024);
        assert_eq!(store.bookmark_budget_percent(), 50);

        let over = StoreConfig {
            bookmark_budget_percent: 100,
            ..StoreConfig::default()
        };
        assert_eq!(over.bookmark_budget_percent(), MAX_BOOKMARK_BUDGET_PERCENT);

        let mllp = MllpConfig::default();
        assert_eq!(mllp.max_message_size(), 10 * 1024 * 1024);
        assert_eq!(mllp.read_timeout(), Duration::from_secs(60));
        assert_eq!(mllp.write_timeout(), Duration::from_secs(30));
    }

    #[test]
    fn test_parse_full_toml() {
        let toml_str = r#"
[server]
mllp_port = 3000
web_port = 9090
shutdown_timeout_secs = 5
allowed_hosts = ["harux.klinik.local"]
mllp_bind = "10.1.2.3"
web_bind = "::1"

[logging]
level = "debug"
file = "logs/test.log"
max_files = 30

[store]
max_messages = 5000
max_memory_mb = 256
bookmark_budget_percent = 30

[mllp]
max_message_size_mb = 20
read_timeout_secs = 120
write_timeout_secs = 60
max_connections = 50
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.server.mllp_port, 3000);
        assert_eq!(config.server.web_port, 9090);
        assert_eq!(config.server.shutdown_timeout_secs, 5);
        assert_eq!(config.server.allowed_hosts, ["harux.klinik.local"]);
        assert_eq!(config.logging.level, "debug");
        assert_eq!(config.logging.file.as_deref(), Some("logs/test.log"));
        assert_eq!(config.logging.max_files, 30);
        assert_eq!(config.server.mllp_addr().to_string(), "10.1.2.3:3000");
        assert_eq!(config.server.web_addr().to_string(), "[::1]:9090");
        assert_eq!(config.store.max_messages, 5000);
        assert_eq!(config.store.max_memory_mb, 256);
        assert_eq!(config.store.bookmark_budget_percent, 30);
        assert_eq!(config.mllp.max_message_size_mb, 20);
        assert_eq!(config.mllp.read_timeout_secs, 120);
        assert_eq!(config.mllp.write_timeout_secs, 60);
        assert_eq!(config.mllp.max_connections, 50);
    }

    #[test]
    fn test_parse_partial_toml() {
        let toml_str = r#"
[server]
mllp_port = 4000
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.server.mllp_port, 4000);
        // All other fields should use defaults
        assert_eq!(config.server.web_port, 8080);
        assert_eq!(config.logging.level, "info");
        assert_eq!(config.logging.file, None);
        assert_eq!(config.store.max_messages, 10_000);
        assert_eq!(config.mllp.read_timeout_secs, 60);
    }

    #[test]
    fn test_parse_empty_toml() {
        let config: Config = toml::from_str("").unwrap();
        assert_eq!(config.server.mllp_port, 2575);
        assert_eq!(config.server.web_port, 8080);
    }

    fn load(toml: &str, env: &[(&str, &str)]) -> Result<Config, String> {
        let env: Vec<(String, String)> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Config::from_sources(
            Some((PathBuf::from("harux.toml"), toml.to_string())),
            |key| env.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()),
        )
    }

    /// #233: one bad value used to discard the whole file silently.
    #[test]
    fn wrong_type_is_an_error_naming_the_key() {
        let err = load(
            "[server]\nmllp_port = 12578\n[store]\nmax_messages = \"500\"\n",
            &[],
        )
        .unwrap_err();
        assert!(
            err.contains("harux.toml") && err.contains("max_messages"),
            "{err}"
        );
    }

    /// #233: typos used to be ignored.
    #[test]
    fn unknown_keys_and_sections_are_errors() {
        let err = load("[store]\nmax_mesages = 500\n", &[]).unwrap_err();
        assert!(err.contains("max_mesages"), "{err}");
        let err = load("[unknown_section]\nfoo = 1\n", &[]).unwrap_err();
        assert!(err.contains("unknown_section"), "{err}");
    }

    #[test]
    fn invalid_env_override_is_an_error() {
        for (key, val) in [("MLLP_PORT", "abc"), ("WEB_PORT", "99999")] {
            let err = load("", &[(key, val)]).unwrap_err();
            assert!(err.contains(key) && err.contains(val), "{err}");
        }
        let cfg = load("", &[("MLLP_PORT", " 3000 "), ("RUST_LOG", "debug")]).unwrap();
        assert_eq!(cfg.server.mllp_port, 3000);
        assert_eq!(cfg.logging.level, "debug");
    }

    #[test]
    fn nonsense_values_are_rejected_together() {
        let err = load(
            "[store]\nmax_messages = 0\nmax_memory_mb = 0\n[mllp]\nmax_connections = 0\n",
            &[],
        )
        .unwrap_err();
        for key in [
            "store.max_messages",
            "store.max_memory_mb",
            "mllp.max_connections",
        ] {
            assert!(err.contains(key), "{key} missing in: {err}");
        }
        let err = load("[server]\nmllp_port = 9000\nweb_port = 9000\n", &[]).unwrap_err();
        assert!(err.contains("both 9000"), "{err}");
        // Same port on two different addresses is fine (#255 review); an
        // unspecified address overlaps every other one.
        let two = "[server]\nmllp_port = 9000\nweb_port = 9000\nmllp_bind = \"127.0.0.1\"\n";
        assert!(load(&format!("{two}web_bind = \"127.0.0.2\"\n"), &[]).is_ok());
        assert!(load(&format!("{two}web_bind = \"::1\"\n"), &[]).is_ok());
        let mapped = load(&format!("{two}web_bind = \"::ffff:127.0.0.1\"\n"), &[]).unwrap_err();
        assert!(
            mapped.contains("both 9000"),
            "IPv4-mapped is the same address: {mapped}"
        );
        for overlapping in ["127.0.0.1", "0.0.0.0", "::"] {
            let err = load(&format!("{two}web_bind = \"{overlapping}\"\n"), &[]).unwrap_err();
            assert!(err.contains("both 9000"), "{overlapping}: {err}");
        }
        for dir in ["logs/", "logs\\\\", ".", "logs/..", "logs/.", "logs\\\\."] {
            let err = load(&format!("[logging]\nfile = \"{dir}\"\n"), &[]).unwrap_err();
            assert!(err.contains("must name a file"), "{dir}: {err}");
        }
        assert!(
            load("[logging]\nfile = \"\"\n", &[]).is_ok(),
            "empty = no file log"
        );
        assert!(load("[logging]\nfile = \"harux.log\"\n", &[]).is_ok());
    }

    /// #255 review: the effective configuration must not claim a file log
    /// that could not be opened.
    #[test]
    fn effective_configuration_shows_an_unusable_log_file() {
        let mut cfg = load("[logging]\nfile = \"logs/harux.log\"\nmax_files = 3\n", &[]).unwrap();
        assert!(cfg
            .to_string()
            .contains("logs/harux.log.YYYY-MM-DD, last 3 kept"));
        cfg.logging.file_unusable = true;
        let shown = cfg.to_string();
        assert!(
            shown.contains("logs/harux.log (unusable, logging to stdout only)"),
            "{shown}"
        );
        let err = load("[store]\nmax_memory_mb = 5\n", &[]).unwrap_err();
        assert!(
            err.contains("smaller than mllp.max_message_size_mb"),
            "{err}"
        );
    }

    /// #233 review: a typo in the level used to switch logging off silently.
    #[test]
    fn log_level_is_validated() {
        let err = load("[logging]\nlevel = \"warm\"\n", &[]).unwrap_err();
        assert!(err.contains("\"warm\""), "{err}");
        let err = load("", &[("RUST_LOG", "harux=verbose")]).unwrap_err();
        assert!(err.contains("verbose"), "{err}");
        for ok in [
            "info",
            "WARN",
            "harux=debug,tower_http=info",
            "off",
            "warn,harux::mllp=trace",
        ] {
            assert!(load("", &[("RUST_LOG", ok)]).is_ok(), "{ok}");
        }
    }

    /// #237: a bind address that is not an IP address stops the start and
    /// names the key.
    #[test]
    fn invalid_bind_address_is_an_error() {
        for bad in ["localhost", "0.0.0.0:8080", "300.1.1.1", ""] {
            let err = load(&format!("[server]\nweb_bind = \"{bad}\"\n"), &[]).unwrap_err();
            assert!(err.contains("web_bind"), "{bad}: {err}");
        }
        let cfg = load("[server]\nmllp_bind = \"::\"\n", &[]).unwrap();
        assert_eq!(cfg.server.mllp_addr().to_string(), "[::]:2575");
    }

    /// #272: sending is off by default; the section fails closed.
    #[test]
    fn send_section_defaults_and_validation() {
        let cfg = load("", &[]).unwrap();
        assert!(!cfg.send.enabled, "sending is off by default (D11)");
        assert!(cfg.send.loopback_target);
        assert_eq!(cfg.send.connect_timeout_secs, 5);
        assert_eq!(cfg.send.response_timeout_secs, 30);
        assert_eq!(cfg.send.max_run_messages, 1000);
        assert_eq!(cfg.send.max_run_store_percent, 25);
        assert_eq!(cfg.send.max_concurrent_runs, 4);
        assert!(cfg.to_string().contains("Sending:            off"));

        let err = load(
            "[send]\nconnect_timeout_secs = 0\nmax_run_store_percent = 101\nmax_concurrent_runs = 0\n",
            &[],
        )
        .unwrap_err();
        for key in [
            "send.connect_timeout_secs",
            "send.max_run_store_percent",
            "send.max_concurrent_runs",
        ] {
            assert!(err.contains(key), "{key} missing in: {err}");
        }
        let err = load("[send.tls]\nclient_cert_file = \"harux.pem\"\n", &[]).unwrap_err();
        assert!(err.contains("must be set together"), "{err}");
        let err = load("[send.tls]\nca_flie = \"x\"\n", &[]).unwrap_err();
        assert!(err.contains("ca_flie"), "{err}");
        let err = load("[send]\nenabeld = true\n", &[]).unwrap_err();
        assert!(err.contains("enabeld"), "{err}");
        for bad in [
            "orchestra.local",
            "orchestra.local:",
            "orchestra.local:0",
            "orchestra.local:7000-6000",
            "::1:2575",
            "[::1",
            "bad host:6000",
            "2130706433:6000",
        ] {
            let err = load(
                &format!("[send]\nallowed_destinations = [\"{bad}\"]\n"),
                &[],
            )
            .unwrap_err();
            assert!(err.contains("send.allowed_destinations"), "{bad}: {err}");
        }
        let cfg = load(
            "[send]\nenabled = true\nallowed_destinations = [\"Orchestra.Klinik.Local.:6000-6999\", \"[::ffff:10.0.0.1]:2575\"]\n",
            &[],
        )
        .unwrap();
        let rules = cfg.send.destination_rules().unwrap();
        assert_eq!(rules[0].host, "orchestra.klinik.local");
        assert_eq!(rules[0].ports, 6000..=6999);
        assert_eq!(rules[1].host, "10.0.0.1", "canonical form");
        assert!(rules[1].allows("10.0.0.1", 2575));
        assert!(!rules[1].allows("10.0.0.1", 2576));
        let shown = cfg.to_string();
        assert!(
            shown.contains("only to orchestra.klinik.local:6000-6999, 10.0.0.1:2575"),
            "{shown}"
        );
    }

    #[test]
    fn hosts_are_normalized() {
        for (input, expect) in [
            ("Orchestra.Local.", Some("orchestra.local")),
            ("[::1]", Some("::1")),
            ("::ffff:127.0.0.1", Some("127.0.0.1")),
            ("10.1.2.3", Some("10.1.2.3")),
            ("host_1-a", Some("host_1-a")),
            ("", None),
            ("a..b", None),
            ("-a.b", None),
            ("10.0.0.01", None),
            ("evil/path", None),
            ("a b", None),
        ] {
            assert_eq!(normalize_host(input).as_deref(), expect, "{input:?}");
        }
    }

    #[test]
    fn loopback_destination_follows_the_listener() {
        let cfg = load("[server]\nmllp_port = 3000\n", &[]).unwrap();
        assert_eq!(
            cfg.loopback_destination().unwrap().to_string(),
            "127.0.0.1:3000"
        );
        let cfg = load("[server]\nmllp_bind = \"::\"\n", &[]).unwrap();
        assert_eq!(
            cfg.loopback_destination().unwrap().to_string(),
            "[::1]:2575"
        );
        let cfg = load("[server]\nmllp_bind = \"10.0.0.5\"\n", &[]).unwrap();
        assert_eq!(
            cfg.loopback_destination().unwrap().to_string(),
            "10.0.0.5:2575"
        );
        let cfg = load("[send]\nloopback_target = false\n", &[]).unwrap();
        assert!(cfg.loopback_destination().is_none());
    }

    #[test]
    fn shipped_config_files_load() {
        for path in ["harux.toml", "examples/harux-example.toml"] {
            let contents = std::fs::read_to_string(path).unwrap();
            let cfg = Config::from_sources(Some((PathBuf::from(path), contents)), |_| None)
                .unwrap_or_else(|e| panic!("{path}: {e}"));
            assert_eq!(cfg.source.as_deref(), Some(std::path::Path::new(path)));
        }
        assert!(Config::from_sources(None, |_| None)
            .unwrap()
            .source
            .is_none());
    }
}
