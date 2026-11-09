//! Edem/iLook protocol configuration. The stream host is provider data, never an editable CDN.
use super::{CatalogError, ProviderKind, ProviderProfile};
use serde::{Deserialize, Serialize};
pub const STREAM_HOST: &str = "sewv654wfcsdwfi87fwvgbngh.siauliairsavlt.pw";
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub key: String,
    #[serde(default)]
    pub list: u8,
    #[serde(default)]
    pub portal: String,
}
impl Config {
    pub fn parse(source: &str) -> Result<Self, CatalogError> {
        let mut value: serde_json::Value =
            serde_json::from_str(source).map_err(|_| CatalogError::InvalidProfile)?;
        let object = value.as_object_mut().ok_or(CatalogError::InvalidProfile)?;
        // Historical persisted endpoint values never regain authority over streams.
        for field in ["edcdn", "cdn", "ededcdn"] {
            object.remove(field);
        }
        for (old, new) in [
            ("edkey", "key"),
            ("edlist", "list"),
            ("vpurl", "portal"),
            ("edvpurl", "portal"),
        ] {
            if let Some(old_value) = object.remove(old) {
                object.entry(new).or_insert(old_value);
            }
        }
        if let Some(list) = object
            .get("list")
            .and_then(|value| value.as_str())
            .and_then(|text| text.parse::<u8>().ok())
        {
            object.insert("list".into(), list.into());
        }
        let config: Self =
            serde_json::from_value(value).map_err(|_| CatalogError::InvalidProfile)?;
        if config.key.len() > 512
            || config.key.is_empty()
            || !config
                .key
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
            || config.list > 3
            || (!config.portal.is_empty() && super::parse_vportal_link(&config.portal).is_err())
        {
            return Err(CatalogError::InvalidProfile);
        }
        Ok(config)
    }
    pub fn playlist_url(&self) -> String {
        let suffix = if self.list == 0 {
            String::new()
        } else {
            self.list.to_string()
        };
        format!("https://epg.drm-play.com/edem/edem_epg_ico{suffix}.m3u8")
    }
    pub fn stream(&self, channel: &str) -> Result<String, CatalogError> {
        if channel.is_empty() || channel.len() > 20 || !channel.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(CatalogError::InvalidFormat);
        }
        Ok(format!(
            "http://{STREAM_HOST}/iptv/{}/{channel}/index.m3u8",
            self.key
        ))
    }
    pub fn template_profile(&self, profile: &ProviderProfile) -> ProviderProfile {
        let mut template = profile.clone();
        template.kind = ProviderKind::M3u;
        template.source = self.playlist_url();
        template
    }
}
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EdemConfig(<private>)")
    }
}
/// Template links contain placeholder credentials; only their numeric channel identity is used.
pub fn channel_id(url: &str) -> Result<&str, CatalogError> {
    let path = url
        .split_once("://")
        .and_then(|(_, rest)| rest.split_once('/'))
        .map(|(_, path)| path)
        .ok_or(CatalogError::InvalidFormat)?;
    let parts: Vec<_> = path.split('/').collect();
    if parts.len() != 4
        || parts[0] != "iptv"
        || parts[3] != "index.m3u8"
        || parts[2].is_empty()
        || parts[2].len() > 20
        || !parts[2].bytes().all(|b| b.is_ascii_digit())
    {
        return Err(CatalogError::InvalidFormat);
    }
    Ok(parts[2])
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn latest_endpoint_replaces_template_credentials() {
        let c = Config::parse(r#"{"key":"test_key","list":3}"#).unwrap();
        assert_eq!(
            c.playlist_url(),
            "https://epg.drm-play.com/edem/edem_epg_ico3.m3u8"
        );
        assert_eq!(
            c.stream(channel_id("http://localhost/iptv/000000/407/index.m3u8").unwrap())
                .unwrap(),
            format!("http://{STREAM_HOST}/iptv/test_key/407/index.m3u8")
        );
        let migrated =
            Config::parse(r#"{"edkey":"x","edlist":"2","edcdn":"old.invalid","vpurl":""}"#)
                .unwrap();
        assert_eq!(migrated.list, 2);
        assert!(migrated.stream("42").unwrap().contains(STREAM_HOST));
        assert!(Config::parse(r#"{"key":"x","unknown":"old"}"#).is_err());
        assert!(Config::parse(r#"{"key":"../secret"}"#).is_err());
        assert!(Config::parse(r#"{"key":"x","list":4}"#).is_err());
        assert!(!format!("{:?}", c).contains("test_key"));
    }
}
