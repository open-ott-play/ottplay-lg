//! Pure, bounded normalization of provider-attested protection flags.
use serde_json::Value;
use std::collections::BTreeMap;

const KEYS: [&str; 4] = ["is_adult", "adult", "censored", "is_censored"];

fn flag(value: &Value) -> bool {
    match value {
        Value::Bool(value) => *value,
        Value::Number(number) if number.as_u64() == Some(0) => false,
        Value::String(value) if value == "0" || value.eq_ignore_ascii_case("false") => false,
        // Unknown advertised values must not silently remove protection.
        _ => true,
    }
}

pub fn advertised(row: &Value) -> Option<bool> {
    KEYS.iter().filter_map(|key| row.get(key)).map(flag).reduce(|a, b| a || b)
}

pub fn attributes(fields: &BTreeMap<String, String>) -> Option<bool> {
    KEYS.iter().filter_map(|key| fields.get(*key))
        .map(|value| flag(&Value::String(value.clone()))).reduce(|a, b| a || b)
}

/// A child cannot remove a protection attested by its parent container.
pub fn inherit(parent: Option<bool>, child: Option<bool>) -> Option<bool> {
    match (parent, child) {
        (Some(a), Some(b)) => Some(a || b),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{CatalogEntry, ProviderKind, ProviderProfile};
    use serde_json::json;
    #[test]
    fn explicit_boolean_numeric_and_string_flags_are_preserved_and_invalid_is_closed() {
        assert_eq!(advertised(&json!({})), None);
        for value in [json!(false), json!(0), json!("0"), json!("false")] {
            assert_eq!(advertised(&json!({"is_adult": value})), Some(false));
        }
        for value in [json!(true), json!(1), json!("1"), json!("true"), Value::Null,
            json!(-1), json!("invalid"), json!({}), json!([])] {
            assert_eq!(advertised(&json!({"is_adult": value})), Some(true));
        }
        assert_eq!(advertised(&json!({"is_adult":0,"censored":1})), Some(true));
        assert_eq!(inherit(Some(true), Some(false)), Some(true));
        assert_eq!(inherit(None, Some(false)), Some(false));
        assert_eq!(inherit(None, None), None);
    }
    fn profile(kind: ProviderKind, source: String) -> ProviderProfile {
        ProviderProfile { id: "source".into(), name: "Fixture source".into(), kind,
            source, source_alias: "Fixture source".into(), enabled: true }
    }
    #[test]
    fn m3u_flags_survive_normalization_and_old_wire_rows_remain_compatible() {
        let profile = profile(ProviderKind::M3u, "https://example.invalid/list.m3u".into());
        for value in ["1", "true", "invalid"] {
            let rows = super::super::parse_m3u(&format!(
                "#EXTM3U\n#EXTINF:-1 is_adult=\"{value}\" group-title=\"Documentary\",Fixture\nhttps://example.invalid/live.m3u8\n"), &profile).unwrap();
            let row = &rows.entries[0];
            assert_eq!(row.protection, Some(true));
            let encoded = serde_json::to_value(row).unwrap();
            assert_eq!(encoded["protection"], true);
            let mut old = encoded;
            old.as_object_mut().unwrap().remove("protection");
            assert_eq!(serde_json::from_value::<CatalogEntry>(old).unwrap().protection, None);
        }
    }
    #[test]
    fn xtream_flags_and_series_container_protection_reach_the_episode() {
        use super::super::xtream;
        let config = xtream::Config::parse(r#"{"server":"https://example.invalid","username":"fixture","password":"fixture"}"#).unwrap();
        let profile = profile(ProviderKind::Xtream, serde_json::to_string(&config).unwrap());
        let rows = xtream::streams(&json!([
            {"stream_id":1,"name":"Fixture","is_adult":"1"},
            {"stream_id":2,"name":"Other fixture","is_adult":false}
        ]), &profile, &config, "live", &BTreeMap::new()).unwrap();
        assert_eq!(rows.entries.iter().map(|row| row.protection).collect::<Vec<_>>(), [Some(true), Some(false)]);
        let parents = xtream::series(&json!([{"series_id":3,"name":"Fixture series","is_adult":1}]),
            &config, &BTreeMap::new()).unwrap();
        let episodes = xtream::episodes(&json!({"episodes":{"1":[{
            "id":4,"container_extension":"mp4","is_adult":0
        }]}}), &profile, &config, &parents[0]).unwrap();
        assert_eq!(episodes.entries[0].protection, Some(true));
    }
    #[test]
    fn stalker_container_protection_is_inherited_despite_an_unmarked_child() {
        use super::super::stalker;
        let config = stalker::Config::parse(r#"{"portal":"https://example.invalid/c/","mac":"00:1a:79:01:02:03"}"#).unwrap();
        let profile = profile(ProviderKind::Stalker, serde_json::to_string(&config).unwrap());
        let (_, folders) = stalker::rows(&profile, &config, "vod", &BTreeMap::new(), &stalker::Folder::default(),
            &[json!({"id":1,"name":"Fixture series","is_series":1,"censored":1})]).unwrap();
        assert_eq!(folders[0].protection, Some(true));
        let (rows, _) = stalker::rows(&profile, &config, "vod", &BTreeMap::new(), &folders[0],
            &[json!({"id":2,"name":"Fixture leaf","cmd":"https://example.invalid/leaf.mp4","censored":0})]).unwrap();
        assert_eq!(rows.entries[0].protection, Some(true));
    }
    #[test]
    fn rpc_catalog_and_portal_children_retain_their_source_flags() {
        use super::super::{stalker, stalker_api};
        let config = stalker::Config::parse(r#"{"portal":"https://example.invalid/stalker_portal/api/","mac":"00:1a:79:01:02:03"}"#).unwrap();
        let source = profile(ProviderKind::Stalker, serde_json::to_string(&config).unwrap());
        let rows = stalker_api::catalog(&source, &config, &json!([
            {"id":1,"name":"Fixture","url":"https://example.invalid/live.m3u8","is_adult":true}
        ])).unwrap();
        assert_eq!(rows.entries[0].protection, Some(true));
        let source = profile(ProviderKind::VPortal, "portal::[key:fixture]https://example.invalid/api".into());
        let page = super::super::parse_vportal_response(&json!({
            "type":"category","title":"Fixture folder","adult":true,"items":[
                {"type":"stream","title":"Fixture leaf","url":"https://example.invalid/leaf.mp4","adult":false}
            ]
        }).to_string(), &source, "").unwrap();
        assert_eq!(page.catalog.entries[0].protection, Some(true));
    }
}
