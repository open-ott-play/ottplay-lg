//! Structural native operator settings. Protocol requests are evaluated by the application shared core.
use super::{CatalogError, OperatorId};
use serde::{Deserialize, Serialize};
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub server: String,
    #[serde(
        alias = "user",
        alias = "login",
        alias = "key",
        alias = "token",
        alias = "id"
    )]
    pub username: String,
    #[serde(alias = "pass", alias = "pin")]
    pub password: String,
    #[serde(alias = "m3u", alias = "www")]
    pub playlist: String,
    pub mac: String,
    pub list: u8,
    pub mode: u8,
}
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OperatorConfig(<private>)")
    }
}
impl Config {
    pub fn parse(id: OperatorId, source: &str) -> Result<Self, CatalogError> {
        if source.len() > 16384 {
            return Err(CatalogError::InvalidProfile);
        }
        let mut config: Self =
            serde_json::from_str(source).map_err(|_| CatalogError::InvalidProfile)?;
        for value in [
            &config.server,
            &config.username,
            &config.password,
            &config.playlist,
            &config.mac,
        ] {
            if value.len() > 8192 || value.chars().any(char::is_control) {
                return Err(CatalogError::InvalidProfile);
            }
        }
        if config.list > 3
            || config.mode > 2
            || (id.canonical_id() == "kb-team" && config.list > 2)
            || (matches!(id.canonical_id(), "shura" | "ottclub" | "antifriz") && config.mode > 1)
        {
            return Err(CatalogError::InvalidProfile);
        }
        if id.canonical_id() == "shura" {
            if config.server.is_empty() {
                config.server = "1".into();
            }
            let server = config
                .server
                .parse::<u8>()
                .ok()
                .filter(|server| [1, 2, 3, 5].contains(server))
                .ok_or(CatalogError::InvalidProfile)?;
            config.server = server.to_string();
        }
        if id.canonical_id() == "ottclub" && !config.server.is_empty() {
            if config.server.chars().any(|c| "/?#@\\".contains(c))
                || super::http_url(&format!("http://{}/", config.server)).is_none()
            {
                return Err(CatalogError::InvalidProfile);
            }
        }
        if id.canonical_id() == "tvteam" && config.playlist.is_empty() {
            config.playlist = "https://tv.team/pl/11/".into();
        }
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn branded_server_fields_are_validated_for_the_actual_protocol() {
        let shura = OperatorId::from_id("shura").unwrap();
        assert_eq!(
            Config::parse(shura, r#"{"server":"3"}"#).unwrap().server,
            "3"
        );
        for server in ["https://bad.invalid", "0", "4", "6", "1/evil", "1?evil"] {
            assert!(
                Config::parse(shura, &serde_json::json!({"server":server}).to_string()).is_err()
            );
        }
        let club = OperatorId::from_id("ottclub").unwrap();
        assert_eq!(
            Config::parse(club, r#"{"server":"service.invalid:8080"}"#)
                .unwrap()
                .server,
            "service.invalid:8080"
        );
        for server in [
            "https://service.invalid",
            "service.invalid/path",
            "user@service.invalid",
            "service.invalid?x=y",
        ] {
            assert!(
                Config::parse(club, &serde_json::json!({"server":server}).to_string()).is_err()
            );
        }
    }
}
