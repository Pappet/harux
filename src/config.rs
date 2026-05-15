use serde::Deserialize;
use std::path::PathBuf;
use std::time::Duration;
use tracing::info;

/// Top-level configuration for Harux.
///
/// Load priority: `harux.toml` (next to binary, then CWD) → env vars → built-in defaults.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub logging: LoggingConfig,
    pub store: StoreConfig,
    pub mllp: MllpConfig,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub mllp_port: u16,
    pub web_port: u16,
    pub shutdown_timeout_secs: u64,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub level: String,
    pub file: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct StoreConfig {
    pub max_messages: usize,
    pub max_memory_mb: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
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
    /// Load configuration with priority: file → env vars → defaults.
    pub fn load() -> Self {
        let mut config = match find_config_path() {
            Some(path) => {
                info!("Loading config from {}", path.display());
                match std::fs::read_to_string(&path) {
                    Ok(contents) => match toml::from_str::<Config>(&contents) {
                        Ok(cfg) => cfg,
                        Err(e) => {
                            eprintln!(
                                "WARNING: Failed to parse {}: {}. Using defaults.",
                                path.display(),
                                e
                            );
                            Config::default()
                        }
                    },
                    Err(e) => {
                        eprintln!(
                            "WARNING: Failed to read {}: {}. Using defaults.",
                            path.display(),
                            e
                        );
                        Config::default()
                    }
                }
            }
            None => Config::default(),
        };

        // Env-var overrides (highest priority)
        if let Ok(val) = std::env::var("MLLP_PORT") {
            if let Ok(port) = val.parse::<u16>() {
                config.server.mllp_port = port;
            }
        }
        if let Ok(val) = std::env::var("WEB_PORT") {
            if let Ok(port) = val.parse::<u16>() {
                config.server.web_port = port;
            }
        }
        if let Ok(val) = std::env::var("RUST_LOG") {
            config.logging.level = val;
        }

        config
    }
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
        assert_eq!(config.mllp.max_message_size_mb, 10);
        assert_eq!(config.mllp.read_timeout_secs, 60);
        assert_eq!(config.mllp.write_timeout_secs, 30);
        assert_eq!(config.mllp.max_connections, 100);
    }

    #[test]
    fn test_convenience_methods() {
        let store = StoreConfig::default();
        assert_eq!(store.max_memory_bytes(), 512 * 1024 * 1024);

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

[logging]
level = "debug"
file = "logs/test.log"

[store]
max_messages = 5000
max_memory_mb = 256

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
        assert_eq!(config.logging.level, "debug");
        assert_eq!(config.logging.file.as_deref(), Some("logs/test.log"));
        assert_eq!(config.store.max_messages, 5000);
        assert_eq!(config.store.max_memory_mb, 256);
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

    #[test]
    fn test_unknown_fields_ignored() {
        let toml_str = r#"
[server]
mllp_port = 2575
unknown_field = "should be ignored"

[unknown_section]
foo = "bar"
"#;
        // serde(default) with deny_unknown_fields not set → unknown fields are ignored
        let result = toml::from_str::<Config>(toml_str);
        assert!(result.is_ok());
    }
}
