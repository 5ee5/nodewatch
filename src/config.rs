//! Environment-driven configuration.
//!
//! `Config::from_lookup` takes injected lookup/read functions so tests never
//! touch the process environment (`std::env::set_var` is unsafe in edition 2024).

use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::time::Duration;

pub const MIN_TOKEN_LEN: usize = 32;

/// A secret string whose `Debug` output is redacted so it can't leak into logs.
#[derive(Clone)]
// TODO(step 2/3): remove once the sampler and auth consume this.
#[allow(dead_code)]
pub struct Secret(String);

impl Secret {
    // TODO(step 2/3): remove once the sampler and auth consume this.
    #[allow(dead_code)]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

#[derive(Debug, Clone)]
// TODO(step 2/3): remove once the sampler and auth consume this.
#[allow(dead_code)]
pub struct Config {
    pub bind: SocketAddr,
    pub token: Secret,
    pub cookie_secure: bool,
    pub session_ttl: Duration,
    pub refresh_interval: Duration,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ConfigError {
    MissingToken,
    TokenConflict,
    TokenTooShort,
    TokenFile(String),
    Invalid { var: &'static str, reason: String },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingToken => write!(
                f,
                "no access token configured: set NODEWATCH_TOKEN or NODEWATCH_TOKEN_FILE \
                 (generate one with `openssl rand -hex 32`)"
            ),
            Self::TokenConflict => {
                write!(
                    f,
                    "set only one of NODEWATCH_TOKEN and NODEWATCH_TOKEN_FILE"
                )
            }
            Self::TokenTooShort => {
                write!(
                    f,
                    "access token must be at least {MIN_TOKEN_LEN} characters"
                )
            }
            Self::TokenFile(e) => write!(f, "cannot read NODEWATCH_TOKEN_FILE: {e}"),
            Self::Invalid { var, reason } => write!(f, "invalid {var}: {reason}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|k| std::env::var(k).ok(), |p| std::fs::read_to_string(p))
    }

    pub fn from_lookup(
        get: impl Fn(&str) -> Option<String>,
        read_file: impl Fn(&str) -> io::Result<String>,
    ) -> Result<Self, ConfigError> {
        let bind = parse_or(&get, "NODEWATCH_BIND", "127.0.0.1:8080", |s| {
            s.parse::<SocketAddr>().map_err(|e| e.to_string())
        })?;

        let token = match (get("NODEWATCH_TOKEN"), get("NODEWATCH_TOKEN_FILE")) {
            (Some(_), Some(_)) => return Err(ConfigError::TokenConflict),
            (Some(t), None) => t,
            (None, Some(path)) => {
                read_file(&path).map_err(|e| ConfigError::TokenFile(e.kind().to_string()))?
            }
            (None, None) => return Err(ConfigError::MissingToken),
        };
        let token = token.trim().to_owned();
        if token.is_empty() {
            return Err(ConfigError::MissingToken);
        }
        if token.len() < MIN_TOKEN_LEN {
            return Err(ConfigError::TokenTooShort);
        }

        let cookie_secure = parse_or(&get, "NODEWATCH_COOKIE_SECURE", "false", |s| {
            s.parse::<bool>().map_err(|e| e.to_string())
        })?;
        let ttl = parse_or(&get, "NODEWATCH_SESSION_TTL_SECS", "43200", |s| {
            s.parse::<u64>().map_err(|e| e.to_string())
        })?;
        let refresh = parse_or(&get, "NODEWATCH_REFRESH_SECS", "2", |s| {
            s.parse::<u64>().map_err(|e| e.to_string())
        })?;

        Ok(Self {
            bind,
            token: Secret(token),
            cookie_secure,
            session_ttl: Duration::from_secs(ttl.max(60)),
            refresh_interval: Duration::from_secs(refresh.clamp(1, 60)),
        })
    }
}

fn parse_or<T>(
    get: &impl Fn(&str) -> Option<String>,
    var: &'static str,
    default: &str,
    parse: impl Fn(&str) -> Result<T, String>,
) -> Result<T, ConfigError> {
    let raw = get(var).unwrap_or_else(|| default.to_owned());
    parse(raw.trim()).map_err(|reason| ConfigError::Invalid { var, reason })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn load(vars: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Config::from_lookup(
            |k| map.get(k).cloned(),
            |p| {
                if p == "/ok" {
                    Ok(format!("{TOKEN}\n"))
                } else {
                    Err(io::Error::from(io::ErrorKind::NotFound))
                }
            },
        )
    }

    #[test]
    fn defaults_with_token() {
        let c = load(&[("NODEWATCH_TOKEN", TOKEN)]).unwrap();
        assert_eq!(c.bind, "127.0.0.1:8080".parse().unwrap());
        assert!(!c.cookie_secure);
        assert_eq!(c.refresh_interval, Duration::from_secs(2));
        assert_eq!(c.session_ttl, Duration::from_secs(43200));
    }

    #[test]
    fn token_is_required() {
        assert_eq!(load(&[]).unwrap_err(), ConfigError::MissingToken);
        assert_eq!(
            load(&[("NODEWATCH_TOKEN", "  ")]).unwrap_err(),
            ConfigError::MissingToken
        );
    }

    #[test]
    fn short_token_rejected() {
        assert_eq!(
            load(&[("NODEWATCH_TOKEN", "short")]).unwrap_err(),
            ConfigError::TokenTooShort
        );
    }

    #[test]
    fn token_file_is_read_and_trimmed() {
        let c = load(&[("NODEWATCH_TOKEN_FILE", "/ok")]).unwrap();
        assert_eq!(c.token.expose(), TOKEN);
    }

    #[test]
    fn token_file_error_and_conflict() {
        assert!(matches!(
            load(&[("NODEWATCH_TOKEN_FILE", "/nope")]),
            Err(ConfigError::TokenFile(_))
        ));
        assert_eq!(
            load(&[("NODEWATCH_TOKEN", TOKEN), ("NODEWATCH_TOKEN_FILE", "/ok")]).unwrap_err(),
            ConfigError::TokenConflict
        );
    }

    #[test]
    fn invalid_values_name_the_variable() {
        let err = load(&[("NODEWATCH_TOKEN", TOKEN), ("NODEWATCH_BIND", "nope")]).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Invalid {
                var: "NODEWATCH_BIND",
                ..
            }
        ));
    }

    #[test]
    fn refresh_is_clamped() {
        let c = load(&[("NODEWATCH_TOKEN", TOKEN), ("NODEWATCH_REFRESH_SECS", "0")]).unwrap();
        assert_eq!(c.refresh_interval, Duration::from_secs(1));
    }

    #[test]
    fn debug_never_prints_token() {
        let c = load(&[("NODEWATCH_TOKEN", TOKEN)]).unwrap();
        assert!(!format!("{c:?}").contains(TOKEN));
    }
}
