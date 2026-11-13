//! Native implementation of the version-one player-settings wire contract.
//!
//! A revision describes the exact stored value and active source, not a row
//! number. Writes go through the same durable transaction as provider settings;
//! this adapter never starts playback or wakes a suspended player.
use super::*;
use super::super::menu_visibility as menus;
use plx_appkit::menu_visibility::{Item as MenuItem, Change as MenuChange};
#[path = "ott_settings_appearance.rs"]
mod appearance;
#[path = "ott_settings_geometry.rs"]
mod geometry;
use super::super::subtitle_settings::selection;
#[path = "ott_settings_osd.rs"]
mod osd;
#[path = "ott_settings_classic.rs"]
mod classic;

#[derive(Clone, Copy)]
enum Setting {
    NativeInputProbe,
    HostedEpg,
    InfoTimeout,
    FontScale,
    UiColors,
    VolumeStep,
    Player,
    Interface,
    Appearance(appearance::Field),
    Geometry(geometry::Field),
    SubtitleSelection(selection::Field),
    Osd(osd::Field),
    Classic(classic::Field),
    Menu(MenuItem),
    M3uMedia,
    Sleep,
}

impl Setting {
    const ALL: [Self; 45] = [Self::FontScale, Self::Player, Self::Interface,
        Self::Appearance(appearance::Field::ALL[0]), Self::Appearance(appearance::Field::ALL[1]),
        Self::Appearance(appearance::Field::ALL[2]), Self::Appearance(appearance::Field::ALL[3]),
        Self::Menu(MenuItem::Providers), Self::Menu(MenuItem::Profiles), Self::Menu(MenuItem::Catalog),
        Self::Menu(MenuItem::Categories), Self::Menu(MenuItem::Favorites), Self::Menu(MenuItem::Search),
        Self::Menu(MenuItem::Description), Self::Menu(MenuItem::Play), Self::Menu(MenuItem::Favorite),
        Self::Menu(MenuItem::Refresh), Self::Menu(MenuItem::Audio), Self::Menu(MenuItem::Subtitles),
        Self::Menu(MenuItem::Quality), Self::Menu(MenuItem::Speed), Self::Menu(MenuItem::Stats), Self::M3uMedia,
        Self::Geometry(geometry::Field::Size), Self::Geometry(geometry::Field::Position),
        Self::Geometry(geometry::Field::Tone), Self::VolumeStep, Self::Sleep, Self::InfoTimeout,
        Self::Osd(osd::Field::Opacity), Self::Osd(osd::Field::Clock),
        Self::Classic(classic::Field::ALL[0]), Self::Classic(classic::Field::ALL[1]),
        Self::Classic(classic::Field::ALL[2]), Self::Classic(classic::Field::ALL[3]),
        Self::Classic(classic::Field::ALL[4]), Self::Classic(classic::Field::ALL[5]),
        Self::Classic(classic::Field::ALL[6]), Self::Classic(classic::Field::ALL[7]),
        Self::Classic(classic::Field::ALL[8]), Self::UiColors,
        Self::SubtitleSelection(selection::Field::Mode), Self::SubtitleSelection(selection::Field::Language), Self::HostedEpg, Self::NativeInputProbe];

    fn find(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|setting| setting.id() == if id == "sUiColorConfig" { "uiColorConfig" } else { id })
    }

    fn local_choice(id: &str) -> Option<Self> {
        osd::Field::find(id).map(Self::Osd).or_else(||
            classic::Field::ALL.into_iter().find(|field| field.id() == id).map(Self::Classic))
    }

    fn id(self) -> &'static str {
        match self {
            Self::NativeInputProbe => "nativeInputProbeV1",
            Self::HostedEpg => "nativeEpgApiV1",
            Self::InfoTimeout => "infoTimeout",
            Self::FontScale => "uiFontScale",
            Self::UiColors => plx_ui::palette::KEY,
            Self::VolumeStep => "volumeStep",
            Self::Player => "players",
            Self::Interface => "interfaceMode",
            Self::Sleep => "sleepTimeout",
            Self::Appearance(field) => field.id(),
            Self::Geometry(field) => field.id(),
            Self::SubtitleSelection(field) => field.id(),
            Self::Osd(field) => field.id(),
            Self::Classic(field) => field.id(),
            Self::Menu(item) => item.setting(),
            Self::M3uMedia => "m3uMediaUrl",
        }
    }

    fn default(self) -> Value {
        match self {
            Self::NativeInputProbe | Self::HostedEpg | Self::UiColors => json!(""),
            Self::InfoTimeout => json!(plx_screens::player::input::InfoTimeout::DEFAULT.seconds()),
            Self::FontScale => json!(100), Self::VolumeStep => json!(5), Self::Player => json!(0), Self::Interface => json!(0),
            Self::Appearance(field) => field.default_value(),
            Self::Geometry(field) => field.default_value(),
            Self::SubtitleSelection(field) => field.default_value(),
            Self::Osd(field) => field.default_value(),
            Self::Classic(field) => field.default_value(),
            Self::Menu(_) => json!(1),
            Self::M3uMedia | Self::Sleep => json!(0),
        }
    }
    fn storage_key(self) -> &'static str {
        match self { Self::Appearance(_) => appearance::STORAGE_KEY, Self::Menu(_) => menus::KEY, _ => self.id() }
    }
    fn valid(self, value: &Value) -> bool {
        match self {
            Self::NativeInputProbe => value.as_str().is_some_and(|text|text.len()<=4096),
            Self::HostedEpg => hosted_epg_settings::parse(value).is_ok(),
            Self::UiColors => value.as_str().is_some_and(|text| plx_ui::palette::Config::parse(text).is_ok()),
            Self::M3uMedia => value.as_str().is_some_and(|value| value.len() <= 4096
                && (value.trim().is_empty() || plx_data::ott::parse_vportal_link(value).is_ok())),
            Self::Appearance(field) => field.merge(Default::default(), value).is_ok(),
            Self::Geometry(field) => field.edit(value).is_ok(),
            Self::SubtitleSelection(field) => field.valid(value),
            Self::Osd(field) => field.valid(value),
            Self::Classic(field) => field.valid(value),
            Self::Menu(_) => value.as_u64().is_some_and(|value| value <= 1),
            _ => value.as_u64().is_some_and(|number| match self {
                Self::FontScale => u16::try_from(number).ok()
                    .and_then(plx_machine::machine::FontScale::from_percent).is_some(),
                Self::VolumeStep => (3..=10).contains(&number),
                Self::InfoTimeout => plx_screens::player::input::InfoTimeout::from_seconds(number).is_some(),
                Self::Player => number == 0,
                Self::Interface => plx_machine::machine::UiMode::from_index(number).is_some(),
                Self::Sleep => super::super::sleep_timer::Choice::from_index(number).is_some(),
                Self::NativeInputProbe | Self::HostedEpg | Self::UiColors | Self::Appearance(_) | Self::Geometry(_) | Self::SubtitleSelection(_) | Self::Osd(_) | Self::Classic(_) | Self::Menu(_) | Self::M3uMedia => unreachable!(),
            }),
        }
    }

    fn writable(self) -> bool {
        matches!(self, Self::HostedEpg | Self::FontScale | Self::UiColors | Self::InfoTimeout | Self::VolumeStep | Self::Interface | Self::Sleep | Self::M3uMedia | Self::Appearance(_) | Self::Geometry(_) | Self::SubtitleSelection(_) | Self::Osd(_) | Self::Classic(_) | Self::Menu(_))
    }

    fn reason(self) -> &'static str {
        match self {
            Self::HostedEpg | Self::FontScale | Self::UiColors | Self::InfoTimeout | Self::VolumeStep | Self::Interface | Self::Sleep | Self::M3uMedia | Self::Appearance(_) | Self::Geometry(_) | Self::SubtitleSelection(_) | Self::Osd(_) | Self::Classic(_) | Self::Menu(_) => "unsupported",
            Self::NativeInputProbe => "unsupported",
            Self::Player => "device_forced_auto",
        }
    }

    fn descriptor(self) -> Value {
        if matches!(self, Self::NativeInputProbe) {
            return json!({"id":self.id(),"type":"string","scope":"application","sensitive":false,
                "access":{"read":true,"write":false,"reason":"unsupported"},"default_value":"","apply":"immediate",
                "constraints":{"max_utf8_bytes":4096},
                "description":"Last 16 nonprintable physical SDL keys from the existing bounded native playback capture. Empty without a capture; no text or persistent telemetry."});
        }
        if matches!(self, Self::HostedEpg) {
            return json!({"id":self.id(),"type":"string","scope":"application","sensitive":false,
                "access":{"read":true,"write":true},"default_value":"","apply":"immediate",
                "constraints":{"max_utf8_bytes":4096},
                "description":"Optional hosted guide fallback. Empty disables it; otherwise enter JSON with version 1, url ending /epg/v1 and source epg-one. Provider-advertised guides keep priority."});
        }
        if matches!(self, Self::UiColors) {
            return json!({"id":self.id(),"type":"string","scope":"application","sensitive":false,
                "access":{"read":true,"write":true},"default_value":"","apply":"immediate",
                "constraints":{"max_utf8_bytes":plx_ui::palette::MAX_BYTES},
                "description":"Canonical interface palette. Empty inherits the current theme; OSD opacity is separate."});
        }
        if let Self::Appearance(field) = self { return field.descriptor(); }
        if let Self::Geometry(field) = self { return field.descriptor(); }
        if let Self::SubtitleSelection(field) = self { return field.descriptor(); }
        if let Self::Osd(field) = self { return field.descriptor(); }
        if let Self::Classic(field) = self { return field.descriptor(); }
        if let Self::Menu(item) = self { return menus::descriptor(item); }
        if matches!(self, Self::M3uMedia) {
            return json!({"id":self.id(),"type":"string","scope":"provider","sensitive":true,
                "access":{"read":false,"write":true},"constraints":{"max_utf8_bytes":4096},
                "apply":"manual","description":"Private media attachment to the captured M3U profile; used on the next matching Favorite selection."});
        }
        let mut result = json!({
            "id":self.id(),"type":"integer","scope":"application","sensitive":false,
            "access":{"read":true,"write":self.writable()},
            "default_value":self.default(),"apply":"immediate"
        });
        if !self.writable() {
            result["access"]["reason"] = json!(self.reason());
        }
        match self {
            Self::FontScale => {
                result["constraints"] = json!({"minimum":plx_machine::machine::FontScale::MIN_PERCENT,
                    "maximum":plx_machine::machine::FontScale::MAX_PERCENT});
                result["description"] = json!("Interface text scale.");
                result["unit"] = json!("percent");
            }
            Self::VolumeStep => {
                result["constraints"] = json!({"minimum":3,"maximum":10});
                result["description"] = json!("Volume increment for physical desktop playback keys.");
                result["unit"] = json!("percent");
            }
            Self::InfoTimeout => {
                result["constraints"] = json!({"minimum":3,"maximum":20});
                result["description"] = json!("Ordinary player information display time; fixed menus and notices keep their own holds.");
                result["unit"] = json!("seconds");
            }
            Self::Player => {
                result["constraints"] = json!({"values":[0]});
                result["choices"] = json!([{"value":0,"label":"Native","aliases":["native"]}]);
                result["description"] = json!("This installation uses the native media decoder.");
                result["apply"] = json!("next_stream");
            }
            Self::Interface => {
                result["constraints"] = json!({"values":[0,1]});
                result["choices"] = json!([{"value":0,"label":"Classic","aliases":["classic"]},
                    {"value":1,"label":"Modern","aliases":["modern"]}]);
                result["description"] = json!("Interface presentation. Switching preserves playback and the selected source.");
            }
            Self::Sleep => {
                result["constraints"] = json!({"values":[0,1,2,3,4]});
                result["choices"] = json!(super::super::sleep_timer::Choice::ALL.into_iter()
                    .map(|choice| json!({"value":choice.index(),"label":choice.label(),"aliases":[]})).collect::<Vec<_>>());
                result["description"] = json!("Standby after no input. Disabled by kiosk; a new session starts a fresh interval.");
            }
            Self::NativeInputProbe | Self::HostedEpg | Self::UiColors | Self::Appearance(_) | Self::Geometry(_) | Self::SubtitleSelection(_) | Self::Osd(_) | Self::Classic(_) | Self::Menu(_) | Self::M3uMedia => unreachable!(),
        }
        result
    }
}

