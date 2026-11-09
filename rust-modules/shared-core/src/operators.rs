//! Pure canonical operator plans. The native caller owns transport admission,
//! credentials, cancellation, catalogue publication and playback identity.
//! No provider script, DOM, network callback or module loader enters this VM.
use super::{Error, Planner, PLANNER};
use rquickjs::Function;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const INPUT_BYTES: usize = 2 * 1024 * 1024;
const OUTPUT_BYTES: usize = 8 * 1024 * 1024;
const URL_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Serialize)]
pub enum NamedProfile {
    #[serde(rename = "1ott")]
    OneOtt,
    #[serde(rename = "only4")]
    Only4,
    #[serde(rename = "shara-tv")]
    SharaTv,
    #[serde(rename = "tvteam")]
    Tvteam,
    #[serde(rename = "antifriz")]
    Antifriz,
    #[serde(rename = "kb-team")]
    KbTeam,
    #[serde(rename = "edem")]
    Edem,
    #[serde(rename = "shura")]
    Shura,
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Account,
    Playlist,
    Categories,
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CatalogProfile {
    Itv,
    Ottclub,
    Shura,
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamProfile {
    Itv,
    Ottclub,
    Shura,
    Antifriz,
    Only4,
    Edem,
}
#[derive(Clone, Copy, Deserialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "UPPERCASE")]
pub enum SourceAction {
    Api,
    Playlist,
    Configure,
}

fn with_planner<T>(call: impl FnOnce(&Planner) -> Result<T, Error>) -> Result<T, Error> {
    PLANNER.with(|slot| {
        let mut planner = slot
            .try_borrow_mut()
            .map_err(|_| Error::RuntimeUnavailable)?;
        if planner.is_none() {
            *planner = Some(Planner::new()?);
        }
        call(planner.as_ref().ok_or(Error::RuntimeUnavailable)?)
    })
}
fn object(value: &Value) -> Result<(), Error> {
    if value.is_object() {
        Ok(())
    } else {
        Err(Error::InvalidInput)
    }
}
/// A plan is still untrusted input to the HTTP owner. In particular the owner
/// must apply its own URL/redirect/local-network and stale-generation policy.
fn url(value: Value) -> Result<String, Error> {
    let url = value.as_str().ok_or(Error::InvalidResult)?;
    if url.is_empty()
        || url.len() > URL_BYTES
        || url.chars().any(char::is_control)
        || !(url.starts_with("http://") || url.starts_with("https://"))
    {
        return Err(Error::InvalidResult);
    }
    Ok(url.into())
}
impl Planner {
    fn operator_call(&self, method: &'static str, args: Value) -> Result<Value, Error> {
        let input = serde_json::to_vec(&args).map_err(|_| Error::InvalidInput)?;
        if input.len() > INPUT_BYTES {
            return Err(Error::InvalidInput);
        }
        let result = self.context.with(|ctx| {
            let call = || -> rquickjs::Result<String> {
                let adapter: Function = ctx.eval("(function(method,args){return JSON.stringify(OttPlayCore[method].apply(OttPlayCore,args));})")?;
                adapter.call((method, ctx.json_parse(input)?))
            };
            call().map_err(|_| { let _ = ctx.catch(); Error::InvalidInput })
        })?;
        if result.len() > OUTPUT_BYTES {
            return Err(Error::InvalidResult);
        }
        serde_json::from_str(&result).map_err(|_| Error::InvalidResult)
    }
    fn operator_catalog(
        &self,
        profile: CatalogProfile,
        response: &Value,
        ids: &[String],
    ) -> Result<Value, Error> {
        if ids.len() > 50_000 {
            return Err(Error::InvalidInput);
        }
        let input = serde_json::to_vec(&json!([profile, response, ids]))
            .map_err(|_| Error::InvalidInput)?;
        if input.len() > INPUT_BYTES {
            return Err(Error::InvalidInput);
        }
        let result = self.context.with(|ctx| {
            let call = || -> rquickjs::Result<String> {
                // A fresh reducer owns this response. No catalogue, token or
                // source state is retained in the shared per-thread VM.
                let adapter: Function = ctx.eval("(function(args){var c=new OttPlayCore.OperatorCatalogClient(args[0]);c.accept(args[1],args[2]);return JSON.stringify(c.result());})")?;
                adapter.call((ctx.json_parse(input)?,))
            };
            call().map_err(|_| { let _ = ctx.catch(); Error::InvalidInput })
        })?;
        if result.len() > OUTPUT_BYTES {
            return Err(Error::InvalidResult);
        }
        let value: Value = serde_json::from_str(&result).map_err(|_| Error::InvalidResult)?;
        if !value["ids"].is_array()
            || !value["channels"].is_object()
            || !value["groups"].is_object()
        {
            return Err(Error::InvalidResult);
        }
        Ok(value)
    }
}

pub fn credentials_valid(profile: NamedProfile, first: &str, second: &str) -> Result<bool, Error> {
    let result = with_planner(|planner| {
        planner.operator_call("operatorCredentialsValid", json!([profile, first, second]))
    })?;
    result.as_bool().ok_or(Error::InvalidResult)
}
pub fn profile_url(profile: NamedProfile, phase: Phase, config: &Value) -> Result<String, Error> {
    object(config)?;
    url(with_planner(|planner| {
        planner.operator_call("operatorProfileUrl", json!([profile, phase, config]))
    })?)
}
pub fn source_action(config: &Value) -> Result<SourceAction, Error> {
    object(config)?;
    serde_json::from_value(with_planner(|planner| {
        planner.operator_call("operatorSourceAction", json!([config]))
    })?)
    .map_err(|_| Error::InvalidResult)
}
pub fn catalog(profile: CatalogProfile, response: &Value, ids: &[String]) -> Result<Value, Error> {
    with_planner(|planner| planner.operator_catalog(profile, response, ids))
}
pub fn live_url(
    profile: StreamProfile,
    id: &str,
    channel: &Value,
    options: &Value,
) -> Result<String, Error> {
    object(channel)?;
    object(options)?;
    url(with_planner(|planner| {
        planner.operator_call("operatorLiveUrl", json!([profile, id, channel, options]))
    })?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn named_operator_plans_are_canonical_and_keep_credentials_as_data() {
        assert!(credentials_valid(NamedProfile::SharaTv, "12345678", "abcdefgh").unwrap());
        assert!(!credentials_valid(NamedProfile::SharaTv, "short", "abcdefgh").unwrap());
        assert_eq!(
            profile_url(
                NamedProfile::Antifriz,
                Phase::Playlist,
                &json!({"key":"test-key"})
            )
            .unwrap(),
            "http://af-play.com/playlist/test-key.m3u8"
        );
        assert_eq!(
            profile_url(
                NamedProfile::Only4,
                Phase::Playlist,
                &json!({"token":"0123456789"})
            )
            .unwrap(),
            "http://only4.tv/pl/0123456789/102/only4tv.m3u8"
        );
        assert_eq!(
            profile_url(
                NamedProfile::KbTeam,
                Phase::Playlist,
                &json!({"mac":"fixture", "list":1})
            )
            .unwrap(),
            "http://kb-team.club/?do=/plugin&bid=federaltv&m3u&box_mac=fixture"
        );
        let url = "https://example.invalid/\";throw new Error('injected');\"";
        assert_eq!(
            profile_url(NamedProfile::Tvteam, Phase::Playlist, &json!({"url":url})).unwrap(),
            url
        );
    }
    #[test]
    fn operator_source_plan_and_media_resolve_share_the_canonical_core() {
        assert_eq!(
            source_action(&json!({"server":"https://example.invalid", "user":"u", "pass":"p"}))
                .unwrap(),
            SourceAction::Api
        );
        assert_eq!(
            source_action(&json!({"m3u":"https://example.invalid/list"})).unwrap(),
            SourceAction::Playlist
        );
        assert_eq!(source_action(&json!({})).unwrap(), SourceAction::Configure);
        assert_eq!(
            live_url(
                StreamProfile::Ottclub,
                "42",
                &json!({}),
                &json!({"server":"example.invalid","key":"fixture"})
            )
            .unwrap(),
            "http://example.invalid/stream/fixture/42.m3u8"
        );
    }
    #[test]
    fn catalog_reducer_does_not_retain_another_sources_rows() {
        let input = json!({"channels":[{"ch_id":"42", "channel_name":"Fixture", "cat_name":"News", "rec_time":12, "server_cdn":"example.invalid", "token":"fixture"}]});
        let rows = catalog(CatalogProfile::Itv, &input, &[]).unwrap();
        assert_eq!(rows["ids"], json!(["42"]));
        assert_eq!(rows["channels"]["42"]["channel_name"], "Fixture");
        let empty = catalog(CatalogProfile::Itv, &json!({"channels":[]}), &[]).unwrap();
        assert_eq!(empty["ids"], json!([]));
        assert!(empty["channels"].as_object().unwrap().is_empty());
    }
    #[test]
    fn malformed_and_oversize_requests_fail_without_vm_exception_details() {
        assert!(source_action(&json!([])).is_err());
        assert!(profile_url(
            NamedProfile::Tvteam,
            Phase::Playlist,
            &json!({"url":"file:///private"})
        )
        .is_err());
        assert!(profile_url(
            NamedProfile::Tvteam,
            Phase::Playlist,
            &json!({"url":"https://example.invalid/\nsecret"})
        )
        .is_err());
        assert!(source_action(&json!({"m3u":"x".repeat(INPUT_BYTES)})).is_err());
        assert!(catalog(CatalogProfile::Itv, &json!({"channels":{}}), &[]).is_err());
        assert_eq!(source_action(&json!({})).unwrap(), SourceAction::Configure);
    }
}

/// Stable identity/credential-schema metadata only. Registry membership does not
/// assert that a native transport has already been implemented for that kind.
#[derive(Clone, Copy, Debug)]
pub struct OperatorProfile {
    pub id: &'static str,
    pub kind: &'static str,
    pub legacy_prefix: &'static str,
    pub title: &'static str,
}
#[path = "operators_registry.rs"]
mod registry;
pub use registry::{OperatorId, PROFILES};
pub fn profile(id: &str) -> Option<&'static OperatorProfile> {
    PROFILES.iter().find(|profile| profile.id == id)
}
#[derive(Clone, Copy, Debug)]
pub struct CredentialField {
    pub name: &'static str,
    pub legacy_key: &'static str,
    pub secret: bool,
}
const GENERIC_FIELDS: &[CredentialField] = &[
    CredentialField {
        name: "m3u",
        legacy_key: "cfg.m3u",
        secret: true,
    },
    CredentialField {
        name: "server",
        legacy_key: "cfg.server",
        secret: false,
    },
    CredentialField {
        name: "user",
        legacy_key: "cfg.user",
        secret: true,
    },
    CredentialField {
        name: "pass",
        legacy_key: "cfg.pass",
        secret: true,
    },
];
const NAMED_FIELDS: &[CredentialField] = &[
    CredentialField {
        name: "username",
        legacy_key: "login",
        secret: true,
    },
    CredentialField {
        name: "password",
        legacy_key: "pass",
        secret: true,
    },
];
impl OperatorProfile {
    pub fn generic(&self) -> bool {
        matches!(self.kind, "operator" | "xtream-fallback")
    }
    pub fn credential_fields(&self) -> &'static [CredentialField] {
        if self.generic() {
            return GENERIC_FIELDS;
        }
        match self.id {
            "1ott" => &[
                CredentialField {
                    name: "username",
                    legacy_key: "id",
                    secret: true,
                },
                CredentialField {
                    name: "password",
                    legacy_key: "pin",
                    secret: true,
                },
            ],
            "only4" => &[CredentialField {
                name: "username",
                legacy_key: "token",
                secret: true,
            }],
            "tvteam" => &[CredentialField {
                name: "playlist",
                legacy_key: "www",
                secret: true,
            }],
            "shara-tv" => NAMED_FIELDS,
            "ottclub" => &[
                CredentialField {
                    name: "username",
                    legacy_key: "ottkey",
                    secret: true,
                },
                CredentialField {
                    name: "server",
                    legacy_key: "ottwww",
                    secret: false,
                },
            ],
            "itv" | "antifriz" => &[CredentialField {
                name: "username",
                legacy_key: "key",
                secret: true,
            }],
            "shura" => &[
                CredentialField {
                    name: "username",
                    legacy_key: "key",
                    secret: true,
                },
                CredentialField {
                    name: "server",
                    legacy_key: "server",
                    secret: false,
                },
            ],
            // These providers have separately owned native schemas, including
            // device-derived identity for kb-team; never fabricate saved fields.
            _ => &[],
        }
    }
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct GenericConfig {
    pub m3u: String,
    pub server: String,
    pub user: String,
    pub pass: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenericPlan {
    pub action: SourceAction,
    pub request: Option<String>,
    pub channels: Vec<Value>,
}
/// Reconstruct a pure canonical request/reducer from the current complete input.
/// `response` is the decoded account API response, never fetched by the VM.
/// A network failure selects the canonical playlist fallback without retrying IO.
pub fn generic_plan(
    id: &str,
    config: &GenericConfig,
    response: Option<&Value>,
    network_failed: bool,
) -> Result<GenericPlan, Error> {
    if !profile(id).is_some_and(OperatorProfile::generic) {
        return Err(Error::InvalidInput);
    }
    let input = serde_json::to_vec(&json!([config, response, network_failed]))
        .map_err(|_| Error::InvalidInput)?;
    if input.len() > INPUT_BYTES {
        return Err(Error::InvalidInput);
    }
    let result = with_planner(|planner| {
        planner.context.with(|ctx| {
        let call = || -> rquickjs::Result<String> {
            let adapter: Function = ctx.eval("(function(args){var config=args[0],response=args[1],failed=args[2],a=OttPlayCore.operatorSourceAction(config);if(a==='CONFIGURE')return JSON.stringify({action:a,request:null,channels:[]});if(a==='PLAYLIST')return JSON.stringify({action:a,request:config.m3u,channels:[]});var c=new OttPlayCore.OperatorChannelClient(config,encodeURIComponent);if(failed)c.reject();else if(response!==null)c.accept(response);return JSON.stringify({action:c.action(),request:c.request(),channels:!failed&&response!==null&&c.action()==='API'?c.channelCatalog():[]});})")?;
            adapter.call((ctx.json_parse(input)?,))
        };
        call().map_err(|_| { let _ = ctx.catch(); Error::InvalidInput })
    })
    })?;
    if result.len() > OUTPUT_BYTES {
        return Err(Error::InvalidResult);
    }
    let plan: GenericPlan = serde_json::from_str(&result).map_err(|_| Error::InvalidResult)?;
    if let Some(request) = &plan.request {
        url(Value::String(request.clone()))?;
    }
    if plan.channels.len() > 50_000 {
        return Err(Error::InvalidResult);
    }
    Ok(plan)
}

#[cfg(test)]
mod registry_tests {
    use super::*;
    #[test]
    fn registry_has_all_canonical_driver_identities_without_duplicates() {
        let mut ids = std::collections::BTreeSet::new();
        assert_eq!(PROFILES.len(), 52);
        for p in PROFILES {
            assert!(ids.insert(p.id));
        }
        for id in [
            "great",
            "tvteam",
            "bestlist/stalker",
            "edem",
            "shara-tv",
            "vportal",
            "offline",
        ] {
            assert!(profile(id).is_some(), "{id}");
        }
        assert_eq!(profile("shara.club").unwrap().legacy_prefix, "shara_club");
        assert_eq!(
            profile("1ott").unwrap().credential_fields()[1].legacy_key,
            "pin"
        );
        assert!(profile("great")
            .unwrap()
            .credential_fields()
            .iter()
            .any(|f| f.name == "pass" && f.secret));
    }
    #[test]
    fn generic_operator_m3u_and_api_choices_match_canonical_policy() {
        let m3u = GenericConfig {
            m3u: "https://example.invalid/list.m3u".into(),
            ..Default::default()
        };
        let plan = generic_plan("great", &m3u, None, false).unwrap();
        assert_eq!(plan.action, SourceAction::Playlist);
        assert_eq!(plan.request.as_deref(), Some(m3u.m3u.as_str()));
        let cfg = GenericConfig {
            server: "https://example.invalid".into(),
            user: "u&1".into(),
            pass: "p?2".into(),
            ..m3u
        };
        let plan = generic_plan("great", &cfg, None, false).unwrap();
        assert_eq!(plan.action, SourceAction::Api);
        let request = plan.request.unwrap();
        assert!(request.contains("username=u%261"), "{request}");
        assert!(request.contains("password=p%3F2"));
        let fallback = generic_plan("great", &cfg, None, true).unwrap();
        assert_eq!(fallback.action, SourceAction::Playlist);
        assert!(fallback.request.unwrap().contains("get.php"));
        assert!(generic_plan("unregistered", &cfg, None, false).is_err());
        assert!(generic_plan("edem", &cfg, None, false).is_err());
    }
    #[test]
    fn generic_api_catalog_is_reduced_by_core_and_failure_keeps_it_empty() {
        let cfg = GenericConfig {
            server: "https://example.invalid".into(),
            user: "user".into(),
            pass: "pass".into(),
            ..Default::default()
        };
        let response = json!({"categories":[{"category_id":"1","category_name":"News"}],
            "live_streams":[{"stream_id":42,"name":"Fixture","category_id":"1","stream_icon":"","tv_archive":1,"tv_archive_duration":2}]});
        let plan = generic_plan("great", &cfg, Some(&response), false).unwrap();
        assert_eq!(plan.channels.len(), 1);
        let failed = generic_plan("great", &cfg, Some(&response), true).unwrap();
        assert!(failed.channels.is_empty());
        assert_eq!(failed.action, SourceAction::Playlist);
    }
}

/// Parse the canonical named-provider playlist without executing its legacy UI.
/// KbTeam's legacy URL hash is caller-owned: supply the exact URL->hash table.
/// Missing hash inputs fail rather than silently changing portable channel IDs.
pub fn parse_operator_playlist(
    profile: NamedProfile,
    text: &str,
    existing: &[String],
    hashes: &std::collections::BTreeMap<String, u32>,
) -> Result<Value, Error> {
    if existing.len() > 50_000 || hashes.len() > 50_000 {
        return Err(Error::InvalidInput);
    }
    let input = serde_json::to_vec(&json!([profile, text, existing, hashes]))
        .map_err(|_| Error::InvalidInput)?;
    if input.len() > INPUT_BYTES {
        return Err(Error::InvalidInput);
    }
    let result = with_planner(|planner| {
        planner.context.with(|ctx| {
        let call = || -> rquickjs::Result<String> {
            let adapter: Function = ctx.eval("(function(args){return JSON.stringify(OttPlayCore.parseOperatorPlaylist(args[1],args[0],function(url){if(!Object.prototype.hasOwnProperty.call(args[3],url))throw new Error('Missing native hash');return args[3][url];},args[2]));})")?;
            adapter.call((ctx.json_parse(input)?,))
        };
        call().map_err(|_| { let _=ctx.catch(); Error::InvalidInput })
    })
    })?;
    if result.len() > OUTPUT_BYTES {
        return Err(Error::InvalidResult);
    }
    let value: Value = serde_json::from_str(&result).map_err(|_| Error::InvalidResult)?;
    if !value["entries"].is_array() || !value["channels"].is_object() || !value["ids"].is_array() {
        return Err(Error::InvalidResult);
    }
    if value["entries"]
        .as_array()
        .is_some_and(|rows| rows.len() > 50_000)
    {
        return Err(Error::InvalidResult);
    }
    Ok(value)
}
#[cfg(test)]
mod playlist_tests {
    use super::*;
    #[test]
    fn canonical_named_playlist_retains_provider_id_and_archive_metadata() {
        let text="#EXTM3U\n#EXTINF:-1 tvg-name=\"fixture-id\" group-title=\"News\" timeshift=\"1\",Fixture\nhttps://example.invalid/live/42/index.m3u8\n";
        let parsed =
            parse_operator_playlist(NamedProfile::Tvteam, text, &[], &Default::default()).unwrap();
        assert_eq!(parsed["ids"], json!(["fixture-id"]));
        assert_eq!(parsed["channels"]["fixture-id"]["channel_name"], "Fixture");
        assert_eq!(parsed["channels"]["fixture-id"]["rec"], 168);
    }
    #[test]
    fn kbteam_requires_explicit_native_hash_mapping_instead_of_fabricating_ids() {
        let text = "#EXTM3U\n#EXTINF:-1,Fixture\nhttps://example.invalid/channel?token=fixture\n";
        assert!(
            parse_operator_playlist(NamedProfile::KbTeam, text, &[], &Default::default()).is_err()
        );
        let hashes =
            std::collections::BTreeMap::from([("https://example.invalid/channel".into(), 42)]);
        let parsed = parse_operator_playlist(NamedProfile::KbTeam, text, &[], &hashes).unwrap();
        assert_eq!(parsed["ids"], json!([42]));
    }
    #[test]
    fn closed_operator_ids_round_trip_and_cannot_collapse_to_generic_alias() {
        assert_eq!(OperatorId::ALL.len(), 43);
        for id in OperatorId::ALL {
            let wire = serde_json::to_string(id).unwrap();
            assert_eq!(serde_json::from_str::<OperatorId>(&wire).unwrap(), *id);
            assert_eq!(OperatorId::from_id(id.canonical_id()), Some(*id));
        }
        assert!(serde_json::from_str::<OperatorId>("\"operator\"").is_err());
        assert_eq!(
            OperatorId::Antifriz
                .named_profile()
                .map(|p| serde_json::to_value(p).unwrap()),
            Some(json!("antifriz"))
        );
    }
}
