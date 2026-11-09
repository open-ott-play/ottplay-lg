//! Pure drafts for native Xtream and Stalker setup. No IO or durable mutation.
//!
//! Mount this as a child of `ott_runtime`. The runtime owns the draft's lifetime,
//! profile identity, current-screen admission and asynchronous save receipt.
//! `prepare` is validation, not a commit: keep the draft on validation/write
//! failure and close it only after the matching durable save succeeds. Cancel
//! before submission discards the draft. Leaving an already submitted save must
//! not let its eventual receipt reopen this form or overwrite a newer draft.
//!
//! The source remains the existing private JSON string. Parsing validates it;
//! it does not replace entered endpoint/MAC bytes with the codec's normalized
//! representation. In particular, editing basic fields preserves all optional
//! provider fields. Omitted advanced fields keep the codecs' serde defaults.
use plx_data::ott::{stalker, xtream, ProviderKind, ProviderProfile};
use serde_json::{Map, Value};
use plx_screens::ott::OttTextPolicy;

const PREFIX: &str = "profile.setup.";
const UNKNOWN_FIELD: &str = "This field is not available for this provider";
const INVALID_SOURCE: &str = "Invalid provider credentials";

/// `limit` is a UTF-8 byte bound, not a character count. A scalar editor may
/// additionally limit characters, but must leave the byte check to `stage`.
/// Titles are application copy; the screen must not mark them as server text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Field {
    pub key: &'static str,
    pub title: &'static str,
    pub secret: bool,
    pub limit: usize,
    pub text_policy: OttTextPolicy,
}

const NAME: Field = Field {
    key: "profile.setup.name",
    title: "Profile name",
    secret: false,
    limit: 512,
    text_policy: OttTextPolicy::Predictive,
};
const XTREAM_FIELDS: &[Field] = &[
    NAME,
    Field {
        key: "profile.setup.server",
        title: "Server URL",
        secret: false,
        limit: 8192,
        text_policy: OttTextPolicy::Literal,
    },
    Field {
        key: "profile.setup.username",
        title: "Username",
        secret: false,
        limit: 1024,
        text_policy: OttTextPolicy::Literal,
    },
    Field {
        key: "profile.setup.password",
        title: "Password",
        secret: true,
        limit: 1024,
        text_policy: OttTextPolicy::Literal,
    },
];
const EDEM_FIELDS: &[Field] = &[
    NAME,
    Field {key:"profile.setup.key",title:"Access key",secret:true,limit:512,text_policy:OttTextPolicy::Literal},
    Field {key:"profile.setup.list",title:"List template (0–3)",secret:false,limit:1,text_policy:OttTextPolicy::Literal},
    Field {key:"profile.setup.portal",title:"VPortal link",secret:true,limit:8192,text_policy:OttTextPolicy::Literal},
];
const STALKER_FIELDS: &[Field] = &[
    NAME,
    Field {
        key: "profile.setup.portal",
        title: "Portal URL",
        secret: false,
        limit: 8192,
        text_policy: OttTextPolicy::Literal,
    },
    Field {
        key: "profile.setup.mac",
        title: "MAC address",
        secret: true,
        // Config::parse trims the value before validating the 17-byte MAC.
        // Do not silently change that existing contract to an input length of
        // 17; the enclosing source's 16 KiB bound remains authoritative.
        limit: 16384,
        text_policy: OttTextPolicy::Literal,
    },
];