fn token(value: &Value) -> Option<&str> {
    value.as_str().filter(|text| {
        !text.is_empty()
            && text.len() <= 128
            && text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
    })
}

fn setting_context(saved: &Saved, provider: ProviderKind, runtime: &str, nonce: &[u8]) -> Value {
    let active = saved.profiles.active_profile.as_ref().and_then(|id| {
        saved
            .profiles
            .profiles
            .iter()
            .find(|profile| &profile.id == id && profile.kind == provider)
    });
    let number = active.and_then(|profile| {
        saved
            .settings
            .get(&format!("slot:{}", profile.id))
            .and_then(Value::as_u64)
            .filter(|slot| (1..=15).contains(slot))
            .or_else(|| {
                saved
                    .profiles
                    .profiles
                    .iter()
                    .filter(|p| p.kind == provider)
                    .position(|p| p.id == profile.id)
                    .map(|n| (n + 1) as u64)
                    .filter(|n| *n <= 15)
            })
    });
    json!({
        // The private per-process nonce keeps this source-bound token from
        // being a public oracle for guessing source credentials.
        "token":hash(&json!([nonce,runtime,wire_provider(provider),active,number])),
        "provider":wire_provider(provider),"profile":number
    })
}

fn media_attachment(saved: &Saved) -> Option<crate::app::ott_m3u_attachment::Attachment> {
    let parent=saved.profiles.active_profile.as_ref()?;
    let registry=crate::app::ott_m3u_attachment::Registry::read(&saved.settings,&saved.profiles).ok()?;
    registry.for_parent(parent).cloned()
}

fn media_scope(context: &mut Value, revision: u64, navigation_epoch: u64) {
    context["token"]=json!(hash(&json!([context["token"],revision,navigation_epoch])));
}

fn stored(saved: &Saved, setting: Setting) -> Value {
    if matches!(setting, Setting::NativeInputProbe) {
        return json!({"id":setting.id(),"storage":"default","value":""});
    }
    if matches!(setting, Setting::UiColors) {
        let raw = saved.settings.get(setting.id());
        let decoded = raw.and_then(Value::as_str).and_then(|text| plx_ui::palette::Config::decode(text).ok());
        let mut result = json!({"id":setting.id(),
            "storage":if raw.is_none() {"default"} else if decoded.is_some() {"stored"} else {"invalid"},
            "value":decoded.unwrap_or_default().encode()});
        if decoded.is_some() { result["persisted_value"] = raw.cloned().unwrap(); }
        return result;
    }
    if let Setting::Menu(item) = setting { return menus::snapshot(saved, item); }
    if let Setting::Appearance(field) = setting {
        return field.snapshot(saved.settings.get(appearance::STORAGE_KEY));
    }
    if matches!(setting, Setting::M3uMedia) {
        use crate::app::ott_m3u_attachment::{Registry, SETTINGS_KEY};
        let registry = Registry::read(&saved.settings, &saved.profiles);
        let configured = registry.as_ref().ok().is_some_and(|registry|
            saved.profiles.active_profile.as_deref().is_some_and(|parent|
                registry.for_parent(parent).is_some()));
        // An explicit clear persists a valid, possibly empty registry. Its
        // absence is a default; malformed bytes are never durable-value proof.
        let storage = if registry.is_err() { "invalid" }
            else if saved.settings.contains_key(SETTINGS_KEY) { "stored" }
            else { "default" };
        return json!({"id":setting.id(),"redacted":true,"configured":configured,"storage":storage});
    }
    let raw = saved.settings.get(setting.id());
    let valid = raw.is_some_and(|value| setting.valid(value));
    let mut result = json!({"id":setting.id(),
        "storage":if raw.is_none() {"default"} else if valid {"stored"} else {"invalid"},
        "value":raw.filter(|value|setting.valid(value)).cloned().unwrap_or_else(||json!(setting.default()))
    });
    if let Some(value) = raw.filter(|_| valid) {
        result["persisted_value"] = value.clone();
    }
    result
}

