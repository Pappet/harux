use serde::Deserialize;
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
    /// The file the configuration was read from, if any — logged at startup.
    #[serde(skip)]
    pub source: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    pub mllp_port: u16,
    pub web_port: u16,
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

// --- Defaults ---

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            mllp_port: 2575,
            web_port: 8080,
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
        if self.server.mllp_port == self.server.web_port {
            problems.push(format!(
                "server.mllp_port and server.web_port are both {}",
                self.server.mllp_port
            ));
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
        writeln!(f, "  MLLP port:          {}", self.server.mllp_port)?;
        writeln!(f, "  Web port:           {}", self.server.web_port)?;
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
        if let Some(file) = &self.logging.file {
            writeln!(f, "  File log:           {} (daily rotation)", file)?;
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
        write!(f, "  Max connections:    {}", self.mllp.max_connections)
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

[logging]
level = "debug"
file = "logs/test.log"

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