const OPERATOR_PLAYLIST: Field = Field {key:"profile.setup.playlist",title:"Playlist URL",secret:true,limit:8192,text_policy:OttTextPolicy::Literal};
const OPERATOR_MODE: Field = Field {key:"profile.setup.mode",title:"Stream format (0–2)",secret:false,limit:1,text_policy:OttTextPolicy::Literal};
const OPERATOR_MODE_BINARY: Field = Field {key:"profile.setup.mode",title:"Stream format: 0 HLS, 1 MPEGTS",secret:false,limit:1,text_policy:OttTextPolicy::Literal};
const OPERATOR_LIST: Field = Field {key:"profile.setup.list",title:"Playlist (0–2)",secret:false,limit:1,text_policy:OttTextPolicy::Literal};
const OPERATOR_MAC: Field = Field {key:"profile.setup.mac",title:"Device MAC",secret:true,limit:32,text_policy:OttTextPolicy::Literal};
const OPERATOR_URL_FIELDS: &[Field] = &[NAME,OPERATOR_PLAYLIST];
const OPERATOR_LOGIN_FIELDS: &[Field] = &[NAME,XTREAM_FIELDS[2],XTREAM_FIELDS[3]];
const OPERATOR_MAC_FIELDS: &[Field] = &[NAME,OPERATOR_MAC,OPERATOR_LIST];
const OPERATOR_KEY_FIELDS: &[Field] = &[NAME,XTREAM_FIELDS[2],OPERATOR_MODE];
const SHURA_SERVER: Field = Field {key:"profile.setup.server",title:"Server number (1, 2, 3, 5)",secret:false,limit:1,text_policy:OttTextPolicy::Literal};
const OTTCLUB_SERVER: Field = Field {key:"profile.setup.server",title:"Server host[:port]",secret:false,limit:8192,text_policy:OttTextPolicy::Literal};
const SHURA_FIELDS: &[Field] = &[NAME,SHURA_SERVER,XTREAM_FIELDS[2],OPERATOR_MODE_BINARY];
const OTTCLUB_FIELDS: &[Field] = &[NAME,OTTCLUB_SERVER,XTREAM_FIELDS[2],OPERATOR_MODE_BINARY];
const ANTIFRIZ_FIELDS: &[Field] = &[NAME,XTREAM_FIELDS[2],OPERATOR_MODE_BINARY];
const OPERATOR_GENERIC_FIELDS: &[Field] = &[NAME,XTREAM_FIELDS[1],XTREAM_FIELDS[2],XTREAM_FIELDS[3],OPERATOR_PLAYLIST];
fn operator_fields(id:plx_data::ott::OperatorId)->&'static [Field] {
    match id.canonical_id() {
        "tvteam"=>OPERATOR_URL_FIELDS,
        "1ott"|"shara-tv"=>OPERATOR_LOGIN_FIELDS,
        "kb-team"=>OPERATOR_MAC_FIELDS,
        "only4"|"itv"=>OPERATOR_KEY_FIELDS,
        "antifriz"=>ANTIFRIZ_FIELDS,
        "ottclub"=>OTTCLUB_FIELDS,
        "shura"=>SHURA_FIELDS,
        _=>OPERATOR_GENERIC_FIELDS,
    }
}

#[derive(Clone)]
pub(super) struct Draft {
    profile: ProviderProfile,
    source: Map<String, Value>,
}

impl std::fmt::Debug for Draft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProviderSetupDraft(<private>)")
    }
}