// Existing application settings retain their bytes and revisions. These five
// display choices use the current provider, never the selected profile slot.
fn storage_key_for(setting: Setting, provider: &str) -> String {
    match setting {
        Setting::Classic(field) => field.storage_key(provider),
        _ => setting.storage_key().into(),
    }
}
fn revision_key_for(setting: Setting, provider: &str) -> String {
    if matches!(setting, Setting::Classic(field) if field.provider_scoped()) {
        format!("settingRevision:{}", storage_key_for(setting, provider))
    } else { revision_key(setting) }
}
fn stored_for(saved: &Saved, setting: Setting, provider: &str) -> Value {
    if !matches!(setting, Setting::Classic(field) if field.provider_scoped()) {
        return stored(saved, setting);
    }
    let raw = saved.settings.get(&storage_key_for(setting, provider));
    let valid = raw.is_some_and(|value| setting.valid(value));
    let mut result = json!({"id":setting.id(),
        "storage":if raw.is_none() {"default"} else if valid {"stored"} else {"invalid"},
        "value":raw.filter(|value|setting.valid(value)).cloned().unwrap_or_else(||json!(setting.default()))});
    if let Some(value) = raw.filter(|_| valid) { result["persisted_value"] = value.clone(); }
    result
}
fn signature_for(saved: &Saved, setting: Setting, provider: &str) -> String {
    if !matches!(setting, Setting::Classic(field) if field.provider_scoped()) {
        return signature(saved, setting);
    }
    hash(&json!([setting.id(), provider,
        saved.settings.get(&storage_key_for(setting, provider)),
        saved.settings.get(&revision_key_for(setting, provider))]))
}

fn signature(saved: &Saved, setting: Setting) -> String {
    if matches!(setting, Setting::M3uMedia) {
        return hash(&json!([setting.id(),media_attachment(saved),saved.settings.get(&revision_key(setting))]));
    }
    hash(&json!([
        setting.id(),
        saved.settings.get(setting.storage_key()),
        saved.settings.get(&revision_key(setting))
    ]))
}

fn revision_key(setting: Setting) -> String {
    match setting {
        Setting::Appearance(_) | Setting::Geometry(_) | Setting::SubtitleSelection(_) => super::super::subtitle_settings::REVISION.into(),
        Setting::Menu(_) => menus::KEY.into(),
        _ => format!("settingRevision:{}", setting.id()),
    }
}

fn with_revision(
    receipts: &mut Receipts,
    saved: &Saved,
    setting: Setting,
    context: &Value,
    invocation: &Invocation,
    after_save: bool,
    now: u64,
) -> Value {
    let provider = context["provider"].as_str().unwrap_or_default();
    let mut snapshot = stored_for(saved, setting, provider);
    let identity = format!(
        "setting:{}",
        hash(&json!([invocation.id, setting.id(), after_save]))
    );
    let revision = receipts.record(
        &identity,
        signature_for(saved, setting, provider),
        context["token"].as_str().unwrap_or_default().to_owned(),
        0,
        now,
    );
    snapshot["revision"] = json!(revision);
    snapshot
}

/// Issue the post-write revision from the authoritative public snapshot, whose
/// CAS revision is only known after the storage acknowledgement.
pub(super) fn public_saved(runtime: &mut Runtime, inv: &Invocation, response: &mut Value) {
    if inv.action != "player_settings" || inv.params["op"] != "set"
        || inv.params["id"] != "interfaceMode" { return; }
    let context = response["data"]["context"].clone();
    let now = runtime.now_ms();
    response["data"]["setting"] = with_revision(&mut runtime.command_receipts, &runtime.saved,
        Setting::Interface, &context, inv, true, now);
    response["data"]["setting"]["effective_value"] = json!(runtime.interface_mode().index());
}

pub(super) fn failure(params: &Value, reason: &str) -> Value {
    json!({"status":if reason == "unsupported" {"unsupported"} else {"rejected"},
        "data":{"version":1,"op":params["op"],"runtime":params["runtime"],
            "reason":reason,"error":"The player setting request could not be completed."}})
}

fn schema_page(params: &Value, context: &Value) -> Result<(usize, usize), &'static str> {
    if !subset(params, &["op", "runtime", "limit", "cursor"]) {
        return Err("invalid_request");
    }
    let limit = match params.get("limit") {
        None => 20,
        Some(value) => value
            .as_u64()
            .filter(|value| (1..=20).contains(value))
            .ok_or("invalid_request")? as usize,
    };
    let offset = match params.get("cursor") {
        None => 0,
        Some(value) => {
            let cursor = token(value).ok_or("source_changed")?;
            let (prefix, offset) = cursor.rsplit_once(':').ok_or("source_changed")?;
            if prefix != context["token"].as_str().unwrap_or_default() {
                return Err("source_changed");
            }
            if offset.is_empty() || !offset.bytes().all(|b| b.is_ascii_digit()) {
                return Err("source_changed");
            }
            offset.parse::<usize>().map_err(|_| "invalid_request")?
        }
    };
    if params.get("cursor").is_some() && offset >= Setting::ALL.len() {
        return Err("invalid_request");
    }
    Ok((offset, limit))
}

