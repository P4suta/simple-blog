use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

const DEFAULT_DATA_DIR: &str = "./data";
const DEFAULT_BIND: &str = "127.0.0.1:8080";
const DEFAULT_PUBLIC_URL: &str = "http://localhost:8080";

#[derive(Clone, Debug)]
pub struct Config {
    pub data_dir: PathBuf,
    pub bind: SocketAddr,
    pub public_url: Url,
    pub trusted_proxies: Vec<IpAddr>,
    pub max_upload_bytes: usize,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    pub data_dir: Option<PathBuf>,
    pub bind: Option<String>,
    pub public_url: Option<String>,
    pub trusted_proxies: Option<Vec<IpAddr>>,
    pub max_upload_bytes: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct Overrides {
    pub data_dir: Option<PathBuf>,
    pub bind: Option<String>,
    pub public_url: Option<String>,
    pub trusted_proxies: Option<Vec<IpAddr>>,
    pub max_upload_bytes: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct ConfigSources {
    pub cli: Overrides,
    pub env: BTreeMap<String, String>,
    pub file: Option<ConfigFile>,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("invalid bind address: {0}")]
    Bind(#[from] std::net::AddrParseError),
    #[error("invalid public URL: {0}")]
    PublicUrl(String),
    #[error("invalid max upload size")]
    MaxUpload,
    #[error("could not read configuration at {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not parse configuration at {path}: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
}

impl Config {
    pub fn resolve(sources: ConfigSources) -> Result<Self, ConfigError> {
        let file = sources.file.unwrap_or_default();
        let env = sources.env;

        let data_dir = sources
            .cli
            .data_dir
            .or_else(|| env.get("SIMPLE_BLOG_DATA_DIR").map(PathBuf::from))
            .or(file.data_dir)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_DATA_DIR));
        let bind = sources
            .cli
            .bind
            .or_else(|| env.get("SIMPLE_BLOG_BIND").cloned())
            .or(file.bind)
            .unwrap_or_else(|| DEFAULT_BIND.to_owned())
            .parse()?;
        let public_url = sources
            .cli
            .public_url
            .or_else(|| env.get("SIMPLE_BLOG_PUBLIC_URL").cloned())
            .or(file.public_url)
            .unwrap_or_else(|| DEFAULT_PUBLIC_URL.to_owned());
        let public_url = parse_origin(&public_url)?;
        let trusted_proxies = sources
            .cli
            .trusted_proxies
            .or_else(|| {
                env.get("SIMPLE_BLOG_TRUSTED_PROXIES").map(|value| {
                    value
                        .split(',')
                        .filter_map(|item| item.trim().parse().ok())
                        .collect()
                })
            })
            .or(file.trusted_proxies)
            .unwrap_or_default();
        let max_upload_bytes = sources
            .cli
            .max_upload_bytes
            .or_else(|| {
                env.get("SIMPLE_BLOG_MAX_UPLOAD_BYTES")
                    .and_then(|value| value.parse().ok())
            })
            .or(file.max_upload_bytes)
            .unwrap_or(25 * 1024 * 1024);
        if max_upload_bytes == 0 {
            return Err(ConfigError::MaxUpload);
        }

        Ok(Self {
            data_dir,
            bind,
            public_url,
            trusted_proxies,
            max_upload_bytes,
        })
    }

    /// Loads the data-directory configuration while keeping precedence deterministic.
    pub fn load(overrides: Overrides) -> Result<Self, ConfigError> {
        let env: BTreeMap<String, String> = std::env::vars()
            .filter(|(key, _)| key.starts_with("SIMPLE_BLOG_"))
            .collect();
        let data_dir = overrides
            .data_dir
            .clone()
            .or_else(|| env.get("SIMPLE_BLOG_DATA_DIR").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from(DEFAULT_DATA_DIR));
        let path = data_dir.join("config.toml");
        let file = read_optional_file(&path)?;

        Self::resolve(ConfigSources {
            cli: overrides,
            env,
            file,
        })
    }

    #[must_use]
    pub fn database_path(&self) -> PathBuf {
        self.data_dir.join("simple-blog.sqlite3")
    }

    #[must_use]
    pub fn media_dir(&self) -> PathBuf {
        self.data_dir.join("media")
    }

    #[must_use]
    pub fn backup_dir(&self) -> PathBuf {
        self.data_dir.join("backups")
    }
}

fn read_optional_file(path: &Path) -> Result<Option<ConfigFile>, ConfigError> {
    match std::fs::read_to_string(path) {
        Ok(contents) => toml::from_str(&contents)
            .map(Some)
            .map_err(|source| ConfigError::Parse {
                path: path.to_owned(),
                source,
            }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ConfigError::Read {
            path: path.to_owned(),
            source,
        }),
    }
}

fn parse_origin(value: &str) -> Result<Url, ConfigError> {
    let mut url = Url::parse(value).map_err(|_| ConfigError::PublicUrl(value.to_owned()))?;
    let valid = matches!(url.scheme(), "http" | "https")
        && !url.cannot_be_a_base()
        && url.host().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && matches!(url.path(), "" | "/")
        && url.query().is_none()
        && url.fragment().is_none();
    if !valid {
        return Err(ConfigError::PublicUrl(value.to_owned()));
    }
    url.set_path("/");
    Ok(url)
}