impl Draft {
    /// Empty source means a new profile; otherwise retain the existing source
    /// object after codec validation. The caller supplies a unique identity and
    /// a suitable initial name. Opening/editing does not mutate that caller.
    pub(super) fn new(profile: &ProviderProfile) -> Result<Self, &'static str> {
        let fields = match profile.kind {
            ProviderKind::Xtream => XTREAM_FIELDS,
            ProviderKind::Edem => EDEM_FIELDS,
            ProviderKind::Operator(id) => operator_fields(id),
            ProviderKind::Stalker => STALKER_FIELDS,
            _ => return Err("This provider does not use credential setup"),
        };
        let source = if profile.source.is_empty() {
            fields
                .iter()
                .filter(|field| field.key != NAME.key)
                .map(|field| {
                    (
                        field.key[PREFIX.len()..].to_owned(),
                        Value::String(if (profile.kind == ProviderKind::Edem || matches!(profile.kind,ProviderKind::Operator(_))) && matches!(field.key,"profile.setup.list"|"profile.setup.mode") {"0".into()} else {String::new()}),
                    )
                })
                .collect()
        } else {
            validate_source(profile.kind, &profile.source)?;
            let normalized;
            let source = if profile.kind == ProviderKind::Edem {
                normalized = serde_json::to_string(&plx_data::ott::edem::Config::parse(&profile.source)
                    .map_err(|_| INVALID_SOURCE)?).map_err(|_| INVALID_SOURCE)?;
                &normalized
            } else if let ProviderKind::Operator(id)=profile.kind {
                normalized=serde_json::to_string(&plx_data::ott::operators::Config::parse(id,&profile.source).map_err(|_|INVALID_SOURCE)?).map_err(|_|INVALID_SOURCE)?;
                &normalized
            } else { &profile.source };
            serde_json::from_str::<Value>(source)
                .map_err(|_| INVALID_SOURCE)?
                .as_object()
                .cloned()
                .map(|mut source| { if profile.kind == ProviderKind::Edem {
                    let list=source.get("list").and_then(Value::as_u64).unwrap_or(0);
                    source.insert("list".into(),Value::String(list.to_string()));
                    source.entry("portal").or_insert(Value::String(String::new()));
                }
                if matches!(profile.kind,ProviderKind::Operator(_)) {
                    for field in ["list","mode"] {let value=source.get(field).and_then(Value::as_u64).unwrap_or(0);source.insert(field.into(),Value::String(value.to_string()));}
                }
                source })
                .ok_or(INVALID_SOURCE)?
        };
        Ok(Self {
            profile: profile.clone(),
            source,
        })
    }

    pub(super) fn kind(&self) -> ProviderKind {
        self.profile.kind
    }

    /// Immutable local identity, not a credential or a provider account ID.
    /// The runtime can bind this target even while its draft is incomplete.
    pub(super) fn profile_id(&self) -> &str {
        &self.profile.id
    }

    pub(super) fn fields(&self) -> &'static [Field] {
        match self.profile.kind {
            ProviderKind::Xtream => XTREAM_FIELDS,
            ProviderKind::Edem => EDEM_FIELDS,
            ProviderKind::Operator(id) => operator_fields(id),
            ProviderKind::Stalker => STALKER_FIELDS,
            // Only new() constructs a Draft, after the provider check.
            _ => &[],
        }
    }

    fn field(&self, key: &str) -> Result<&'static Field, &'static str> {
        self.fields()
            .iter()
            .find(|field| field.key == key)
            .ok_or(UNKNOWN_FIELD)
    }

    /// This is private scalar-editor input, never a public snapshot or log.
    pub(super) fn value(&self, key: &str) -> Result<&str, &'static str> {
        self.field(key)?;
        if key == NAME.key {
            Ok(&self.profile.name)
        } else {
            self.source
                .get(&key[PREFIX.len()..])
                .and_then(Value::as_str)
                .ok_or(INVALID_SOURCE)
        }
    }

    /// Setup-list readouts do not expose endpoints, account names or passwords.
    /// "Set" means a draft value is present, not that an account was verified.
    pub(super) fn hint(&self, key: &str) -> Result<&'static str, &'static str> {
        Ok(if self.value(key)?.is_empty() {
            "Not set"
        } else {
            "Set"
        })
    }

    /// Stage bounded input without trimming credentials or normalizing URLs.
    /// Incomplete/invalid values remain editable until `prepare` validates the
    /// full profile. Unknown fields and over-limit input leave the draft intact.
    pub(super) fn stage(&mut self, key: &str, value: &str) -> Result<(), &'static str> {
        if value.len() > self.field(key)?.limit {
            return Err("This value is too long");
        }
        if key == NAME.key {
            self.profile.name = value.to_owned();
        } else {
            self.source.insert(
                key[PREFIX.len()..].to_owned(),
                Value::String(value.to_owned()),
            );
        }
        Ok(())
    }

    /// Produce a candidate for the existing verified writer. Neither the draft
    /// nor any saved profile changes on success or failure of this operation.
    pub(super) fn prepare(&self) -> Result<ProviderProfile, &'static str> {
        let mut candidate = self.profile.clone();
        let mut source = self.source.clone();
        if self.profile.kind == ProviderKind::Edem {
            let list = source.get("list").and_then(Value::as_str).and_then(|s| s.parse::<u8>().ok()).filter(|n|*n<=3).ok_or("Select list template 0 to 3")?;
            source.insert("list".into(), Value::from(list));
        }
        if matches!(self.profile.kind,ProviderKind::Operator(_)) {
            for field in ["list","mode"] {
                if let Some(value)=source.get(field) { let value=value.as_str().and_then(|text|text.parse::<u8>().ok()).ok_or("Invalid operator format or playlist")?;source.insert(field.into(),Value::from(value)); }
            }
        }
        candidate.source = serde_json::to_string(&source).map_err(|_| INVALID_SOURCE)?;
        candidate
            .validate()
            .map_err(|_| "Check the profile name and provider credentials")?;
        Ok(candidate)
    }
}