pub(super) fn command(runtime: &mut Runtime, app: &mut App, inv: &Invocation) -> Option<Value> {
    let params = &inv.params;
    let fail = |reason| Some(failure(params, reason));
    let Some(request_runtime) = token(&params["runtime"]) else {
        return fail("invalid_request");
    };
    let Some(op @ ("schema" | "get" | "set")) = params["op"].as_str() else {
        return fail("invalid_request");
    };
    if request_runtime != runtime.remote.runtime() {
        return fail("runtime_changed");
    }
    let now = runtime.now_ms();
    if inv.deadline_ms <= now {
        return fail("source_changed");
    }
    if !runtime.loaded || runtime.saving || runtime.entropy.is_none() {
        return fail("revision_conflict");
    }
    let mut context = setting_context(
        &runtime.saved,
        runtime.provider,
        request_runtime,
        runtime
            .entropy
            .as_ref()
            .expect("settings context requires private entropy"),
    );
    if params["id"] == "m3uMediaUrl" {
        if runtime.provider != ProviderKind::M3u || runtime.persistence_uncertain
            || runtime.m3u_attachments.context_revision() == 0
            || !runtime.active().is_some_and(|parent|parent.enabled && parent.kind==ProviderKind::M3u) {
            return fail("source_changed");
        }
        media_scope(&mut context,runtime.m3u_attachments.context_revision(),runtime.navigation_epoch);
    }
    let mut result = json!({"version":1,"op":op,"runtime":request_runtime,"context":context});
    if op == "schema" {
        let (offset, limit) = match schema_page(params, &context) {
            Ok(page) => page,
            Err(reason) => return fail(reason),
        };
        let end = (offset + limit).min(Setting::ALL.len());
        result["settings"] = json!(Setting::ALL[offset..end]
            .iter()
            .map(|setting| setting.descriptor())
            .collect::<Vec<_>>());
        result["next_cursor"] = if end < Setting::ALL.len() {
            json!(format!(
                "{}:{end}",
                context["token"].as_str().unwrap_or_default()
            ))
        } else {
            Value::Null
        };
        return Some(ok(result));
    }
    if !fields(
        params,
        if op == "get" {
            &["id", "op", "runtime"]
        } else {
            &["context", "id", "op", "revision", "runtime", "value"]
        },
    ) {
        return fail("invalid_request");
    }
    let Some(id) = params["id"].as_str().filter(|id| {
        !id.is_empty()
            && id.len() <= 64
            && id.as_bytes()[0].is_ascii_alphabetic()
            && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    }) else {
        return fail("invalid_request");
    };
    let Some(setting) = Setting::find(id) else {
        return fail("unknown_setting");
    };
    if op == "get" {
        result["descriptor"] = setting.descriptor();
        result["setting"] = with_revision(
            &mut runtime.command_receipts,
            &runtime.saved,
            setting,
            &context,
            inv,
            false,
            now,
        );
        if matches!(setting, Setting::NativeInputProbe) {
            result["setting"]["effective_value"] = json!(runtime.native_input_probe());
        } else if matches!(setting, Setting::HostedEpg) {
            result["setting"]["effective_value"] = stored(&runtime.saved, setting)["value"].clone();
        } else if matches!(setting, Setting::FontScale) {
            result["setting"]["effective_value"] = json!(app.bridge.ui_font_scale_percent());
        } else if matches!(setting, Setting::UiColors) {
            result["setting"]["effective_value"] = json!(plx_ui::palette::current().encode());
        } else if matches!(setting, Setting::VolumeStep) {
            result["setting"]["effective_value"] = json!(runtime.volume_step());
        } else if matches!(setting, Setting::InfoTimeout) {
            result["setting"]["effective_value"] = json!(app.bridge.info_timeout().seconds());
        } else if matches!(setting, Setting::Interface) {
            result["setting"]["effective_value"] = json!(app.bridge.ui_mode().index());
        } else if matches!(setting, Setting::Sleep) {
            result["setting"]["effective_value"] = json!(runtime.sleep_timer_choice().index());
        } else if let Setting::Menu(item) = setting {
            result["setting"]["effective_value"] = json!(u8::from(plx_appkit::menu_visibility::projected().visible(item)));
        } else if let Setting::Appearance(field) = setting {
            result["setting"]["effective_value"] = field.value(
                plx_media::route::subtitle_text_appearance(&app.player.session));
        } else if let Setting::Osd(field) = setting {
            result["setting"]["effective_value"] = field.value(runtime.osd_preferences());
        } else if let Setting::Classic(field) = setting {
            result["setting"]["effective_value"] = field.value(runtime.classic_preferences());
        } else if let Setting::SubtitleSelection(field) = setting {
            if plx_media::route::external_source(&app.player.session).is_some() {
                result["setting"]["effective_value"] = field.value(
                    &plx_media::route::hls_subtitle_policy(&app.player.session));
            }
        } else if let Setting::Geometry(field) = setting {
            // Plex geometry belongs to its profile; do not invent an effective
            // external-source value when that consumer is absent.
            if let Some(value) = plx_media::route::native_subtitle_preferences(&app.player.session) {
                result["setting"]["effective_value"] = field.value(value);
            }
        }
        return Some(ok(result));
    }
    if token(&params["context"]).is_none()
        || token(&params["revision"]).is_none()
        || !setting.valid(&params["value"])
    {
        return fail("invalid_value");
    }
    if params["context"] != context["token"] {
        return fail("source_changed");
    }
    if !setting.writable() {
        return fail("unsupported");
    }
    if !runtime.command_receipts.consume(
        params["revision"].as_str().unwrap_or_default(),
        &signature_for(&runtime.saved, setting, wire_provider(runtime.provider)),
        context["token"].as_str().unwrap_or_default(),
        0,
        now,
    ) {
        return fail("revision_conflict");
    }
    let before = stored_for(&runtime.saved, setting, wire_provider(runtime.provider));
    result["changed"] = json!(before["storage"] != "stored" || before["value"] != params["value"]);
    let mut candidate = if matches!(setting,Setting::M3uMedia) {
        let parent=runtime.active().cloned().expect("captured M3U parent checked above");
        let value=params["value"].as_str().expect("media link validated above");
        let new_id=if !value.trim().is_empty() && media_attachment(&runtime.saved).is_none() {
            match super::super::favorites::nonce(runtime) {Ok(id)=>Some(id),Err(_)=>return fail("storage_failed")}
        } else {None};
        match runtime.attachment_candidate(&parent,value,new_id.as_deref()) {
            Ok(candidate)=>candidate,Err(_)=>return fail("invalid_value"),
        }
    } else if let Setting::Menu(item) = setting {
        match menus::candidate(&runtime.saved, MenuChange::Set { item,
            visible: params["value"].as_u64().expect("validated menu integer") == 1 }) {
            Ok(candidate) => candidate, Err(_) => return fail("storage_failed"),
        }
    } else if let Setting::SubtitleSelection(field) = setting {
        match field.candidate(&runtime.saved, &params["value"]) {
            Ok(candidate) => candidate, Err(_) => return fail("storage_failed"),
        }
    } else if let Setting::Geometry(field) = setting {
        if super::super::subtitle_settings::Stored::read(&runtime.saved).is_err() {
            return fail("storage_failed");
        }
        let edit = match field.edit(&params["value"]) {
            Ok(edit) => edit, Err(_) => return fail("invalid_value"),
        };
        match edit.candidate(&runtime.saved) {
            Ok(candidate) => candidate, Err(_) => return fail("storage_failed"),
        }
    } else if let Setting::Appearance(field) = setting {
        let stored = match super::super::subtitle_settings::Stored::read(&runtime.saved) {
            Ok(stored) => stored, Err(_) => return fail("storage_failed"),
        };
        let appearance = match field.merge(stored.appearance, &params["value"]) {
            Ok(appearance) => appearance, Err(_) => return fail("invalid_value"),
        };
        match super::super::subtitle_settings::Edit::Appearance(appearance).candidate(&runtime.saved) {
            Ok(candidate) => candidate, Err(_) => return fail("storage_failed"),
        }
    } else {
        let mut candidate = runtime.saved.clone();
        candidate.settings.insert(storage_key_for(setting, wire_provider(runtime.provider)), params["value"].clone());
        candidate
    };
    if matches!(setting, Setting::M3uMedia) {
        result["changed"] = json!(media_attachment(&candidate) != media_attachment(&runtime.saved));
    }
    // The shared subtitle revision is prepared by every local and remote Edit.
    if !matches!(setting, Setting::Appearance(_) | Setting::Geometry(_) | Setting::SubtitleSelection(_) | Setting::Menu(_)) {
        candidate.settings.insert(revision_key_for(setting, wire_provider(runtime.provider)), json!(hash(&json!(inv.id))));
    }
    if matches!(setting,Setting::M3uMedia) {
        let mut projected=runtime.m3u_attachments.clone();
        let registry=match crate::app::ott_m3u_attachment::Registry::read(&candidate.settings,&candidate.profiles) {
            Ok(registry)=>registry,Err(_)=>return fail("invalid_value"),
        };
        if projected.observe(&registry,&candidate.profiles).is_err() {return fail("storage_failed");}
        context=setting_context(&candidate,runtime.provider,request_runtime,
            runtime.entropy.as_ref().expect("settings entropy checked above"));
        media_scope(&mut context,projected.context_revision(),runtime.navigation_epoch);
        // The operation receipt keeps the captured request context. Only the
        // newly issued CAS revision belongs to the post-commit authority; a
        // subsequent get reveals that authority to the caller.
    }
    result["persisted"] = json!(true);
    result["application"] = json!({"state":"completed"});
    result["setting"] = with_revision(
        &mut runtime.command_receipts,
        &candidate,
        setting,
        &context,
        inv,
        true,
        now,
    );
    if matches!(setting, Setting::HostedEpg | Setting::FontScale | Setting::UiColors | Setting::InfoTimeout | Setting::VolumeStep | Setting::Interface | Setting::Sleep | Setting::Osd(_) | Setting::Classic(_) | Setting::Menu(_)) {
        result["setting"]["effective_value"] = params["value"].clone();
    }
    let saving = if matches!(setting, Setting::Interface) {
        runtime.save_public_preference(candidate,
            super::super::public_preferences::Patch::InterfaceMode(params["value"].as_u64().unwrap_or(1) as u8),
            Some(inv.clone()), ok(result))
    } else { runtime.save_reply(candidate, inv.clone(), false, ok(result)) };
    match saving {
        Ok(()) => None,
        Err(_) => fail("storage_failed"),
    }
}

// The local editor shares the remote setting key, validator and revision input.
// It prepares one ordinary Saved transaction; no draft value reaches the painter.
impl Runtime {
    pub(in crate::app::ott_runtime) fn ui_color_config(&self) -> String {
        stored(&self.saved, Setting::UiColors)["value"].as_str().unwrap_or("").to_owned()
    }
    pub(in crate::app::ott_runtime) fn ui_color_candidate(&self, text: &str, session: u64)
        -> Result<Saved, &'static str> {
        plx_ui::palette::Config::parse(text)?;
        let mut saved = self.saved.clone();
        saved.settings.insert(Setting::UiColors.id().into(), json!(text));
        saved.settings.insert(revision_key(Setting::UiColors), json!(hash(&json!([
            "local-setting", self.remote.runtime(), session, signature(&self.saved, Setting::UiColors)
        ]))));
        Ok(saved)
    }
    pub(in crate::app::ott_runtime) fn sleep_timer_choice(&self) -> super::super::sleep_timer::Choice {
        self.saved.settings.get(Setting::Sleep.id()).and_then(Value::as_u64)
            .and_then(super::super::sleep_timer::Choice::from_index).unwrap_or_default()
    }

    pub(in crate::app::ott_runtime) fn sleep_timer_token(&self) -> String {
        let context = setting_context(&self.saved, self.provider, self.remote.runtime(),
            self.entropy.as_ref().map(|nonce| nonce.as_slice()).unwrap_or(&[]));
        hash(&json!([signature(&self.saved, Setting::Sleep), self.navigation_epoch,
            self.edit_serial, context["token"]]))
    }

    pub(in crate::app::ott_runtime) fn sleep_timer_candidate(
        &self, choice: super::super::sleep_timer::Choice,
    ) -> Saved {
        let mut saved = self.saved.clone();
        saved.settings.insert(Setting::Sleep.id().into(), json!(choice.index()));
        saved.settings.insert(revision_key(Setting::Sleep), json!(hash(&json!([
            "local-sleep-timer", self.remote.runtime(), choice.index(),
            signature(&self.saved, Setting::Sleep)
        ]))));
        saved
    }

    pub(in crate::app::ott_runtime) fn info_timeout(&self) -> plx_screens::player::input::InfoTimeout {
        self.saved.settings.get("infoTimeout").and_then(Value::as_u64)
            .and_then(plx_screens::player::input::InfoTimeout::from_seconds)
            .unwrap_or(plx_screens::player::input::InfoTimeout::DEFAULT)
    }
    pub(in crate::app::ott_runtime) fn info_timeout_candidate(&self, text: &str, session: u64) -> Result<Saved, &'static str> {
        let text = text.trim();
        if text.is_empty() || text.len() > 2 || !text.bytes().all(|b| b.is_ascii_digit()) {
            return Err("Enter an integer from 3 to 20");
        }
        let value = text.parse::<u64>().ok()
            .and_then(plx_screens::player::input::InfoTimeout::from_seconds)
            .ok_or("Enter an integer from 3 to 20")?;
        let mut saved = self.saved.clone();
        saved.settings.insert("infoTimeout".into(), json!(value.seconds()));
        saved.settings.insert(revision_key(Setting::InfoTimeout), json!(hash(&json!([
            "local-setting", self.remote.runtime(), session, signature(&self.saved, Setting::InfoTimeout)
        ]))));
        Ok(saved)
    }

    /// Only accepted Loaded/Saved publication updates this display snapshot.
    pub(in crate::app) fn osd_preferences(&self)
        -> (plx_appkit::player_hud::model::OsdOpacity, plx_appkit::player_hud::model::PermanentClock)
    { self.osd_display }

    /// Only accepted Loaded/Saved publication updates this consumer snapshot.
    pub(in crate::app) fn classic_preferences(&self) -> plx_appkit::classic_settings::Preferences {
        let [number, name, program, description, progress] = self.classic_display
            .get(wire_provider(self.provider)).copied().unwrap_or([1; 5]);
        self.classic_preferences.with_channel_display(number, name, program, description)
            .and_then(|preferences| preferences.with_show_progress(progress))
            .expect("accepted provider display flags are boolean")
    }

    pub(in crate::app::ott_runtime) fn osd_setting_value(&self, id: &str) -> Option<String> {
        let value = match Setting::local_choice(id)? {
            Setting::Osd(field) => field.value(osd::preferences(&self.saved)),
            Setting::Classic(field) => field.value(self.classic_preferences()),
            _ => return None,
        };
        Some(value.as_u64()?.to_string())
    }

    pub(in crate::app::ott_runtime) fn osd_setting_token(&self, id: &str) -> Option<String> {
        let setting = Setting::local_choice(id)?;
        if !self.loaded || self.saving || self.persistence_uncertain { return None; }
        let context = setting_context(&self.saved, self.provider, self.remote.runtime(), self.entropy.as_ref()?);
        Some(hash(&json!([signature_for(&self.saved, setting, wire_provider(self.provider)), self.page, self.navigation_epoch,
            self.edit_serial, context["token"]])))
    }

    /// The only two setting entries a grouped draft may merge from its candidate.
    /// Provider/source fences are still checked by that editor before publication.
    pub(in crate::app::ott_runtime) fn osd_setting_storage_keys(&self, id: &str) -> Option<[String; 2]> {
        let setting = Setting::local_choice(id)?;
        let provider = wire_provider(self.provider);
        Some([storage_key_for(setting, provider), revision_key_for(setting, provider)])
    }

    pub(in crate::app::ott_runtime) fn osd_setting_candidate(&self, id: &str, text: &str, session: u64)
        -> Result<Saved, &'static str>
    {
        let setting = Setting::local_choice(id).ok_or("Unknown on-screen display setting")?;
        let value = match setting {
            Setting::Osd(field) => field.parse_text(text)?,
            Setting::Classic(field) => field.parse_text(text)?,
            _ => return Err("Unknown on-screen display setting"),
        };
        let mut saved = self.saved.clone();
        saved.settings.insert(storage_key_for(setting, wire_provider(self.provider)), value);
        saved.settings.insert(revision_key_for(setting, wire_provider(self.provider)), json!(hash(&json!([
            "local-setting", self.remote.runtime(), session, signature_for(&self.saved, setting, wire_provider(self.provider))
        ]))));
        Ok(saved)
    }

    pub(in crate::app::ott_runtime) fn interface_mode(&self) -> plx_machine::machine::UiMode {
        let index = stored(&self.saved, Setting::Interface)["value"].as_u64().unwrap_or(0);
        plx_machine::machine::UiMode::from_index(index).unwrap_or_default()
    }

    pub(in crate::app::ott_runtime) fn interface_mode_token(&self) -> String {
        let context = setting_context(&self.saved, self.provider, self.remote.runtime(),
            self.entropy.as_ref().map(|nonce| nonce.as_slice()).unwrap_or(&[]));
        hash(&json!([signature(&self.saved, Setting::Interface), self.navigation_epoch,
            self.edit_serial, context["token"]]))
    }

    pub(in crate::app::ott_runtime) fn interface_mode_candidate(
        &self, mode: plx_machine::machine::UiMode,
    ) -> Saved {
        let mut saved = self.saved.clone();
        saved.settings.insert(Setting::Interface.id().into(), json!(mode.index()));
        saved.settings.insert(revision_key(Setting::Interface), json!(hash(&json!([
            "local-interface-mode", self.remote.runtime(), mode.index(),
            signature(&self.saved, Setting::Interface)
        ]))));
        saved
    }

    pub(in crate::app::ott_runtime) fn font_scale_percent(&self) -> u16 {
        stored(&self.saved, Setting::FontScale)["value"].as_u64().unwrap_or(100) as u16
    }

    pub(in crate::app::ott_runtime) fn volume_step(&self) -> i32 {
        stored(&self.saved, Setting::VolumeStep)["value"].as_u64().unwrap_or(5) as i32
    }

    pub(in crate::app::ott_runtime) fn volume_step_candidate(&self, text: &str, session: u64)
        -> Result<Saved, &'static str>
    {
        let text = text.trim();
        if text.is_empty() || text.len() > 2 || !text.bytes().all(|b| b.is_ascii_digit()) {
            return Err("Enter an integer from 3 to 10");
        }
        let step = text.parse::<u8>().map_err(|_| "Enter an integer from 3 to 10")?;
        let value = json!(step);
        if !Setting::VolumeStep.valid(&value) { return Err("Enter an integer from 3 to 10"); }
        let mut saved = self.saved.clone();
        saved.settings.insert(Setting::VolumeStep.id().into(), value);
        saved.settings.insert(revision_key(Setting::VolumeStep), json!(hash(&json!([
            "local-setting", self.remote.runtime(), session, signature(&self.saved, Setting::VolumeStep)
        ]))));
        Ok(saved)
    }

    pub(in crate::app::ott_runtime) fn font_scale_candidate(&self, text: &str, session: u64)
        -> Result<Saved, &'static str>
    {
        let text = text.trim();
        if text.is_empty() || text.len() > 3 || !text.bytes().all(|b| b.is_ascii_digit()) {
            return Err("Enter an integer from 20 to 200");
        }
        let percent = text.parse::<u16>().map_err(|_| "Enter an integer from 20 to 200")?;
        let value = json!(percent);
        if !Setting::FontScale.valid(&value) {
            return Err("Enter an integer from 20 to 200");
        }
        let mut saved = self.saved.clone();
        saved.settings.insert(Setting::FontScale.id().into(), value);
        // Include the previous signature: A -> B -> A through local editors
        // cannot revive a remote CAS receipt issued before either local write.
        saved.settings.insert(revision_key(Setting::FontScale), json!(hash(&json!([
            "local-setting", self.remote.runtime(), session, signature(&self.saved, Setting::FontScale)
        ]))));
        Ok(saved)
    }
}