fn validate_source(kind: ProviderKind, source: &str) -> Result<(), &'static str> {
    // ProviderProfile's bound also applies before cloning a supplied object.
    if source.len() > 16384 || source.chars().any(char::is_control) {
        return Err(INVALID_SOURCE);
    }
    let valid = match kind {
        ProviderKind::Xtream => xtream::Config::parse(source).is_ok(),
        ProviderKind::Edem => plx_data::ott::edem::Config::parse(source).is_ok(),
        ProviderKind::Operator(id) => plx_data::ott::operators::Config::parse(id,source).is_ok(),
        ProviderKind::Stalker => stalker::Config::parse(source).is_ok(),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(INVALID_SOURCE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn profile(kind: ProviderKind, source: &str) -> ProviderProfile {
        ProviderProfile {
            id: "owned-profile".into(),
            name: "Private name".into(),
            kind,
            source: source.into(),
            source_alias: "Original alias".into(),
            enabled: true,
        }
    }

    fn xtream_draft() -> Draft {
        let mut draft = Draft::new(&profile(ProviderKind::Xtream, "")).unwrap();
        draft
            .stage("profile.setup.server", "https://tv.invalid/base/")
            .unwrap();
        draft.stage("profile.setup.username", "user").unwrap();
        draft.stage("profile.setup.password", "password").unwrap();
        draft
    }

    fn json_source(profile: &ProviderProfile) -> Value {
        serde_json::from_str(&profile.source).unwrap()
    }

    #[test]
    fn new_xtream_setup_uses_native_defaults_without_changing_original() {
        let original = profile(ProviderKind::Xtream, "");
        let mut draft = Draft::new(&original).unwrap();
        assert_eq!(draft.kind(), ProviderKind::Xtream);
        assert_eq!(draft.profile_id(), original.id);
        assert_eq!(draft.fields().len(), 4);
        assert_eq!(draft.hint("profile.setup.password"), Ok("Not set"));
        assert!(draft.prepare().is_err());
        draft
            .stage("profile.setup.server", "https://tv.invalid/base/")
            .unwrap();
        draft.stage("profile.setup.username", "user").unwrap();
        draft.stage("profile.setup.password", "password").unwrap();
        draft.stage("profile.setup.name", "Family TV").unwrap();
        let candidate = draft.prepare().unwrap();
        assert_eq!(candidate.id, original.id);
        assert_eq!(candidate.source_alias, original.source_alias);
        assert_eq!(candidate.enabled, original.enabled);
        assert_eq!(candidate.name, "Family TV");
        assert_eq!(draft.kind(), ProviderKind::Xtream);
        assert_eq!(draft.profile_id(), original.id);
        assert_eq!(original.name, "Private name");
        assert!(original.source.is_empty());
        assert!(json_source(&candidate).get("output").is_none());
        assert_eq!(
            xtream::Config::parse(&candidate.source).unwrap().output,
            "m3u8"
        );
        assert_eq!(draft.hint("profile.setup.password"), Ok("Set"));
    }

    #[test]
    fn invalid_draft_is_retained_correctable_and_never_partially_saved() {
        let mut draft = xtream_draft();
        draft
            .stage("profile.setup.server", "https://tv.invalid/?bad=1")
            .unwrap();
        draft.stage("profile.setup.password", "").unwrap();
        assert!(draft.prepare().is_err());
        assert_eq!(
            draft.value("profile.setup.server"),
            Ok("https://tv.invalid/?bad=1")
        );
        assert_eq!(draft.value("profile.setup.password"), Ok(""));
        assert_eq!(draft.value("profile.setup.username"), Ok("user"));
        draft
            .stage("profile.setup.server", "https://tv.invalid/")
            .unwrap();
        draft.stage("profile.setup.password", "corrected").unwrap();
        assert!(draft.prepare().is_ok());
        // prepare is repeatable and does not consume an otherwise valid draft.
        assert_eq!(draft.prepare().unwrap(), draft.prepare().unwrap());
    }

    #[test]
    fn editing_xtream_preserves_optional_output_and_original_endpoint_bytes() {
        let source = json!({"server":"https://tv.invalid/base/player_api.php/",
            "username":"before","password":"untouched","output":"ts"})
        .to_string();
        let original = profile(ProviderKind::Xtream, &source);
        let mut draft = Draft::new(&original).unwrap();
        draft.stage("profile.setup.username", "after").unwrap();
        let candidate = draft.prepare().unwrap();
        let raw = json_source(&candidate);
        assert_eq!(raw["output"], "ts");
        assert_eq!(raw["password"], "untouched");
        assert_eq!(raw["server"], "https://tv.invalid/base/player_api.php/");
        assert_eq!(
            xtream::Config::parse(&candidate.source).unwrap().server,
            "https://tv.invalid:443/base"
        );
        assert_eq!(original.source, source);
    }

    #[test]
    fn passwords_preserve_spaces_unicode_quotes_and_url_reserved_bytes() {
        let mut draft = xtream_draft();
        let password = "  café/猫?&\"\\  ";
        draft.stage("profile.setup.password", password).unwrap();
        draft.stage("profile.setup.username", " username ").unwrap();
        let candidate = draft.prepare().unwrap();
        let config = xtream::Config::parse(&candidate.source).unwrap();
        assert_eq!(config.password, password);
        assert_eq!(config.username, " username ");
        assert_eq!(json_source(&candidate)["password"], password);
    }

    #[test]
    fn field_bounds_count_unicode_bytes_and_rejection_retains_prior_value() {
        let mut draft = xtream_draft();
        let at_limit = "é".repeat(512);
        draft.stage("profile.setup.password", &at_limit).unwrap();
        assert!(draft.prepare().is_ok());
        assert!(draft
            .stage("profile.setup.password", &"é".repeat(513))
            .is_err());
        assert_eq!(draft.value("profile.setup.password"), Ok(at_limit.as_str()));
        draft.stage("profile.setup.name", &"é".repeat(256)).unwrap();
        assert!(draft.prepare().is_ok());
        assert!(draft.stage("profile.setup.name", &"é".repeat(257)).is_err());
    }

    #[test]
    fn invalid_controls_are_not_silently_stripped_from_credentials() {
        let mut draft = xtream_draft();
        for invalid in ["line\nbreak", "nul\0byte", "carriage\rreturn"] {
            draft.stage("profile.setup.password", invalid).unwrap();
            assert!(draft.prepare().is_err());
            assert_eq!(draft.value("profile.setup.password"), Ok(invalid));
        }
        draft.stage("profile.setup.password", "repaired").unwrap();
        assert!(draft.prepare().is_ok());
    }

    #[test]
    fn new_stalker_setup_uses_native_defaults_and_retains_mac_input() {
        let mut draft = Draft::new(&profile(ProviderKind::Stalker, "")).unwrap();
        assert_eq!(draft.kind(), ProviderKind::Stalker);
        assert_eq!(draft.profile_id(), "owned-profile");
        assert_eq!(draft.fields().len(), 3);
        draft
            .stage("profile.setup.portal", "https://portal.invalid/c/")
            .unwrap();
        draft
            .stage("profile.setup.mac", " 00:1a:79:00:00:01 ")
            .unwrap();
        let candidate = draft.prepare().unwrap();
        let raw = json_source(&candidate);
        assert_eq!(raw.as_object().unwrap().len(), 2);
        assert_eq!(raw["mac"], " 00:1a:79:00:00:01 ");
        let config = stalker::Config::parse(&candidate.source).unwrap();
        assert_eq!(config.mac, "00:1A:79:00:00:01");
        assert_eq!(config.stb_type, "MAG250");
        assert_eq!(config.language, "en");
        assert_eq!(config.timezone, "UTC");
        assert!(config.include_vod);
    }

    #[test]
    fn invalid_stalker_portal_and_mac_drafts_survive_until_corrected() {
        let mut draft = Draft::new(&profile(ProviderKind::Stalker, "")).unwrap();
        let portal = "https://portal.invalid/c/";
        let mac = "00:1a:79:00:00:01";
        draft.stage("profile.setup.mac", mac).unwrap();
        for invalid in [
            "https://portal.invalid/api?token=1",
            "https://portal.invalid/c/#fragment",
            "file:///portal",
            "ftp://portal.invalid/c/",
            "portal.invalid/c/",
        ] {
            draft.stage("profile.setup.portal", invalid).unwrap();
            assert!(draft.prepare().is_err());
            assert_eq!(draft.value("profile.setup.portal"), Ok(invalid));
            assert_eq!(draft.value("profile.setup.mac"), Ok(mac));
            draft.stage("profile.setup.portal", portal).unwrap();
            assert!(draft.prepare().is_ok());
        }
        for invalid in [
            "",
            "00:1a:79:00:00",
            "00:1a:79:00:00:0g",
            "00-1a-79-00-00-01",
            "001a79000001",
            "00:1a:79:00:00:001",
        ] {
            draft.stage("profile.setup.mac", invalid).unwrap();
            assert!(draft.prepare().is_err());
            assert_eq!(draft.value("profile.setup.mac"), Ok(invalid));
            assert_eq!(draft.value("profile.setup.portal"), Ok(portal));
            assert_eq!(draft.kind(), ProviderKind::Stalker);
            assert_eq!(draft.profile_id(), "owned-profile");
            draft.stage("profile.setup.mac", mac).unwrap();
            assert!(draft.prepare().is_ok());
        }
    }

    #[test]
    fn supported_stalker_api_keeps_the_entered_draft_editable() {
        let original = profile(ProviderKind::Stalker, "");
        let mut draft = Draft::new(&original).unwrap();
        let mac = "00:1a:79:00:00:01";
        draft.stage("profile.setup.mac", mac).unwrap();
        for portal in [
            "https://portal.invalid/service/api",
            "https://portal.invalid/service/api///",
        ] {
            draft.stage("profile.setup.portal", portal).unwrap();
            let candidate = draft.prepare().unwrap();
            assert_eq!(json_source(&candidate)["portal"], portal);
            assert_eq!(json_source(&candidate)["mac"], mac);
            assert_eq!(stalker::Config::parse(&candidate.source).unwrap().dialect(),
                stalker::Dialect::JsonRpc);
            assert_eq!(draft.value("profile.setup.portal"), Ok(portal));
            assert_eq!(draft.value("profile.setup.mac"), Ok(mac));
        }
        let supported = "https://portal.invalid/service/c/";
        draft.stage("profile.setup.portal", supported).unwrap();
        let candidate = draft.prepare().unwrap();
        assert_eq!(json_source(&candidate)["portal"], supported);
        assert_eq!(json_source(&candidate)["mac"], mac);
        assert!(original.source.is_empty());
    }

    #[test]
    fn editing_stalker_preserves_all_optional_device_and_protocol_fields() {
        let source = json!({"portal":"https://portal.invalid/c/","mac":"00:1a:79:00:00:01",
            "stb_type":"MAG254","language":"fr","timezone":"Europe/Paris","include_vod":false,
            "sn":"serial","device_id":"device-one","device_id2":"device-two",
            "signature":"signature","ver":"version","image_version":"image","hw_version":"hw"});
        let original = profile(ProviderKind::Stalker, &source.to_string());
        let mut draft = Draft::new(&original).unwrap();
        draft
            .stage("profile.setup.portal", "https://second.invalid/c/")
            .unwrap();
        draft
            .stage("profile.setup.mac", "00:1a:79:00:00:02")
            .unwrap();
        let candidate = draft.prepare().unwrap();
        let mut expected = source.clone();
        expected["portal"] = json!("https://second.invalid/c/");
        expected["mac"] = json!("00:1a:79:00:00:02");
        assert_eq!(json_source(&candidate), expected);
        assert_eq!(json_source(&original), source);
    }

    #[test]
    fn fields_cannot_cross_providers_or_set_advanced_or_runtime_actions() {
        let mut draft = xtream_draft();
        let before = draft.prepare().unwrap();
        for key in [
            "profile.setup.portal",
            "profile.setup.mac",
            "profile.setup.output",
            "profile.setup.id",
            "profile.setup.save",
            "profile.setup.cancel",
            "username",
            "",
        ] {
            assert!(draft.stage(key, "untrusted").is_err());
            assert!(draft.value(key).is_err());
            assert!(draft.hint(key).is_err());
        }
        assert_eq!(draft.prepare().unwrap(), before);
        let mut stalker = Draft::new(&profile(ProviderKind::Stalker, "")).unwrap();
        assert!(stalker
            .stage("profile.setup.server", "https://tv.invalid")
            .is_err());
        assert!(stalker.stage("profile.setup.username", "user").is_err());
        assert!(Draft::new(&profile(ProviderKind::M3u, "https://tv.invalid/list.m3u")).is_err());
    }

    #[test]
    fn setup_reuses_url_validation_without_accepting_credential_urls() {
        let mut draft = xtream_draft();
        for url in [
            "tv.invalid",
            "file:///secret",
            "https://u:p@tv.invalid/",
            "https://tv.invalid/?username=u&password=p",
            "https://tv.invalid/#fragment",
            " https://tv.invalid/",
            "https://tv.invalid/white space",
        ] {
            draft.stage("profile.setup.server", url).unwrap();
            assert!(draft.prepare().is_err(), "invalid endpoint accepted");
            assert_eq!(draft.value("profile.setup.server"), Ok(url));
        }
        for suffix in ["player_api.php", "get.php", "xmltv.php"] {
            let url = format!("https://tv.invalid/base/{suffix}/");
            draft.stage("profile.setup.server", &url).unwrap();
            let candidate = draft.prepare().unwrap();
            assert_eq!(json_source(&candidate)["server"], url);
            assert_eq!(
                xtream::Config::parse(&candidate.source).unwrap().server,
                "https://tv.invalid:443/base"
            );
        }
    }

    #[test]
    fn malformed_existing_source_is_not_replaced_with_an_empty_profile() {
        for source in [
            "not-json",
            "[]",
            "null",
            "{}",
            r#"{"server":"https://tv.invalid","username":"u","password":"p","unexpected":true}"#,
        ] {
            assert!(Draft::new(&profile(ProviderKind::Xtream, source)).is_err());
        }
    }

    #[test]
    fn hints_and_debug_never_expose_private_values() {
        let draft = xtream_draft();
        assert_eq!(format!("{draft:?}"), "ProviderSetupDraft(<private>)");
        for field in draft.fields() {
            assert_eq!(draft.hint(field.key), Ok("Set"));
        }
        assert!(draft.field("profile.setup.password").unwrap().secret);
        let stalker = Draft::new(&profile(ProviderKind::Stalker, "")).unwrap();
        assert!(stalker.field("profile.setup.mac").unwrap().secret);
    }
    #[test]
    fn every_setup_field_declares_its_text_semantics() {
        use plx_screens::ott::OttTextPolicy::{Literal, Predictive};
        let fields = |kind| {
            Draft::new(&profile(kind, "")).unwrap().fields().iter()
                .map(|field| (field.key, field.text_policy)).collect::<Vec<_>>()
        };
        assert_eq!(fields(ProviderKind::Xtream), vec![
            ("profile.setup.name", Predictive),
            ("profile.setup.server", Literal),
            ("profile.setup.username", Literal),
            ("profile.setup.password", Literal),
        ]);
        assert_eq!(fields(ProviderKind::Stalker), vec![
            ("profile.setup.name", Predictive),
            ("profile.setup.portal", Literal),
            ("profile.setup.mac", Literal),
        ]);
    }

}

#[cfg(test)]
mod edem_setup_tests {
    use super::*;
    #[test]
    fn edem_legacy_edit_retains_list_and_portal_and_discards_saved_endpoint() {
        let profile=ProviderProfile{id:"edem-1".into(),name:"Edem".into(),kind:ProviderKind::Edem,
            source:r#"{"edkey":"old_key","edlist":"3","vpurl":"portal::[key:vod]https://portal.invalid/api","edcdn":"old.invalid"}"#.into(),source_alias:String::new(),enabled:true};
        let mut draft=Draft::new(&profile).unwrap();
        assert_eq!(draft.value("profile.setup.list").unwrap(),"3");
        draft.stage("profile.setup.key","new_key").unwrap();
        let saved=draft.prepare().unwrap();
        let config=plx_data::ott::edem::Config::parse(&saved.source).unwrap();
        assert_eq!(config.key,"new_key");assert_eq!(config.list,3);
        assert_eq!(config.portal,"portal::[key:vod]https://portal.invalid/api");
        assert!(!saved.source.contains("old.invalid"));
    }
    #[test]
    fn edem_form_saves_numeric_template_and_keeps_missing_key_editable() {
        let p=ProviderProfile{id:"edem-1".into(),name:"Edem".into(),kind:ProviderKind::Edem,source:String::new(),source_alias:String::new(),enabled:true};
        let mut draft=Draft::new(&p).unwrap();
        assert!(draft.prepare().is_err());
        draft.stage("profile.setup.key","test_key").unwrap();
        draft.stage("profile.setup.list","3").unwrap();
        let saved=draft.prepare().unwrap();
        let config=plx_data::ott::edem::Config::parse(&saved.source).unwrap();
        assert_eq!(config.list,3);
        assert!(Draft::new(&saved).unwrap().prepare().is_ok());
        assert!(!draft.fields().iter().any(|field|field.key.contains("cdn")));
        draft.stage("profile.setup.list","9").unwrap();
        assert!(draft.prepare().is_err());
    }
}