/// Restore only after the encrypted settings snapshot was loaded or durably
/// committed. Typography changes invalidate shared measurement and paint caches
/// without touching the foreground player or its standby state.
pub(super) fn after_saved(runtime: &mut Runtime, app: &mut App) -> Result<(), &'static str> {
    runtime.sync_hosted_epg_config()?;
    let subtitles = super::super::subtitle_settings::Stored::read(&runtime.saved)?;
    let menu = menus::Stored::read(&runtime.saved)?;
    let percent = stored(&runtime.saved, Setting::FontScale)["value"]
        .as_u64()
        .unwrap_or(100) as u16;
    app.bridge
        .apply_ui_font_scale(percent)
        .map(|_| ())
        .map_err(|_| "Interface text scale could not be applied")?;
    subtitles.apply(app, false);
    plx_appkit::menu_visibility::project(menu.visibility());
    let palette = stored(&runtime.saved, Setting::UiColors)["value"].as_str()
        .and_then(|text| plx_ui::palette::Config::parse(text).ok()).unwrap_or_default();
    plx_ui::palette::project(palette);
    app.bridge.apply_ui_mode(runtime.interface_mode());
    let timeout = runtime.info_timeout();
    app.bridge.apply_info_timeout(timeout);
    if let Some(player) = crate::app::bridge::player_mut(&mut app.pages) {
        player.hud.set_info_timeout(timeout);
    }
    runtime.osd_display = osd::preferences(&runtime.saved);
    runtime.classic_preferences = classic::preferences(&runtime.saved);
    // Capture every provider before an uncertain visible Saved image can replace
    // the raw document. Browsing another provider or slot still uses its accepted flags.
    runtime.classic_display = ProviderKind::all().into_iter().map(|provider| {
            let key = wire_provider(provider);
            let display = classic::with_provider_display(runtime.classic_preferences, &runtime.saved, key);
            (key, [u8::from(display.show_number()), u8::from(display.show_name()),
                u8::from(display.show_program()), u8::from(display.show_description()),
                u8::from(display.show_progress())])
        }).collect();
    Ok(())
}

/// Connection migration needs a durable, acknowledged transition with rollback.
/// Refuse it explicitly until that owner exists; writing inert keys is not apply.
pub(super) fn patch(runtime: &Runtime, inv: &Invocation) -> Value {
    let params = &inv.params;
    let mut response = failure(params, "unsupported");
    response["data"]["group"] = params["group"].clone();
    response["data"]["capability_reason"] = json!("lifecycle_transition_unavailable");
    if token(&params["runtime"]).is_none()
        || !matches!(
            params["group"].as_str(),
            Some("command_server" | "local_http")
        )
        || !match params["op"].as_str() {
            Some("get") => fields(params, &["group", "op", "runtime"]),
            Some("set") => fields(
                params,
                &["context", "group", "op", "revision", "runtime", "values"],
            ),
            _ => false,
        }
    {
        return failure(params, "invalid_request");
    }
    if params["runtime"] != runtime.remote.runtime() {
        return failure(params, "runtime_changed");
    }
    if inv.deadline_ms <= runtime.now_ms() {
        return failure(params, "source_changed");
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved() -> Saved {
        let mut saved = Saved::default();
        saved.profiles.profiles.push(ProviderProfile {
            id: "m3u-profile".into(),
            name: "Home".into(),
            kind: ProviderKind::M3u,
            source: "https://example.test/channels.m3u".into(),
            source_alias: String::new(),
            enabled: true,
        });
        saved.profiles.active_profile = Some("m3u-profile".into());
        saved.settings.insert("slot:m3u-profile".into(), json!(7));
        saved
    }

    #[test]
    fn context_uses_sparse_slot_and_changes_with_source() {
        let mut saved = saved();
        let before = setting_context(&saved, ProviderKind::M3u, "runtime:1", &[1; 16]);
        assert_eq!(before["profile"], 7);
        assert_eq!(before["provider"], "m3u");
        saved.profiles.profiles[0].source = "https://example.test/other.m3u".into();
        let after = setting_context(&saved, ProviderKind::M3u, "runtime:1", &[1; 16]);
        assert_ne!(before["token"], after["token"]);
        assert!(!after.to_string().contains("example.test"));
        assert_ne!(
            after["token"],
            setting_context(&saved, ProviderKind::M3u, "runtime:2", &[1; 16])["token"]
        );
        assert_ne!(
            after["token"],
            setting_context(&saved, ProviderKind::M3u, "runtime:1", &[2; 16])["token"]
        );
        assert_eq!(
            setting_context(&saved, ProviderKind::Local, "runtime:1", &[1; 16])["profile"],
            Value::Null
        );
    }

    #[test]
    fn font_scale_rejects_noninteger_and_out_of_range_values() {
        for value in [
            json!(-1),
            json!(0),
            json!(19),
            json!(201),
            json!(100.5),
            json!("100"),
            Value::Null,
            json!(true),
        ] {
            assert!(!Setting::FontScale.valid(&value), "{value}");
        }
        for value in [json!(20), json!(50), json!(100), json!(150), json!(200)] {
            assert!(Setting::FontScale.valid(&value), "{value}");
        }
        let descriptor = Setting::FontScale.descriptor();
        assert_eq!(descriptor["constraints"]["minimum"], 20);
        assert_eq!(descriptor["constraints"]["maximum"], 200);
        assert_eq!(descriptor["access"]["write"], true);
        assert_eq!(descriptor["default_value"], 100);
    }

    #[test]
    fn choice_descriptors_include_cli_required_aliases() {
        for (setting, label, alias) in [
            (Setting::Player, "Native", "native"),
        ] {
            let descriptor = setting.descriptor();
            let choices = descriptor["choices"].as_array().unwrap();
            assert_eq!(choices.len(), 1);
            assert!(fields(&choices[0], &["value", "label", "aliases"]));
            assert_eq!(choices[0]["value"], setting.default());
            assert_eq!(choices[0]["label"], label);
            assert_eq!(choices[0]["aliases"], json!([alias]));
        }
    }

    #[test]
    fn stored_value_reports_invalid_without_echoing_it() {
        let mut saved = saved();
        assert_eq!(stored(&saved, Setting::FontScale)["storage"], "default");
        saved
            .settings
            .insert("uiFontScale".into(), json!("invalid"));
        let invalid = stored(&saved, Setting::FontScale);
        assert_eq!(invalid["storage"], "invalid");
        assert_eq!(invalid["value"], 100);
        assert!(invalid.get("persisted_value").is_none());
        saved.settings.insert("uiFontScale".into(), json!(120));
        let valid = stored(&saved, Setting::FontScale);
        assert_eq!(valid["storage"], "stored");
        assert_eq!(valid["persisted_value"], 120);
    }

    #[test]
    fn revisions_are_single_use_bound_to_storage_source_and_expiration() {
        let mut saved = saved();
        let context = setting_context(&saved, ProviderKind::M3u, "runtime:1", &[1; 16])["token"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut receipts = Receipts::default();
        let sig = signature(&saved, Setting::FontScale);
        let token = receipts.record("setting:read", sig.clone(), context.clone(), 0, 10);
        assert!(receipts.consume(&token, &sig, &context, 0, 11));
        assert!(!receipts.consume(&token, &sig, &context, 0, 12));
        let token = receipts.record("setting:changed", sig.clone(), context.clone(), 0, 10);
        saved.settings.insert("uiFontScale".into(), json!(120));
        assert!(!receipts.consume(
            &token,
            &signature(&saved, Setting::FontScale),
            &context,
            0,
            11
        ));
        let token = receipts.record("setting:source", sig.clone(), context.clone(), 0, 10);
        assert!(!receipts.consume(&token, &sig, "another-source", 0, 11));
        let token = receipts.record("setting:expired", sig.clone(), context.clone(), 0, 10);
        assert!(!receipts.consume(&token, &sig, &context, 0, 120010));
    }

    #[test]
    fn restoring_the_same_value_does_not_restore_an_old_revision() {
        let mut saved = saved();
        saved.settings.insert("uiFontScale".into(), json!(100));
        let before = signature(&saved, Setting::FontScale);
        saved.settings.insert("uiFontScale".into(), json!(120));
        saved
            .settings
            .insert(revision_key(Setting::FontScale), json!("write:1"));
        saved.settings.insert("uiFontScale".into(), json!(100));
        saved
            .settings
            .insert(revision_key(Setting::FontScale), json!("write:2"));
        assert_ne!(signature(&saved, Setting::FontScale), before);
    }

    #[test]
    fn schema_pages_validate_limits_fields_and_source_cursor() {
        let context = json!({"token":"context:1"});
        assert_eq!(
            schema_page(&json!({"op":"schema","runtime":"runtime:1"}), &context),
            Ok((0, 20))
        );
        assert_eq!(
            schema_page(
                &json!({"op":"schema","runtime":"runtime:1","limit":1,"cursor":"context:1:2"}),
                &context
            ),
            Ok((2, 1))
        );
        for limit in [json!(0), json!(21), json!("1"), json!(1.5)] {
            assert_eq!(
                schema_page(
                    &json!({"op":"schema","runtime":"runtime:1","limit":limit}),
                    &context
                ),
                Err("invalid_request")
            );
        }
        assert_eq!(
            schema_page(
                &json!({"op":"schema","runtime":"runtime:1","unexpected":true}),
                &context
            ),
            Err("invalid_request")
        );
        assert_eq!(
            schema_page(
                &json!({"op":"schema","runtime":"runtime:1","cursor":"context:2:1"}),
                &context
            ),
            Err("source_changed")
        );
        assert_eq!(
            schema_page(
                &json!({"op":"schema","runtime":"runtime:1","cursor":format!("context:1:{}", Setting::ALL.len())}),
                &context
            ),
            Err("invalid_request")
        );
        assert_eq!(
            schema_page(
                &json!({"op":"schema","runtime":"runtime:1","cursor":"context:1:-1"}),
                &context
            ),
            Err("source_changed")
        );
    }
}

#[cfg(test)]
#[path = "ott_settings_appearance_receipts_tests.rs"]
mod appearance_receipts_tests;

#[cfg(test)]
mod menu_tests {
    use super::*;
    #[test]
    fn each_remote_menu_field_is_v1_integer_and_shares_compound_storage() {
        for item in MenuItem::ALL {
            let field = Setting::find(item.setting()).unwrap();
            assert_eq!(field.storage_key(), menus::KEY);
            assert_eq!(revision_key(field), menus::KEY);
            assert!(field.valid(&json!(0))); assert!(field.valid(&json!(1)));
            for value in [json!(true), json!(false), json!(-1), json!(2), json!(1.0), json!("1"), Value::Null] { assert!(!field.valid(&value)); }
            assert_eq!(field.default(), 1);
            assert_eq!(field.descriptor()["id"], item.setting());
            assert_eq!(stored(&Saved::default(), field)["storage"], "default");
        }
    }
    #[test]
    fn local_set_reset_invalidates_every_previous_remote_menu_signature() {
        let original = Saved::default();
        let a = menus::candidate(&original, MenuChange::Set { item: MenuItem::Audio, visible: false }).unwrap();
        let b = menus::candidate(&a, MenuChange::Reset).unwrap();
        for item in MenuItem::ALL {
            let field = Setting::Menu(item);
            assert_ne!(signature(&original, field), signature(&a, field));
            assert_ne!(signature(&original, field), signature(&b, field));
            assert_ne!(signature(&a, field), signature(&b, field));
            assert_eq!(stored(&b, field)["value"], 1);
        }
    }
    #[test]
    fn menu_fields_extend_existing_paginated_schema_without_reordering_it() {
        assert_eq!(Setting::ALL.len(), 30 + MenuItem::ALL.len());
        assert_eq!(Setting::ALL[43].id(), "nativeEpgApiV1");
        assert_eq!(Setting::ALL[44].id(), "nativeInputProbeV1");
        assert_eq!(Setting::ALL[41..43].iter().map(|setting| setting.id()).collect::<Vec<_>>(),
            ["nativeSubtitleMode", "nativeSubtitleLanguage"]);
        assert_eq!(Setting::ALL[35..40].iter().map(|setting| setting.id()).collect::<Vec<_>>(),
            ["showNumber", "showName", "showProgram", "showDescription", "showProgress"]);
        assert_eq!(Setting::ALL[40].id(), "uiColorConfig");
        assert_eq!(Setting::ALL[0].id(), "uiFontScale");
        assert_eq!(Setting::ALL[7].id(), MenuItem::Providers.setting());
        let context = json!({"token":"context:1"});
        assert_eq!(schema_page(&json!({"op":"schema","runtime":"runtime:1","cursor":"context:1:20"}), &context), Ok((20,20)));
        assert_eq!(schema_page(&json!({"op":"schema","runtime":"runtime:1","cursor":"context:1:40"}), &context), Ok((40,20)));
    }
}

#[cfg(test)]
#[path = "ott_m3u_attachment_settings_tests.rs"]
mod m3u_attachment_tests;

#[cfg(test)]
#[path = "ott_interface_settings_tests.rs"]
mod interface_settings_tests;

#[cfg(test)]
#[path = "ott_settings_geometry_tests.rs"]
mod geometry_tests;

#[cfg(test)]
mod volume_step_tests {
    use super::*;

    #[test]
    fn volume_step_schema_is_a_bounded_integer_and_appends_without_reordering() {
        let setting = Setting::find("volumeStep").unwrap();
        let descriptor = setting.descriptor();
        assert_eq!(descriptor["type"], "integer");
        assert_eq!(descriptor["default_value"], 5);
        assert_eq!(descriptor["constraints"], json!({"minimum":3,"maximum":10}));
        assert_eq!(descriptor["access"], json!({"read":true,"write":true}));
        assert_eq!(Setting::ALL[26..29].iter().map(|setting| setting.id()).collect::<Vec<_>>(),
            ["volumeStep", "sleepTimeout", "infoTimeout"]);
        for value in 3..=10 { assert!(setting.valid(&json!(value))); }
        for value in [json!(-1), json!(0), json!(2), json!(11), json!(u64::MAX),
            json!(5.0), json!("5"), json!(true), json!(false), Value::Null, json!({})] {
            assert!(!setting.valid(&value), "{value}");
        }
    }

    #[test]
    fn volume_step_absence_defaults_to_five_and_invalid_storage_is_not_echoed() {
        let mut saved = Saved::default();
        assert_eq!(stored(&saved, Setting::VolumeStep)["storage"], "default");
        assert_eq!(stored(&saved, Setting::VolumeStep)["value"], 5);
        for value in [json!(2), json!(11), json!("private-invalid-fixture"), json!(5.0), json!(true)] {
            saved.settings.insert("volumeStep".into(), value);
            let report = stored(&saved, Setting::VolumeStep);
            assert_eq!(report["storage"], "invalid");
            assert_eq!(report["value"], 5);
            assert!(report.get("persisted_value").is_none());
        }
        for step in [3, 5, 10] {
            saved.settings.insert("volumeStep".into(), json!(step));
            let report = stored(&saved, Setting::VolumeStep);
            assert_eq!(report["storage"], "stored");
            assert_eq!(report["value"], step);
            assert_eq!(report["persisted_value"], step);
        }
    }

    #[test]
    fn volume_step_local_candidates_validate_without_publishing_and_retire_aba() {
        let _serial = plx_base::testlock::serial();
        let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
        runtime.remote = Remote::disabled_for_test("dev_fixture", "fixture");
        runtime.saved.settings.insert("unrelated".into(), json!({"keep":true}));
        let original = serde_json::to_value(&runtime.saved).unwrap();
        let original_signature = signature(&runtime.saved, Setting::VolumeStep);
        for text in ["", "2", "11", "-3", "+5", "5.0", "5e0", "true", "100", "５"] {
            assert_eq!(runtime.volume_step_candidate(text, 1).err(), Some("Enter an integer from 3 to 10"));
            assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), original);
        }
        let ten = runtime.volume_step_candidate("10", 1).unwrap();
        assert_eq!(runtime.volume_step(), 5);
        runtime.saved = ten;
        assert_eq!(runtime.volume_step(), 10);
        let five = runtime.volume_step_candidate("5", 2).unwrap();
        assert_ne!(signature(&five, Setting::VolumeStep), original_signature);
        assert_ne!(signature(&five, Setting::VolumeStep), signature(&runtime.saved, Setting::VolumeStep));
        let mut unrelated = five;
        unrelated.settings.remove("volumeStep");
        unrelated.settings.remove(&revision_key(Setting::VolumeStep));
        assert_eq!(serde_json::to_value(&unrelated).unwrap(), original);
    }
}

#[cfg(test)]
#[path = "ott_sleep_timer_settings_tests.rs"]
mod sleep_timer_tests;

#[cfg(test)]
mod color_tests {
    #[test]
    fn normalized_palette_read_preserves_raw_saved_bytes() {
        let mut saved = super::Saved::default();
        let raw = r##"{ "colors": {"text":"#aabbcc"}, "version":1 }"##;
        saved.settings.insert("uiColorConfig".into(), serde_json::json!(raw));
        let value = super::stored(&saved, super::Setting::UiColors);
        assert_eq!(value["storage"], "stored");
        assert_eq!(value["value"], r##"{"version":1,"colors":{"text":"#AABBCC"}}"##);
        assert_eq!(value["persisted_value"], raw);
        assert_eq!(saved.settings["uiColorConfig"], raw);
    }

    use super::*;
    #[test]
    fn color_wire_contract_is_app_scoped_canonical_and_aliases_one_record() {
        let canonical = Setting::find("uiColorConfig").unwrap();
        let alias = Setting::find("sUiColorConfig").unwrap();
        assert_eq!(alias.id(),canonical.id());
        assert_eq!(alias.storage_key(),canonical.storage_key());
        assert_eq!(revision_key(alias),revision_key(canonical));
        let schema = canonical.descriptor();
        assert_eq!(schema["scope"],"application");
        assert_eq!(schema["type"],"string");
        assert_eq!(schema["constraints"]["max_utf8_bytes"],2048);
        assert_eq!(schema["default_value"],"");
        assert_eq!(canonical.default(),json!(""));
        assert!(canonical.valid(&json!("")));
        let text = plx_ui::palette::Config::preset("midnight").unwrap().encode();
        assert!(canonical.valid(&json!(text)));
        for value in [json!(0),json!({}),json!("null"),json!(" "),json!("{\"version\":1,\"colors\":{}}"),json!(text.to_lowercase())] {
            assert!(!canonical.valid(&value));
        }
    }
}

#[cfg(test)]
mod subtitle_selection_tests {
    use super::*;
    #[test]
    fn native_selection_descriptors_and_cas_share_the_native_subtitle_revision() {
        let before = Saved::default();
        let mode = Setting::find("nativeSubtitleMode").unwrap();
        let language = Setting::find("nativeSubtitleLanguage").unwrap();
        for setting in [mode, language] {
            assert!(setting.writable());
            assert_eq!(setting.descriptor()["scope"], "application");
            assert_eq!(setting.descriptor()["apply"], "immediate");
            assert_eq!(revision_key(setting), super::super::super::subtitle_settings::REVISION);
        }
        assert_eq!(stored(&before, mode)["value"], "off");
        assert_eq!(stored(&before, language)["value"], "");
        let candidate = selection::Field::Mode.candidate(&before, &json!("auto")).unwrap();
        assert_ne!(signature(&before, language), signature(&candidate, language),
            "a mode update also retires a previously issued language CAS receipt");
        assert_eq!(stored(&candidate, mode)["persisted_value"], "auto");
        assert!(!mode.valid(&json!("always")));
        assert!(!language.valid(&json!("en/US")));
    }
}

#[cfg(test)]
mod hosted_epg_setting_tests {
    use super::*;
    #[test]
    fn native_epg_setting_uses_legacy_string_schema_and_preserves_exact_json_text() {
        let setting = Setting::find("nativeEpgApiV1").unwrap();
        let descriptor = setting.descriptor();
        let fixture: Value = serde_json::from_str(include_str!("fixtures/native-epg-setting-v1.json")).unwrap();
        assert_eq!(descriptor, fixture);
        assert_eq!(descriptor["type"], "string");
        assert_eq!(descriptor["default_value"], "");
        assert_eq!(descriptor["constraints"], json!({"max_utf8_bytes":4096}));
        assert!(descriptor.get("nullable").is_none());
        let text = r#"{ "source": "epg-one", "url": "https://guide.invalid/epg/v1", "version": 1 }"#;
        let value = json!(text);
        assert!(setting.valid(&value));
        let mut saved = Saved::default();
        saved.settings.insert(setting.id().into(), value.clone());
        let record = stored(&saved, setting);
        assert_eq!(record["value"], value);
        assert_eq!(record["persisted_value"], value);
        assert!(setting.valid(&json!("")));
        assert!(!setting.valid(&Value::Null));
        assert!(!setting.valid(&json!({"version":1,"url":"https://guide.invalid/epg/v1","source":"epg-one"})));
    }
    #[test]
    fn native_input_probe_setting_is_readonly_string_and_never_persisted_input() {
        let setting = Setting::find("nativeInputProbeV1").unwrap();
        let descriptor = setting.descriptor();
        assert_eq!(descriptor["type"], "string");
        assert_eq!(descriptor["access"], json!({"read":true,"write":false,"reason":"unsupported"}));
        assert_eq!(descriptor["constraints"]["max_utf8_bytes"], 4096);
        assert!(!setting.writable());
        let mut saved = Saved::default();
        saved.settings.insert(setting.id().into(), json!("untrusted-persisted-input"));
        assert_eq!(stored(&saved, setting), json!({"id":"nativeInputProbeV1","storage":"default","value":""}));
    }

}
