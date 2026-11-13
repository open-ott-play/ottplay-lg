//! Native adapters for the existing authenticated OTT request/result contract.
//! Source credentials remain inside Saved and never enter metadata receipts.
use super::*;
#[path = "ott_catalog_remote.rs"]
mod catalog_remote;
#[path = "ott_controls.rs"]
mod controls;
#[path = "ott_media_tracks.rs"]
pub(super) mod media_tracks;
#[path = "ott_video_evidence.rs"]
mod video_evidence;

pub(super) fn restartable(runtime: &Runtime, app: &App) -> bool {
    controls::restartable(runtime, app)
}

pub(super) fn restart_stream(runtime: &mut Runtime, app: &mut App) -> Result<(), &'static str> {
    controls::restart_stream(runtime, app).map(|_| ())
}

/// Opaque canonical projection shared with physical number entry.
pub(super) struct NumberedChannels(Vec<controls::Channel>);
impl NumberedChannels {
    pub(super) fn from_numbered(rows: impl IntoIterator<Item = (usize, String, String)>) -> Self {
        Self(
            rows.into_iter()
                .map(|(number, id, name)| controls::Channel { id, name, number })
                .collect(),
        )
    }
    /// The Classic list renders filtered positions, not remote API channel numbers.
    /// Keep this conversion explicit so command/Player callers stay canonical.
    pub(super) fn with_list_ordinals(mut self) -> Self {
        for (index, row) in self.0.iter_mut().enumerate() {
            row.number = index + 1;
        }
        self
    }
    pub(super) fn resolve(&self, number: &str) -> Option<(String, String)> {
        let id = controls::resolve(&self.0, number)?;
        let row = self.0.iter().find(|row| row.id == id)?;
        Some((id, row.name.clone()))
    }
}
pub(super) fn numbered_channels(runtime: &Runtime) -> NumberedChannels {
    let visible = runtime.search_filter();
    NumberedChannels(
        controls::channels(runtime)
            .into_iter()
            .filter(|row| visible(&row.name))
            .collect(),
    )
}
#[path = "ott_kiosk_commands.rs"]
mod kiosk;
#[path = "ott_commands_offline.rs"]
mod offline;
#[path = "ott_provider_settings.rs"]
mod provider_adapters;
#[path = "ott_provider_config.rs"]
mod provider_config;

pub(super) fn provider_settings_before_saved(runtime: &mut Runtime, inv: Option<&Invocation>) -> bool {
    provider_config::before_saved(runtime, inv)
}
pub(super) fn provider_settings_settled(runtime: &mut Runtime, inv: &Invocation, durable: bool, applied: bool) -> Option<Value> {
    provider_config::settled(runtime, inv, durable, applied)
}
pub(super) fn provider_settings_uncertain(runtime: &mut Runtime, inv: &Invocation) -> Option<Value> {
    provider_config::uncertain(runtime, inv)
}

#[path = "ott_settings.rs"]
mod settings;
use crate::app::ott_remote::AfterReply;
pub(super) use kiosk::Settlement as KioskSettlement;
use plx_machine::machine::{Edge, InputEvent, InputKind, Key, Source, Tick};

#[derive(Default)]
pub(super) struct Receipts {
    native_provider: provider_config::State,
    kiosk: Option<kiosk::Pending>,
    media_tracks: media_tracks::State,
    rows: Vec<Receipt>,
    catalog: catalog_remote::CatalogReceipts,
    catalog_play: Option<catalog_remote::PendingPlay>,
    play_response: Option<(String, Value)>,
    deferred_channel: Option<DeferredChannel>,
}
struct DeferredChannel {
    invocation: Invocation,
    catalog_generation: u64,
    last_input: u32,
    context: String,
}

fn selection_context(runtime: &Runtime) -> String {
    hash(&json!([
        context(runtime),
        runtime.view_selection().0,
        runtime.view_selection().1
    ]))
}

fn supersede_deferred(runtime: &mut Runtime) {
    if let Some(pending) = runtime.command_receipts.deferred_channel.take() {
        runtime
            .remote
            .complete(pending.invocation, rejected("Channel request superseded"));
    }
}

/// Retire only read snapshots after an explicit temporary list-order change.
/// The existing admitted playback receipt and deferred playback intent stay owned.
pub(super) fn retire_catalog_reads(runtime: &mut Runtime) {
    runtime.command_receipts.catalog.retire_reads();
}

/// Explicit playback exit must retire a channel still waiting for its first
/// catalogue batch as well as the receipt for a previously admitted start.
pub(super) fn cancel_pending_play(runtime: &mut Runtime) {
    supersede_deferred(runtime);
    runtime.command_receipts.play_response = None;
    runtime.command_receipts.catalog_play = None;
}

fn deferred_ready(
    deadline: u64,
    now: u64,
    current: bool,
    allowed: bool,
    loading: bool,
    empty: bool,
) -> Result<bool, &'static str> {
    if deadline <= now {
        return Err("Channel request expired");
    }
    if !current {
        return Err("Channel source or request changed");
    }
    if !allowed {
        return Err("Playback is suspended");
    }
    Ok(!loading || !empty)
}

fn drain_deferred(runtime: &mut Runtime, app: &mut App) {
    let Some(pending) = runtime.command_receipts.deferred_channel.take() else {
        return;
    };
    let current = pending.catalog_generation == runtime.generation
        && pending.last_input == app.last_input
        && pending.context == selection_context(runtime)
        && runtime.remote.invocation_is_current(&pending.invocation);
    match deferred_ready(
        pending.invocation.deadline_ms,
        runtime.now_ms(),
        current,
        app.running && super::playback_allowed(app),
        runtime.catalog_loading,
        controls::channels(runtime).is_empty(),
    ) {
        Ok(false) => runtime.command_receipts.deferred_channel = Some(pending),
        Ok(true) => {
            if let Some(response) = command(runtime, app, &pending.invocation) {
                runtime.remote.complete(pending.invocation, response);
            }
        }
        Err(error) => runtime.remote.complete(pending.invocation, rejected(error)),
    }
}
struct Receipt {
    token: String,
    signature: String,
    context: String,
    number: usize,
    expires: u64,
}
impl Receipts {
    fn record(
        &mut self,
        id: &str,
        signature: String,
        context: String,
        number: usize,
        now: u64,
    ) -> String {
        self.rows.retain(|r| r.expires > now);
        let token = format!("revision:{id}");
        self.rows.retain(|r| r.token != token);
        if self.rows.len() >= 64 {
            self.rows.remove(0);
        }
        self.rows.push(Receipt {
            token: token.clone(),
            signature,
            context,
            number,
            expires: now.saturating_add(120000),
        });
        token
    }
    fn consume(
        &mut self,
        token: &str,
        signature: &str,
        context: &str,
        number: usize,
        now: u64,
    ) -> bool {
        let Some(i) = self.rows.iter().position(|r| r.token == token) else {
            return false;
        };
        let r = self.rows.remove(i);
        r.expires > now && r.signature == signature && r.context == context && r.number == number
    }
}

fn wire_provider(kind: ProviderKind) -> &'static str {
    if kind == ProviderKind::Local {
        "offline"
    } else {
        kind.canonical_id()
    }
}
/// Current in-memory Saved and source ownership, not a storage read or a new
/// durability proof. Only closed provider names and presence flags leave here.
fn provider_context(runtime: &Runtime) -> Value {
    let selected = runtime
        .saved
        .settings
        .get("selectedProvider")
        .and_then(Value::as_str)
        .and_then(provider_kind);
    json!({
        "loaded": runtime.loaded,
        "write_permit_valid": runtime.write_permit.is_valid(),
        "state_load": match runtime.state_load {
            StateLoad::Idle => "idle",
            StateLoad::Loading => "loading",
            StateLoad::Failed(_) => "failed",
        },
        "selected_provider": selected.map(wire_provider),
        "saved_active_profile_present": runtime.saved.profiles.active_profile.is_some(),
        "saved_active_profile_kind": runtime.active().map(|profile| wire_provider(profile.kind)),
        "runtime_provider": wire_provider(runtime.provider),
        "effective_provider": wire_provider(runtime.effective_provider()),
        "managed_active": managed::profile(runtime).is_some(),
        "classic_active": classic_source::profile(runtime).is_some(),
    })
}

fn provider_kind(value: &str) -> Option<ProviderKind> { ProviderKind::from_id(value) }
fn kinds() -> Vec<ProviderKind> { ProviderKind::all() }

/// Explicit IDs/indices outrank labels; complete labels outrank partial names.
/// A provider name containing another provider's ID must not make that ID ambiguous.
fn provider_matches(query: &str) -> Vec<ProviderKind> {
    let candidates: Vec<_> = kinds().into_iter().enumerate().filter_map(|(i, kind)| {
        let label = provider_name(kind).to_lowercase();
        let rank = if query == wire_provider(kind) || query == kind.canonical_id()
            || query == i.to_string() { 2 }
            else if !query.is_empty() && query == label { 1 }
            else if !query.is_empty() && label.contains(query) { 0 }
            else { return None; };
        Some((rank, kind))
    }).collect();
    let best = candidates.iter().map(|(rank, _)| *rank).max();
    candidates.into_iter().filter(|(rank, _)| Some(*rank) == best)
        .map(|(_, kind)| kind).collect()
}

fn provider_candidate(runtime: &Runtime, kind: ProviderKind,
    servers: &[super::ott_plex_setup::PlexSetupServer]) -> Result<Saved, &'static str>
{
    let mut saved = runtime.saved.clone();
    let usable = |profile: &&ProviderProfile| profile.enabled && profile.kind == kind
        && !profile.source.is_empty();
    let mut active = saved.profiles.profiles.iter().find(usable).map(|p| p.id.clone());
    if kind == ProviderKind::Plex && !servers.is_empty()
        && !saved.profiles.profiles.iter().any(|p| p.enabled && p.kind == kind
            && servers.iter().any(|server| server.machine_id == p.source))
    {
        if let super::ott_plex_setup::PlexSetupPlan::Profiles { updated, first_profile_id, .. } =
            super::ott_plex_setup::plan(&saved.profiles, servers)?
        {
            saved.profiles = updated;
            active = Some(first_profile_id);
        }
    } else if kind == ProviderKind::Plex && !servers.is_empty() {
        // Do not let an older stale row precede a currently authorized source.
        active = saved.profiles.profiles.iter().find(|p| p.enabled && p.kind == kind
            && servers.iter().any(|server| server.machine_id == p.source)).map(|p| p.id.clone());
    }
    saved.profiles.active_profile = active;
    saved.settings.insert("selectedProvider".into(), json!(kind.canonical_id()));
    Ok(saved)
}


pub(super) fn provider_name(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::M3u => "M3U",
        ProviderKind::VPortal => "VPortal",
        ProviderKind::Local => "Local videos",
        ProviderKind::Plex => "Plex",
        ProviderKind::Xtream => "Xtream",
        ProviderKind::Edem => "Edem.tv / iLook.tv",
        ProviderKind::Operator(id) => id.name(),
        ProviderKind::Stalker => "Stalker",
    }
}
pub(super) fn default_profile_name(kind:ProviderKind,number:usize)->String {
    if number==1 && matches!(kind,ProviderKind::Edem|ProviderKind::Operator(_)){provider_name(kind).into()}else{format!("{} {number}",provider_name(kind))}
}

fn fields(value: &Value, keys: &[&str]) -> bool {
    value
        .as_object()
        .is_some_and(|map| map.len() == keys.len() && keys.iter().all(|key| map.contains_key(*key)))
}
fn subset(value: &Value, keys: &[&str]) -> bool {
    value
        .as_object()
        .is_some_and(|map| !map.is_empty() && map.keys().all(|key| keys.contains(&key.as_str())))
}
fn text(value: &Value, limit: usize, empty: bool) -> Option<&str> {
    value.as_str().filter(|s| {
        s.len() <= limit && (empty || !s.trim().is_empty()) && !s.chars().any(char::is_control)
    })
}
fn unsupported(message: &str) -> Value {
    json!({"status":"unsupported","data":{"error":message}})
}
pub(super) fn profile_slot_number(saved: &Saved, profile: &ProviderProfile, ordinal: usize) -> u64 {
    saved
        .settings
        .get(&format!("slot:{}", profile.id))
        .and_then(Value::as_u64)
        .unwrap_or((ordinal + 1) as u64)
}

fn profile_index(saved: &Saved, kind: ProviderKind, number: usize) -> Option<usize> {
    saved
        .profiles
        .profiles
        .iter()
        .enumerate()
        .filter(|(_, p)| p.kind == kind)
        .enumerate()
        .find(|(legacy, (_, profile))| {
            profile_slot_number(saved, profile, *legacy) == number as u64
        })
        .map(|(_, (index, _))| index)
}

/// Give a new UI profile an unused wire slot without shifting existing legacy
/// ordinals or explicit remote slots. Called on the candidate before appending.
pub(super) fn assign_profile_slot(
    saved: &mut Saved,
    profile: &ProviderProfile,
) -> Result<(), &'static str> {
    let number = (1..=15)
        .find(|number| profile_index(saved, profile.kind, *number).is_none())
        .ok_or("This provider already has 15 profiles")?;
    saved
        .settings
        .insert(format!("slot:{}", profile.id), json!(number));
    Ok(())
}

fn number(params: &Value) -> Result<usize, &'static str> {
    params["number"]
        .as_u64()
        .filter(|n| (1..=15).contains(n))
        .map(|n| n as usize)
        .ok_or("Choose a profile number from 1 to 15")
}
fn metadata(saved: &Saved, kind: ProviderKind, number: usize) -> Value {
    let profile = profile_index(saved, kind, number).map(|i| &saved.profiles.profiles[i]);
    let mut result = json!({"number":number,"name":profile.map(|p|p.name.as_str()).unwrap_or(""),
        "active":profile.is_some_and(|p|Some(&p.id)==saved.profiles.active_profile.as_ref()),
        "playlist_configured":kind==ProviderKind::M3u && profile.is_some_and(|p|
            plx_data::ott::m3u_source_kind(&p.source)==Ok(plx_data::ott::M3uSourceKind::Playlist)),
        "history_hours":profile.and_then(|p|saved.settings.get(&format!("history:{}",p.id))).and_then(Value::as_u64).unwrap_or(0)});
    if kind == ProviderKind::VPortal {
        result["vportal_configured"] = json!(profile.is_some_and(|p| !p.source.is_empty()));
    }
    result
}
fn hash(value: &Value) -> String {
    let bytes = plx_base::sha256::sha256(value.to_string().as_bytes());
    use std::fmt::Write;
    let mut result = String::with_capacity(64);
    for byte in bytes {
        let _ = write!(&mut result, "{byte:02x}");
    }
    result
}
fn revision(saved: &Saved, kind: ProviderKind, number: usize) -> String {
    let profile = profile_index(saved, kind, number).map(|i| &saved.profiles.profiles[i]);
    hash(&json!({"profile":profile,"metadata":metadata(saved,kind,number)}))
}
fn context(runtime: &Runtime) -> String {
    hash(&json!([
        runtime.remote.runtime(),
        runtime.saved.profiles.active_profile,
        wire_provider(runtime.provider)
    ]))
}
fn defer(
    runtime: &mut Runtime,
    inv: &Invocation,
    saved: Saved,
    reload: bool,
    response: Value,
) -> Option<Value> {
    match runtime.save_reply(saved, inv.clone(), reload, response) {
        Ok(()) => None,
        Err(error) => Some(rejected(error)),
    }
}
fn insert_profile(
    saved: &mut Saved,
    kind: ProviderKind,
    number: usize,
    source: String,
    name: Option<String>,
) -> Result<usize, &'static str> {
    if !(1..=15).contains(&number) {
        return Err("Choose a profile number from 1 to 15");
    }
    let existing = profile_index(saved, kind, number);
    let mut profile = if let Some(index) = existing {
        saved.profiles.profiles[index].clone()
    } else {
        let mut id = format!("{}-{number}", kind.canonical_id().replace('/', "_"));
        while saved.profiles.profiles.iter().any(|p| p.id == id) {
            id.push('_');
        }
        ProviderProfile {
            id,
            name: default_profile_name(kind,number),
            kind,
            source: String::new(),
            source_alias: String::new(),
            enabled: true,
        }
    };
    profile.source = source;
    if let Some(name) = name {
        profile.name = if name.trim().is_empty() {
            default_profile_name(kind,number)
        } else {
            name
        };
    }
    profile
        .validate_stored()
        .map_err(|_| "Invalid provider source or profile name")?;
    // Freeze all previous ordinal slots before adding a sparse numbered slot.
    let ordinals: Vec<_> = saved
        .profiles
        .profiles
        .iter()
        .filter(|p| p.kind == kind)
        .enumerate()
        .map(|(i, p)| (p.id.clone(), i + 1))
        .collect();
    for (id, n) in ordinals {
        saved
            .settings
            .entry(format!("slot:{id}"))
            .or_insert(json!(n));
    }
    saved
        .settings
        .insert(format!("slot:{}", profile.id), json!(number));
    if let Some(index) = existing {
        saved.profiles.profiles[index] = profile;
        Ok(index)
    } else {
        saved.profiles.profiles.push(profile);
        Ok(saved.profiles.profiles.len() - 1)
    }
}

fn profiles(runtime: &mut Runtime, inv: &Invocation) -> Option<Value> {
    if inv.action == "profile_settings"
        && matches!(
            runtime.provider,
            ProviderKind::Xtream | ProviderKind::Stalker | ProviderKind::Edem | ProviderKind::Operator(_)
        )
    {
        return provider_adapters::command(runtime, inv);
    }
    if runtime.effective_provider() == ProviderKind::Local {
        return offline::request(runtime, inv);
    }
    let p = &inv.params;
    if inv.action == "profiles" {
        return Some(if fields(p, &[]) {
            ok(
                json!({"provider":wire_provider(runtime.provider),"profiles":(1..=15).map(|n|metadata(&runtime.saved,runtime.provider,n)).collect::<Vec<_>>()}),
            )
        } else {
            rejected("The profiles query accepts no parameters")
        });
    }
    let n = match number(p) {
        Ok(n) => n,
        Err(e) => return Some(rejected(e)),
    };
    let mut saved = runtime.saved.clone();
    if inv.action == "profile" {
        if !fields(p, &["number"]) {
            return Some(rejected("Profile selection accepts only a number"));
        }
        let Some(index) = profile_index(&saved, runtime.provider, n) else {
            return Some(rejected("Configure this profile before selecting it"));
        };
        let profile = &saved.profiles.profiles[index];
        if !profile.enabled {
            return Some(rejected("This profile is disabled"));
        }
        if profile.source.is_empty() {
            return Some(rejected("Configure this profile before selecting it"));
        }
        let reload = saved.profiles.active_profile.as_ref() != Some(&profile.id);
        saved.profiles.active_profile = Some(profile.id.clone());

        let response = ok(
            json!({"provider":wire_provider(runtime.provider),"profile":metadata(&saved,runtime.provider,n),"dispatched":true,"reloaded":reload}),
        );
        return defer(runtime, inv, saved, reload, response);
    }
    if inv.action != "profile_settings" {
        return Some(unsupported(
            "Portable local profile bindings require a native binding adapter",
        ));
    }
    let allowed = match runtime.provider {
        ProviderKind::M3u => &["name", "playlist", "history_hours"][..],
        ProviderKind::VPortal => &["name", "vportal"][..],
        _ => &[][..],
    };
    if !fields(p, &["number", "settings"]) || !subset(&p["settings"], allowed) {
        return Some(rejected("Unsupported profile settings fields"));
    }
    let settings = &p["settings"];
    let current =
        profile_index(&saved, runtime.provider, n).map(|i| saved.profiles.profiles[i].clone());
    let source_key = if runtime.provider == ProviderKind::VPortal {
        "vportal"
    } else {
        "playlist"
    };
    let source = if let Some(source) = settings.get(source_key) {
        match text(source, 8192, true) {
            Some(s) => s.to_owned(),
            None => return Some(rejected("Use a valid playlist URL")),
        }
    } else {
        current
            .as_ref()
            .map(|p| p.source.clone())
            .unwrap_or_default()
    };
    if current.is_none() && !settings.as_object().unwrap().contains_key(source_key) {
        return Some(rejected("Use a valid playlist URL"));
    }
    let name = if let Some(name) = settings.get("name") {
        match text(name, 256, true) {
            Some(s) => Some(s.to_owned()),
            None => return Some(rejected("Invalid profile name")),
        }
    } else {
        None
    };
    let index = match insert_profile(&mut saved, runtime.provider, n, source, name) {
        Ok(i) => i,
        Err(e) => return Some(rejected(e)),
    };
    if let Some(hours) = settings.get("history_hours") {
        let Some(hours) = hours.as_u64().filter(|n| *n <= 8760) else {
            return Some(rejected("Use whole archive hours from 0 to 8760"));
        };
        saved.settings.insert(
            format!("history:{}", saved.profiles.profiles[index].id),
            json!(hours),
        );
    }
    let reload = !saved.profiles.profiles[index].source.is_empty()
        && saved.profiles.active_profile.as_ref() == Some(&saved.profiles.profiles[index].id);
    let changed =
        saved.profiles != runtime.saved.profiles || saved.settings != runtime.saved.settings;
    let response = ok(
        json!({"provider":wire_provider(runtime.provider),"profile":metadata(&saved,runtime.provider,n),"saved":true,"fields":settings.as_object().unwrap().keys().collect::<Vec<_>>(),"reloaded":reload&&changed}),
    );
    defer(runtime, inv, saved, reload && changed, response)
}
fn provider_settings(runtime: &mut Runtime, inv: &Invocation) -> Option<Value> {
    if provider_config::handles(inv) { return provider_config::command(runtime, inv); }
    let now = runtime.now_ms();
    let p = &inv.params;
    let Some(kind) = p["provider"].as_str().and_then(provider_kind) else {
        return Some(rejected("Unknown provider"));
    };
    if matches!(kind, ProviderKind::Xtream | ProviderKind::Stalker | ProviderKind::Edem | ProviderKind::Operator(_)) {
        return provider_adapters::command(runtime, inv);
    }
    if kind == ProviderKind::Plex {
        return plex_settings(runtime, inv);
    }
    let compare = inv.action == "settings_compare";
    let expected = match (kind, compare) {
        (ProviderKind::VPortal, true) => &["provider", "number", "runtime", "settings"][..],
        (ProviderKind::VPortal, false) => {
            &["provider", "runtime", "context", "revision", "settings"][..]
        }
        (_, true) => &["provider", "number", "settings"][..],
        (_, false) => &["provider", "settings"][..],
    };
    if !fields(p, expected) {
        return Some(rejected("Invalid provider settings parameters"));
    }
    let settings = &p["settings"];
    let n = if kind == ProviderKind::VPortal && !compare {
        settings["profile"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
    } else {
        p["number"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .or_else(|| {
                (!compare).then(|| {
                    (1..=15)
                        .find(|n| {
                            profile_index(&runtime.saved, kind, *n).is_some_and(|i| {
                                Some(&runtime.saved.profiles.profiles[i].id)
                                    == runtime.saved.profiles.active_profile.as_ref()
                            })
                        })
                        .unwrap_or(1)
                })
            })
    };
    let Some(n) = n.filter(|n| (1..=15).contains(n)) else {
        return Some(rejected("Choose a profile number from 1 to 15"));
    };
    if kind == ProviderKind::VPortal {
        if p["runtime"] != runtime.remote.runtime() {
            return Some(rejected("The player runtime changed"));
        }
        let allowed = if compare {
            &["name", "vportal"][..]
        } else {
            &["profile", "name", "vportal"][..]
        };
        if !subset(settings, allowed)
            || text(&settings["vportal"], 8192, true).is_none()
            || settings
                .get("name")
                .is_some_and(|v| text(v, if compare { 512 } else { 256 }, true).is_none())
            || (!compare && settings.get("profile").is_none())
        {
            return Some(rejected("Unsupported VPortal settings fields"));
        }
    } else if kind == ProviderKind::M3u {
        let allowed = if compare {
            &["name", "playlist", "history_hours"][..]
        } else {
            &["playlist"][..]
        };
        if !subset(settings, allowed) {
            return Some(rejected("Unsupported M3U settings fields"));
        }
    } else {
        return Some(unsupported(
            "Select a native local folder on this installation",
        ));
    }
    let current =
        profile_index(&runtime.saved, kind, n).map(|i| &runtime.saved.profiles.profiles[i]);
    if compare {
        let matched = if kind == ProviderKind::VPortal {
            current.is_some_and(|c| {
                settings["vportal"] == c.source && settings.get("name").is_none_or(|n| *n == c.name)
            })
        } else {
            current.is_some_and(|c| {
                settings["playlist"] == c.source
                    && settings["name"] == c.name
                    && settings["history_hours"]
                        == metadata(&runtime.saved, kind, n)["history_hours"]
            })
        };
        let mut data = json!({"provider":wire_provider(kind),"number":n,"matched":matched});
        if kind == ProviderKind::VPortal {
            data["runtime"] = json!(runtime.remote.runtime());
            let ctx = context(runtime);
            let signature = revision(&runtime.saved, kind, n);
            data["context"] = json!(ctx);
            data["revision"] = json!(runtime
                .command_receipts
                .record(&inv.id, signature, ctx, n, now));
        }
        return Some(ok(data));
    }
    if kind == ProviderKind::VPortal {
        let ctx = context(runtime);
        let signature = revision(&runtime.saved, kind, n);
        if p["context"] != ctx
            || !runtime.command_receipts.consume(
                p["revision"].as_str().unwrap_or(""),
                &signature,
                &ctx,
                n,
                now,
            )
        {
            return Some(rejected("The settings source changed before saving"));
        }
    }
    if kind == ProviderKind::M3u && runtime.provider != kind {
        return Some(rejected(
            "Select this provider before changing its settings",
        ));
    }
    let source_key = if kind == ProviderKind::VPortal {
        "vportal"
    } else {
        "playlist"
    };
    let source = if let Some(value) = settings.get(source_key) {
        match text(value, 16384, true) {
            Some(s) => s.to_owned(),
            None => return Some(rejected("Invalid provider source")),
        }
    } else {
        current.map(|c| c.source.clone()).unwrap_or_default()
    };
    let name = if let Some(value) = settings.get("name") {
        match text(value, 256, true) {
            Some(s) => Some(s.to_owned()),
            None => return Some(rejected("Invalid profile name")),
        }
    } else {
        None
    };
    let mut saved = runtime.saved.clone();
    let index = match insert_profile(&mut saved, kind, n, source, name) {
        Ok(i) => i,
        Err(e) => return Some(rejected(e)),
    };
    if kind == ProviderKind::M3u
        && !saved.profiles.profiles[index].source.is_empty()
        && saved.profiles.active_profile.is_none()
    {
        saved.profiles.active_profile = Some(saved.profiles.profiles[index].id.clone());
    }
    let reload = !saved.profiles.profiles[index].source.is_empty()
        && saved.profiles.active_profile.as_ref() == Some(&saved.profiles.profiles[index].id);
    let mut data = json!({"provider":wire_provider(kind),"fields":settings.as_object().unwrap().keys().collect::<Vec<_>>(),"saved":true});
    if kind == ProviderKind::VPortal {
        data["runtime"] = json!(runtime.remote.runtime());
        data["profile"] = json!(n);
        data["context"] = json!(context(runtime));
        let signature = revision(&saved, kind, n);
        let ctx = context(runtime);
        data["revision"] = json!(runtime
            .command_receipts
            .record(&inv.id, signature, ctx, n, now));
        data["reloaded"] = json!(reload);
        data["reload_required"] = json!(false);
    }
    defer(runtime, inv, saved, reload, ok(data))
}
pub(super) fn stored_plex(runtime: &Runtime) -> Option<plx_plex::plex::session::SourceRef> {
    let sources = runtime.saved.plex_sources().ok()?;
    let active = runtime.active().filter(|p| p.kind == ProviderKind::Plex);
    sources
        .into_iter()
        .rev()
        .find(|s| active.is_none_or(|p| p.source == s.machine_id))
}
fn plex_settings(runtime: &mut Runtime, inv: &Invocation) -> Option<Value> {
    let p = &inv.params;
    let compare = inv.action == "settings_compare";
    let settings = &p["settings"];
    if !fields(p, &["provider", "settings"])
        || !(if compare {
            fields(settings, &["server", "token"])
        } else {
            subset(settings, &["server", "token"])
        })
        || settings
            .get("server")
            .is_some_and(|v| text(v, 8192, true).is_none())
        || settings
            .get("token")
            .is_some_and(|v| text(v, 1024, true).is_none())
    {
        return Some(rejected("Unsupported Plex settings fields"));
    }
    let current = stored_plex(runtime);
    if compare {
        let matched = current.is_some_and(|s| {
            let expected = plx_plex::plex::Origin::parse(
                settings["server"]
                    .as_str()
                    .unwrap()
                    .trim()
                    .trim_end_matches('/'),
            );
            expected.is_some()
                && expected.as_ref() == s.origin().as_ref()
                && settings["token"].as_str().unwrap().trim() == s.token
        });
        return Some(ok(json!({"provider":"plex","matched":matched})));
    }
    if runtime.provider != ProviderKind::Plex {
        return Some(rejected("Select Plex before changing its settings"));
    }
    let address = settings["server"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| current.as_ref().map(|s| s.origin_url.clone()))
        .unwrap_or_default();
    let token = settings["token"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| current.as_ref().map(|s| s.token.clone()))
        .unwrap_or_default();
    // An encrypted previously verified source may carry its original HTTP consent.
    // A remote command cannot silently grant plaintext access to a different origin.
    let expected = plx_plex::plex::Origin::parse(&address);
    let same = current
        .as_ref()
        .filter(|s| expected.is_some() && s.origin().as_ref() == expected.as_ref());
    let profile = crate::app::ott_profile::Profile::parse(
        json!({"address":address,"token":token,
        "serverId":same.map(|s|s.machine_id.as_str()).unwrap_or(""),
        "name":current.as_ref().map(|s|s.name.as_str()).unwrap_or("OTTPlay Plex"),
        "allowInsecureHttp":same.is_some_and(|s|s.origin().is_some_and(|o|!o.is_tls()))})
        .to_string()
        .as_bytes(),
    );
    match profile.and_then(|profile| runtime.import_plex(profile, inv.clone())) {
        Ok(()) => None,
        Err(e) => Some(rejected(e)),
    }
}
pub(super) fn offline_rows(runtime: &Runtime) -> Vec<(usize, String)> {
    offline::unbound_rows(&runtime.saved)
}
pub(super) fn offline_selected_resume(saved: &Saved) -> Option<continuity::ResumePoint> {
    offline::selected_resume(saved)
}
pub(super) fn bind_local(
    runtime: &Runtime,
    number: usize,
    path: &str,
) -> Result<Saved, &'static str> {
    let mut candidate = offline::bind(&runtime.saved, number, path)?;
    let index = profile_index(&candidate, ProviderKind::Local, number)
        .ok_or("Bound local profile is unavailable")?;
    candidate.profiles.active_profile = Some(candidate.profiles.profiles[index].id.clone());
    Ok(candidate)
}
fn input_key(value: &str) -> Option<(Key, u32, u32)> {
    use plx_ui::consts::{WCODE_CH_DOWN_KEY, WCODE_CH_UP_KEY};
    match value {
        "up" => Some((Key::Up, 0, 0)),
        "down" => Some((Key::Down, 0, 0)),
        "left" => Some((Key::Left, 0, 0)),
        "right" => Some((Key::Right, 0, 0)),
        "ok" => Some((Key::Ok, 0, 0)),
        "back" => Some((Key::Back, 0, 0)),
        "channel_up" | "ch+" => Some((Key::Other, 0, WCODE_CH_UP_KEY)),
        "channel_down" | "ch-" => Some((Key::Other, 0, WCODE_CH_DOWN_KEY)),
        _ => None,
    }
}
pub(super) fn selected_ids(runtime: &Runtime) -> Vec<String> {
    controls::channels(runtime)
        .into_iter()
        .map(|row| row.id)
        .collect()
}
/// Shared list navigation for physical input and authenticated commands.
pub(super) fn step_in_list(ids: &[String], selected: Option<&str>, offset: i64) -> Option<String> {
    controls::step(ids, selected, offset)
}
/// Readiness is owned by the common transport policy, not individual inputs.
pub(super) fn playback_ready(app: &App) -> bool {
    controls::transport_ready(app)
}
fn select_step(runtime: &Runtime, offset: i64) -> Option<String> {
    if let Some(id) = super::transient::plex_remote::step_id(runtime, offset) {
        return id;
    }
    controls::step(&selected_ids(runtime), runtime.selected.as_deref(), offset)
}
fn play_reply(
    runtime: &mut Runtime,
    app: &mut App,
    id: &str,
    inv: &Invocation,
) -> Result<(), &'static str> {
    let channel = controls::channels(runtime)
        .into_iter()
        .find(|row| row.id == id)
        .ok_or("Channel is no longer in the selected list")?;
    play_channel_reply(runtime, app, id, &channel, inv)
}
fn play_channel_reply(
    runtime: &mut Runtime,
    app: &mut App,
    id: &str,
    channel: &controls::Channel,
    inv: &Invocation,
) -> Result<(), &'static str> {
    let mut response = json!({"dispatched":true,"channel":channel});
    if inv.action == "playback" {
        response["operation"] = inv.params["operation"].clone();
        if inv.params["operation"] == "step_channel" {
            response["offset"] = inv.params["offset"].clone();
        }
    }
    supersede_deferred(runtime);
    runtime.command_receipts.catalog_play = None;
    runtime.play_reply(app, id, inv)?;
    runtime.command_receipts.play_response = Some((inv.id.clone(), ok(response)));
    Ok(())
}
/// Recheck receipt source identity after asynchronous resolution, before native admission.
pub(super) fn pending_catalog_current(runtime: &Runtime) -> bool {
    catalog_remote::pending_current(runtime)
}
/// Exact remote selection must not fall through to a different item on failure.
pub(super) fn catalog_failure(runtime: &mut Runtime, app: &mut App) -> bool {
    catalog_remote::play_failed(runtime, app)
}
/// Called only after the matching asynchronous native start was accepted.
pub(super) fn play_response(runtime: &Runtime, inv: &Invocation) -> Value {
    runtime
        .command_receipts
        .play_response
        .as_ref()
        .filter(|(id, _)| id == &inv.id)
        .map(|(_, response)| response.clone())
        .unwrap_or_else(|| ok(json!({"dispatched":true})))
}
pub(super) fn kiosk_pending(runtime: &Runtime) -> bool {
    kiosk::pending(runtime)
}
pub(super) fn kiosk_settled(
    runtime: &mut Runtime,
    app: &mut App,
    invocation: &Invocation,
    outcome: KioskSettlement,
) -> Option<Value> {
    kiosk::settled(runtime, app, invocation, outcome)
}
pub(super) fn settings_after_saved(
    runtime: &mut Runtime,
    app: &mut App,
) -> Result<(), &'static str> {
    settings::after_saved(runtime, app)
}
fn playback_command(runtime: &mut Runtime, app: &mut App, inv: &Invocation) -> Option<Value> {
    if !super::playback_allowed(app) {
        return Some(rejected("Playback is suspended"));
    }
    let p = &inv.params;
    let operation = p["operation"].as_str().unwrap_or("");
    if multiview_owns_decoder_operation(operation)
        && super::multiview_output::owns_picture(runtime, app)
    {
        runtime.status = "Per-window transport is unavailable for this source".into();
        runtime.changed();
        return Some(unsupported("Per-window transport is unavailable for multiview output"));
    }
    if matches!(
        operation,
        "previous_channel" | "next_channel" | "step_channel"
    ) {
        let offset = if operation == "step_channel" {
            p["offset"]
                .as_i64()
                .filter(|v| *v != 0 && v.unsigned_abs() <= 9007199254740991)
        } else {
            Some(if operation == "previous_channel" {
                -1
            } else {
                1
            })
        };
        if !fields(
            p,
            if operation == "step_channel" {
                &["operation", "offset"]
            } else {
                &["operation"]
            },
        ) || offset.is_none()
        {
            return Some(rejected("Invalid channel movement"));
        }
        let Some(id) = select_step(runtime, offset.unwrap()) else {
            return Some(rejected("No channel is available"));
        };
        return match play_reply(runtime, app, &id, inv) {
            Ok(()) => None,
            Err(e) => Some(rejected(e)),
        };
    }
    if !controls::transport_ready(app) {
        return Some(rejected("No active playback"));
    }
    if operation == "pause" || operation == "resume" {
        if !fields(p, &["operation"]) {
            return Some(rejected("Playback operation accepts no other parameters"));
        }
        let desired = operation == "pause";
        if !controls::set_paused(&mut app.adapters.player, desired) {
            return Some(rejected("The decoder refused the playback operation"));
        }
        runtime.viewer_transport(app);
        return Some(ok(json!({"dispatched":true,"operation":operation})));
    }
    if operation == "seek" || operation == "seek_relative" {
        let key = if operation == "seek" {
            "position"
        } else {
            "offset"
        };
        if !fields(p, &["operation", key]) {
            return Some(rejected("Invalid seek parameters"));
        }
        let Some(value) = p[key]
            .as_f64()
            .filter(|v| v.is_finite() && v.abs() <= 9007199254740991.0)
        else {
            return Some(rejected("Use a finite seek position"));
        };
        let duration = playback::dur();
        if duration <= 0
            || (operation == "seek" && value < 0.0)
            || (operation == "seek_relative" && value == 0.0)
        {
            return Some(unsupported("Seeking requires a finite video timeline"));
        }
        // Absolute seek receipts must echo the requested position exactly. A
        // clamped success changes playback but is rejected by the controller.
        if operation == "seek" && value > duration as f64 / 1e9 {
            return Some(rejected("Seek position exceeds video duration"));
        }
        let from = playback::intended_pos(&app.player.session).max(0) as f64 / 1e9;
        // This wire command addresses the finite decoder timeline, including
        // a finite archive file. Broadcast-epoch seeking has a separate owner.
        // Source/decoder admission above remains native; the canonical core
        // decides the media position and preserves this contract's clamp policy.
        let (source_id, channel_id) = runtime
            .playing
            .as_ref()
            .filter(|playing| playing_matches(playing, app))
            .map(|playing| {
                (
                    profile_fingerprint(&playing.profile),
                    playing.entry.id.clone(),
                )
            })
            .unwrap_or_else(|| {
                (
                    format!(
                        "plex:{}",
                        plx_media::route::cur_sid(&app.player.session).raw()
                    ),
                    format!("item:{}", plx_media::route::cur_rk(&app.player.session)),
                )
            });
        let target = plx_shared_core::SeekTarget {
            source_id,
            channel_id,
            kind: plx_shared_core::PlaybackKind::Vod,
            archive_start: None,
        };
        let request = plx_shared_core::SeekRequest::finite(
            if operation == "seek" {
                plx_shared_core::SeekIntent::Absolute
            } else {
                plx_shared_core::SeekIntent::Offset
            },
            value,
            from,
            duration as f64 / 1e9,
        );
        let position = match plx_shared_core::seek_plan(Some(&target), &request) {
            Ok(plx_shared_core::SeekPlan::Seek { position }) => position,
            Ok(_) => return Some(unsupported("Seeking is unavailable for this video")),
            Err(_) => return Some(rejected("Unable to calculate the seek position")),
        };
        let requested = if operation == "seek" {
            value
        } else {
            from + value
        };
        playback::commit_seek((position * 1e9) as i64);
        runtime.viewer_seek(app);
        let mut data = json!({"dispatched":true,"operation":operation,"position":position});
        if operation == "seek_relative" {
            data["from"] = json!(from);
            data["offset"] = json!(value);
            data["clamped"] = json!(position != requested);
        }
        return Some(ok(data));
    }
    Some(unsupported("Unsupported native playback operation"))
}

fn multiview_owns_decoder_operation(operation: &str) -> bool {
    matches!(operation,
        "previous_channel" | "next_channel" | "step_channel" | "pause" | "resume" | "seek" | "seek_relative")
}

#[cfg(all(test, feature = "hostsim"))]
mod multiview_transport_guard_tests {
    use super::multiview_owns_decoder_operation;

    #[test]
    fn remote_decoder_transport_is_separated_from_read_and_lifecycle_commands() {
        for operation in ["previous_channel", "next_channel", "step_channel", "pause", "resume", "seek", "seek_relative"] {
            assert!(multiview_owns_decoder_operation(operation), "{operation}");
        }
        for operation in ["status", "volume", "mute", "notifications", "exit_app", "unknown"] {
            assert!(!multiview_owns_decoder_operation(operation), "{operation}");
        }
    }
}

/// Keep individual-setting failures inside their existing typed wire contract.
/// Other command families retain their own response shapes.
pub(super) fn settings_public_saved(runtime: &mut Runtime, inv: &Invocation, response: &mut Value) {
    settings::public_saved(runtime, inv, response);
}

pub(super) fn settings_failure(inv: &Invocation, reason: &str) -> Option<Value> {
    provider_config::failure(inv, reason)
        .or_else(|| (inv.action == "player_settings").then(|| settings::failure(&inv.params, reason)))
}

/// Early fences use the same strict raw request as the track handler. A lossy
/// params Value must never repair duplicate keys or mint a different echo.
fn media_tracks_failure(
    inv: &Invocation,
    reason: crate::app::ott_remote::media_tracks::Error,
) -> Option<Value> {
    if inv.action != "media_tracks" {
        return None;
    }
    let request =
        crate::app::ott_remote::media_tracks::Request::decode(inv.media_tracks_projection()?)
            .ok()?;
    Some(request.error_response(reason).into_envelope())
}

pub(super) fn policy_rejection(inv: &Invocation, error: &str) -> Value {
    if inv.action == "plex_servers" {
        if let Some(raw) = inv.plex_servers_projection() {
            return match crate::app::ott_remote::plex_servers::Request::decode(raw) {
                Ok(request) => request
                    .correlation()
                    .rejected(crate::app::ott_remote::plex_servers::Error::Restricted),
                Err(error) => error.response(),
            };
        }
    }
    if let Some(response) =
        media_tracks_failure(inv, crate::app::ott_remote::media_tracks::Error::Restricted)
    {
        return response;
    }
    if inv.action == favorites_items::ACTION {
        return favorites_items::error(inv, "policy_denied");
    }
    if inv.action == "favorites_global_recovery" {
        return favorites_recovery::failure(inv, "policy_denied");
    }
    settings_failure(inv, "policy_restricted").unwrap_or_else(|| rejected(error))
}

pub(super) fn command(runtime: &mut Runtime, app: &mut App, inv: &Invocation) -> Option<Value> {
    runtime.remote.note_command(inv, runtime.now_ms());
    if inv.action == "multiview" {
        let mut driver = std::mem::take(&mut runtime.multiview_driver);
        let response = driver.command(runtime, app, inv);
        runtime.multiview_driver = driver;
        return response;
    }
    if inv.action == "native_playback_diagnostics" {
        if runtime.now_ms() >= inv.deadline_ms {
            return Some(super::native_playback_diagnostics::failure(
                inv,
                "invalid_request",
            ));
        }
        if input_policy::remote(runtime, inv).is_err() {
            return Some(super::native_playback_diagnostics::failure(
                inv,
                "policy_denied",
            ));
        }
        return Some(super::native_playback_diagnostics::command(
            runtime, app, inv,
        ));
    }
    if inv.action == "notifications" {
        return notification_settings::command(runtime, inv);
    }
    if inv.action == "plex_servers" {
        return managed::command(runtime, app, inv);
    }
    // This closed adapter is read-only; ACK is refused before IO. Its parser
    // supplies family-shaped expiry/runtime/schema errors as well.
    if inv.action == "favorites_global_recovery" {
        return favorites_recovery::command(runtime, inv);
    }
    if inv.action == "local_browser" && runtime.now_ms() >= inv.deadline_ms {
        return Some(runtime.usb_remote_failure(inv, "expired"));
    }
    if inv.action == "local_media"
        && inv.params.get("source_handle").is_some()
        && runtime.now_ms() >= inv.deadline_ms
    {
        return Some(crate::app::ott_local_session::handle_failure(
            &inv.params,
            runtime.remote.runtime(),
            crate::app::ott_local_session::LocalError::RuntimeChanged,
        ));
    }
    if runtime.now_ms() >= inv.deadline_ms {
        if inv.action == favorites_items::ACTION {
            return Some(favorites_items::error(inv, "expired"));
        }
        return Some(
            provider_config::failure(inv, "expired")
                .or_else(|| category_select_rejection(runtime, inv, "unavailable"))
                .or_else(|| {
                    media_tracks_failure(
                        inv,
                        crate::app::ott_remote::media_tracks::Error::Unavailable,
                    )
                })
                .or_else(|| settings_failure(inv, "source_changed"))
                .unwrap_or_else(|| rejected("Request expired")),
        );
    }
    if let Err(error) = super::input_policy::remote(runtime, inv) {
        if inv.action == "local_browser" {
            return Some(runtime.usb_remote_failure(inv, "restricted"));
        }
        if inv.action == "local_media" && inv.params.get("source_handle").is_some() {
            return Some(crate::app::ott_local_session::handle_failure(
                &inv.params,
                runtime.remote.runtime(),
                crate::app::ott_local_session::LocalError::Restricted,
            ));
        }
        return Some(
            category_select_rejection(runtime, inv, "restricted")
                .unwrap_or_else(|| policy_rejection(inv, error)),
        );
    }
    let p = &inv.params;
    match inv.action.as_str() {
        favorites_items::ACTION => favorites_items::command(runtime, app, inv),
        "kiosk" => kiosk::command(runtime, app, inv),
        "media_tracks" => media_tracks::command(runtime, app, inv, Runtime::track_selection_kiosk),
        "local_browser" => runtime.usb_remote_command(inv),
        "local_media" => super::transient::command(runtime, app, inv),
        "plex_queue" | "plex_library" => super::transient::plex_remote::command(runtime, app, inv),
        "state" | "status" => {
            let version = if fields(p, &[]) {
                playback_failure::Version::V2
            } else if fields(p, &["playback_failure_version"]) {
                match p["playback_failure_version"].as_u64() {
                    Some(2) => playback_failure::Version::V2,
                    Some(3) => playback_failure::Version::V3,
                    _ => return Some(rejected("playback_failure_version must be 2 or 3")),
                }
            } else {
                return Some(rejected(
                    "The state query accepts only playback_failure_version",
                ));
            };
            let mut value = state(app, runtime);
            value["row_guide"] = runtime.row_guide_diagnostics();
            if let Some(guide) = value["row_guide"].as_object_mut() {
                guide.insert("app_ui_mode".into(), json!(app.bridge.ui_mode().index()));
                guide.insert("reply_current".into(), json!(runtime.reply.is_some()
                    && runtime.reply == bridge::ott_ui_target(&app.pages)));
                guide.insert("ui".into(), bridge::ott_guide_presentation(&app.pages).unwrap_or(Value::Null));
                let metadata = app.bridge.metadata_view();
                let now = metadata.now_playing();
                let ott = now.and_then(|now| now.ott.as_ref());
                let live = ott.and_then(|ott| ott.live.as_ref());
                let programme = live.and_then(|live| live.guide.as_ref())
                    .and_then(|guide| guide.at((runtime.wall_clock_ms / 1000) as i64).0);
                let offer = app.bridge.ott_player_offer();
                let resume_key = runtime.resume_key();
                let saved_resume = resume_key.as_ref().and_then(|key| runtime.saved.resume.get(key));
                let checkpoint = resume_key.as_ref().and_then(|key| runtime.checkpoints.get(key));
                let selection = runtime.playing.as_ref().map(|playing| &playing.selection_id);
                // Presence and sizes only: diagnose a real information-panel
                // gate without exporting programme text, sources or credentials.
                guide.insert("player".into(), json!({
                    "live_channel": live.is_some(),
                    "summary_chars": now.map_or(0, |now| now.summary.chars().count()),
                    "current_programme": programme.is_some(),
                    "programme_description_chars": programme.map_or(0, |p| p.description.chars().count()),
                    "confirmed_start": ott.is_some_and(|ott| ott.confirmed_start_unix_ms.is_some()),
                    "remote_offer": offer.is_some(),
                    "recovery_only": offer.is_some_and(|offer| offer.recovery_only),
                    "footer_visible": bridge::player(&app.pages)
                        .is_some_and(|page| page.hud_up(&app.player.session, app.loop_t)),
                    "surface_up": app.pages.surface_up(),
                    "resume": {
                        "saved": saved_resume.is_some(),
                        "checkpoint": checkpoint.is_some(),
                        "saved_source_current": saved_resume.is_some_and(|value| runtime.resume_is_current(value)),
                        "saved_matches_playing": saved_resume.is_some_and(|value| selection == Some(&value.entry_id)),
                        "checkpoint_matches_playing": checkpoint.is_some_and(|value| selection == Some(&value.entry_id)),
                        "pending_restore": runtime.continuity.pending_restore().is_some(),
                        "restoring_current": runtime.restore_play_generation == Some(runtime.play_generation),
                        "saving": runtime.saving,
                        "persistence_uncertain": runtime.persistence_uncertain
                    }
                }));
            }
            value["provider_context"] = provider_context(runtime);
            value["uuid"] = json!(runtime.remote.device_id());
            if value.get("provider").is_none() {
                value["provider"] = json!(wire_provider(runtime.effective_provider()));
            }
            value["ready"] = json!(
                runtime.loaded && (!runtime.catalog_loading || !runtime.catalog.entries.is_empty())
            );
            value["channels"] = json!(runtime.catalog.entries.len());
            value["performance"] = performance_snapshot(app, runtime);
            value["catalog"] = json!(runtime.catalog_diagnostics.snapshot(
                runtime.generation,
                runtime.catalog_loading,
                runtime.catalog_complete,
                runtime.catalog.entries.len()
            ));
            value["preplay"] = json!(runtime.preplay.snapshot(runtime.now_ms()));
            value["command_activity"] = runtime.remote.command_activity(runtime.now_ms());
            value["playback_failure"] = json!(match version {
                playback_failure::Version::V2 => runtime.playback_failure.snapshot(),
                playback_failure::Version::V3 => runtime.playback_failure.snapshot_version(version),
            });
            value["video_evidence"] = json!(video_evidence::snapshot(runtime, app));
            Some(ok(value))
        }
        "capabilities" => {
            if !(fields(p, &[])
                || subset(
                    p,
                    &[
                        "local_media_session",
                        "local_media_progressive",
                        "local_media_handles",
                    ],
                ))
                || [
                    "local_media_session",
                    "local_media_progressive",
                    "local_media_handles",
                ]
                .iter()
                .any(|key| {
                    p.get(*key)
                        .is_some_and(|value| value.as_bool() != Some(true))
                })
            {
                return Some(rejected("Capability probe flags must be true"));
            }
            let mut playback = vec!["previous_channel", "next_channel", "step_channel"];
            if !super::playback_allowed(app) || selected_ids(runtime).is_empty() {
                playback.clear();
            }
            if controls::transport_ready(app) {
                playback.extend(["pause", "resume"]);
                if playback::dur() > 0 {
                    playback.push("seek");
                }
            }
            let mut data = json!({"version":1,"player":{"version":env!("PLX_VERSION"),"build_sha":env!("PLX_BUILD_SHA"),"platform":"webos-native","runtime":runtime.remote.runtime(),"standby":app.ott_standby},
                "input":["up","down","left","right","ok","back","channel_up","channel_down"],"playback":playback,"lifecycle":["exit_app","standby","wake"],
                "vportal_settings":{"version":1},"player_settings":{"version":1,"operations":["schema","get","set"],"max_page_size":20},
                "favorites_global":plx_data::ott::favorites_wire::Protocol::capability(),
                "favorites_global_items":{"version":1}});
            data["native_playback_capture"] = json!({"version":1,"operations":["start","status","read","stop"],
                "default_duration_s":30,"min_duration_s":5,"max_duration_s":60,
                "sample_interval_ms":1000,"max_records":64});
            data["playback_failure_versions"] = json!([2, 3]);
            data["notifications"] = json!({"version":1,"operations":["get","set"]});
            data["favorites_global_recovery"] = favorites_recovery::capability();
            if runtime.usb_remote_capable() {
                data["local_browser"] = json!({"version":1,"operations":["roots","list","more","select"],"max_entries":64});
                data["local_media_handles"] =
                    json!({"version":1,"operations":["connect","play","shuffle"]});
            }
            data["local_media"] = crate::app::ott_local_session::capability(true);
            data["local_media_session"] = crate::app::ott_local_session::extension_capability();
            data["local_media_progressive"] = crate::app::ott_local_session::extension_capability();
            data["native_provider_settings"] = provider_config::capability();
            data["plex_queue"] = crate::app::ott_plex_queue::queue_capability();
            data["plex_queue_state_v2"] = crate::app::ott_plex_queue::queue_state_v2_capability();
            data["plex_queue_repeat"] = crate::app::ott_plex_queue::queue_repeat_capability();
            data["plex_library"] = crate::app::ott_plex_queue::library_capability();
            data["plex_library_truncation"] = json!({"version":1});
            if media_tracks::supports_current(runtime, app, Runtime::track_selection_kiosk) {
                data["media_tracks"] = json!({"version":1});
            }
            if catalog_remote::capability(runtime) {
                data["catalog_random"] = json!({"version":2,"scope":"receipt","request_field":"receipt_random"});
                data["categories"] =
                    json!({"version":1,"operations":["list","select"],"maxPageSize":200});
            }
            if controls::transport_ready(app) && playback::dur() > 0 {
                data["playback_seek_relative"] = json!({"version":1});
            }
            if !super::playback_allowed(app) {
                data["input"] = json!([]);
            } else if controls::transport_ready(app) {
                data["input"].as_array_mut().unwrap().extend([
                    json!("mute"),
                    json!("volume_up"),
                    json!("volume_down"),
                ]);
            }
            if runtime.effective_provider() == ProviderKind::Local {
                data["offline_profiles"] = json!({"version":1,"max_profiles":15,"max_sources":32,"operations":["list","load","save","export","import"]});
            }
            if controls::restartable(runtime, app) {
                data["lifecycle"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!("restart_stream"));
            }
            if let Some(capabilities) = super::companion_adapter::capabilities(runtime).as_object()
            {
                for (name, value) in capabilities {
                    if !value.is_null() {
                        data[name] = value.clone();
                    }
                }
            }
            if super::input_policy::navigation(runtime).is_err() {
                data["input"] = json!([]);
                data["playback"] = json!([]);
            }
            Some(ok(data))
        }
        "providers" => Some(if fields(p, &[]) {
            ok(
                json!({"providers":kinds().iter().enumerate().map(|(i,k)|json!({"id":wire_provider(*k),"index":i,"name":provider_name(*k),"active":runtime.effective_provider()==*k})).collect::<Vec<_>>()}),
            )
        } else {
            rejected("The providers query accepts no parameters")
        }),
        "provider" => {
            if !fields(p, &["query"]) {
                return Some(rejected("Choose a provider by ID or index"));
            }
            let query = p["query"]
                .as_str()
                .map(str::to_lowercase)
                .or_else(|| p["query"].as_u64().map(|n| n.to_string()))
                .unwrap_or_default();
            let matches = provider_matches(&query);
            if matches.len() != 1 {
                return Some(rejected("Choose one provider by ID or index"));
            }
            let kind = matches[0];
            let servers = if kind == ProviderKind::Plex { runtime.native_plex_setup_servers() }
                else { Vec::new() };
            let saved = match provider_candidate(runtime, kind, &servers) {
                Ok(saved) => saved,
                Err(error) => return Some(rejected(error)),
            };

            let reload = saved.profiles.active_profile.is_some();
            defer(
                runtime,
                inv,
                saved,
                reload,
                ok(json!({"dispatched":true,"provider":wire_provider(kind)})),
            )
        }
        "profiles" | "profile" | "profile_settings" | "profile_save" | "profile_export"
        | "profile_import" => profiles(runtime, inv),
        "settings_compare" | "provider_settings" => provider_settings(runtime, inv),
        "player_settings" => settings::command(runtime, app, inv),
        "player_settings_patch" => Some(settings::patch(runtime, inv)),
        "torrent_indexer" | "native_companion" => {
            super::companion_adapter::command(runtime, app, inv)
        }
        "categories" if inv.params["op"] == "select" => catalog_remote::select(runtime, inv),
        "categories" => Some(catalog_remote::categories(runtime, inv)),
        "play_catalog" => catalog_remote::play(runtime, app, inv),
        "channels" if !fields(p, &[]) && !fields(p, &["search"]) => {
            // Shared Unicode implementation is an explicit integration prerequisite.
            Some(catalog_remote::channels(
                runtime,
                inv,
                plx_base::unicode::caseless_key,
            ))
        }
        "channels" => {
            if !fields(p, &[]) && !fields(p, &["search"]) {
                return Some(rejected("Channel queries accept only search"));
            }
            let query = match p.get("search") {
                None => String::new(),
                Some(value) => match text(value, 1024, true) {
                    Some(query) => query.to_lowercase(),
                    None => return Some(rejected("Use a text channel search")),
                },
            };
            Some(ok(
                json!({"channels":controls::channels(runtime).into_iter().filter(|row|row.name.to_lowercase().contains(&query)).collect::<Vec<_>>()}),
            ))
        }
        "playback" => playback_command(runtime, app, inv),
        "restart" => {
            if (!fields(p, &[]) && !fields(p, &["target"]))
                || p.get("target")
                    .is_some_and(|target| !matches!(target.as_str(), Some("stream" | "player")))
            {
                return Some(rejected("Use restart target stream or player"));
            }
            Some(if p["target"] == "player" {
                unsupported("This installation cannot restart its process through an acknowledged lifecycle adapter")
            } else {
                controls::restart(runtime, app, false)
            })
        }
        "play" => {
            if !super::playback_allowed(app) {
                return Some(rejected("Playback is suspended"));
            }
            if !fields(p, &["query"]) {
                return Some(rejected("Choose one channel by name or number"));
            }
            let query = p["query"]
                .as_str()
                .map(str::to_owned)
                .or_else(|| p["query"].as_u64().map(|n| n.to_string()))
                .unwrap_or_default();
            let Some(id) = controls::resolve(&controls::channels(runtime), &query) else {
                return Some(rejected("Choose one unambiguous channel by name or number"));
            };
            match play_reply(runtime, app, &id, inv) {
                Ok(()) => None,
                Err(e) => Some(rejected(e)),
            }
        }
        "input" => {
            if !super::playback_allowed(app) {
                return Some(rejected("Navigation is suspended; wake the player first"));
            }
            if !fields(p, &["key"]) {
                return Some(rejected("Input accepts only a key"));
            }
            if matches!(
                p["key"].as_str(),
                Some("mute" | "volume_up" | "volume_down")
            ) {
                if !controls::transport_ready(app) {
                    return Some(rejected("No active foreground playback"));
                }
                let current = app.player.volume.desired();
                let (level, muted) = match p["key"].as_str() {
                    Some("mute") => (current.level(), !current.muted()),
                    Some("volume_up") => (current.level().saturating_add(5).min(100), false),
                    _ => (current.level().saturating_sub(5), false),
                };
                let effect = AfterReply::Volume {
                    level,
                    muted,
                    last_input: app.last_input,
                };
                runtime.remote.complete_after_reply(inv.clone(), ok(json!({"accepted":true,"dispatched":false,"effect":"input-after-ack","key":p["key"]})), effect);
                return None;
            }
            let Some(key) = p["key"].as_str().and_then(input_key) else {
                return Some(unsupported("Unsupported navigation key"));
            };
            let effect = AfterReply::Input {
                key: key.0,
                sym: key.1,
                wcode: key.2,
                page: app.pages.top_page(),
                surface: app.pages.top_surface_name(),
                last_input: app.last_input,
            };
            runtime.remote.complete_after_reply(inv.clone(),ok(json!({"accepted":true,"dispatched":false,"effect":"input-after-ack","key":p["key"]})),effect);
            None
        }
        "lifecycle" => {
            if !fields(p, &["operation"]) {
                return Some(rejected("Lifecycle accepts only an operation"));
            }
            match p["operation"].as_str() {
                Some("standby" | "wake") => {
                    if !inv.expects_response() {
                        return Some(unsupported("Standby requires an acknowledged request"));
                    }
                    let standby = p["operation"] == "standby";
                    runtime.remote.complete_after_reply(inv.clone(), ok(json!({"accepted":true,"dispatched":false,"effect":"lifecycle-after-ack","operation":p["operation"]})), AfterReply::Standby(standby));
                    None
                }
                Some("exit_app") => {
                    runtime.remote.complete_after_reply(inv.clone(),ok(json!({"accepted":true,"dispatched":false,"effect":"lifecycle-after-ack","operation":"exit_app"})),AfterReply::Exit);
                    None
                }
                Some("restart_stream") => Some(controls::restart(runtime, app, true)),
                _ => Some(unsupported(
                    "This native lifecycle operation is not available",
                )),
            }
        }
        "command" => legacy_command(runtime, app, inv),
        _ => Some(unsupported(
            "This operation is not supported by this native runtime",
        )),
    }
}
fn legacy_command(runtime: &mut Runtime, app: &mut App, inv: &Invocation) -> Option<Value> {
    let p = &inv.params;
    let mut translated = inv.clone();
    match p["command"].as_str() {
        Some("set_volume") => return Some(controls::volume_command(app, inv)),
        Some("channel_by_number" | "channel_by_name") => {
            translated.action = "play".into();
            translated.params = json!({"query":p.get("channel_name").or_else(||p.get("channel_number")).cloned().unwrap_or(Value::Null)});
        }
        Some("change_provider") => {
            translated.action = "provider".into();
            translated.params = json!({"query":p["provider"]});
        }
        Some("change_playlist") => {
            translated.action = "provider_settings".into();
            translated.params = json!({"provider":"m3u","settings":{"playlist":p["playlist"]}});
        }
        Some("exit_player") => {
            if !inv.expects_response() {
                return Some(unsupported("Exit requires an acknowledged request"));
            }
            translated.action = "lifecycle".into();
            translated.params = json!({"operation":"exit_app"});
        }
        _ => return Some(unsupported("Unsupported native legacy command")),
    }
    if let Err(error) = super::input_policy::remote(runtime, &translated) {
        return Some(rejected(error));
    }
    if translated.action == "play"
        && runtime.catalog_loading
        && controls::channels(runtime).is_empty()
    {
        let query = &translated.params["query"];
        if !(query.as_u64().is_some_and(|number| number > 0) || text(query, 1024, false).is_some())
        {
            return Some(rejected("Choose a channel by name or number"));
        }
        if !super::playback_allowed(app) {
            return Some(rejected("Playback is suspended"));
        }
        supersede_deferred(runtime);
        runtime.command_receipts.deferred_channel = Some(DeferredChannel {
            invocation: translated,
            catalog_generation: runtime.generation,
            last_input: app.last_input,
            context: selection_context(runtime),
        });
        return None;
    }
    command(runtime, app, &translated)
}
pub(super) fn track_request_pending(runtime: &Runtime) -> bool {
    media_tracks::has_pending(runtime)
}

pub(super) fn plex_track_request_pending(runtime: &Runtime) -> bool {
    media_tracks::has_pending_plex(runtime)
}

/// Async preparations fence kiosk writes; a cancelled physical worker does not.
pub(super) fn track_preparation_pending(runtime: &Runtime) -> bool {
    plex_track_request_pending(runtime) || super::hls_tracks::has_owned_pending(runtime)
}

pub(super) fn before_playback_tick(runtime: &mut Runtime, app: &mut App, now_ms: Option<u64>) {
    media_tracks::poll_pending(runtime, app, Runtime::track_selection_kiosk, now_ms);
}

pub(super) fn after_tick(runtime: &mut Runtime, app: &mut App) {
    runtime.usb_remote_poll();
    let now = runtime.now_ms();
    media_tracks::poll_pending(runtime, app, Runtime::track_selection_kiosk, Some(now));
    media_tracks::retire_stale(runtime, app, Runtime::track_selection_kiosk);
    for effect in runtime.remote.take_effects() {
        match effect {
            AfterReply::Volume {
                level,
                muted,
                last_input,
            } => {
                if super::input_policy::navigation(runtime).is_ok()
                    && app.last_input == last_input
                    && controls::transport_ready(app)
                {
                    if let Some(desired) =
                        plx_media::player::volume::DesiredVolume::new(i32::from(level), muted)
                    {
                        plx_media::player::volume::set_volume(
                            &mut app.adapters.player,
                            &mut app.player.volume,
                            desired,
                        );
                        app.last_input = clock::now();
                        runtime.changed();
                    }
                }
            }
            AfterReply::Standby(standby) => {
                if standby && !app.ott_standby {
                    runtime.suspend_with_retry_reason(app, continuity_diagnostics::Reason::Standby);
                }
                let changed = crate::app::ott_standby::set(app, standby);
                if !standby && changed && playback_allowed(app) {
                    runtime.resume_source_work();
                }
                runtime.changed();
            }
            AfterReply::Exit => {
                app.running = false;
            }
            AfterReply::Input {
                key,
                sym,
                wcode,
                page,
                surface,
                last_input,
            } => {
                if super::input_policy::navigation(runtime).is_err()
                    || app.pages.top_page() != page
                    || app.pages.top_surface_name() != surface
                    || app.last_input != last_input
                    || !super::playback_allowed(app)
                {
                    continue;
                }
                let now = clock::now();
                app.last_input = now;
                for edge in [Edge::Down, Edge::Up] {
                    app.inputs.push(InputEvent {
                        at: Tick { ms: now, dt_us: 0 },
                        source: Source::Script,
                        kind: InputKind::Key {
                            key,
                            sym,
                            wcode,
                            edge,
                            at_edge: false,
                        },
                    });
                }
                plx_machine::idle::invalidate();
            }
        }
    }
    drain_deferred(runtime, app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_plex_candidate_uses_authorized_metadata_without_copying_credentials() {
        use super::super::ott_plex_setup::PlexSetupServer;
        let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
        let servers = [PlexSetupServer { machine_id: "native-nas".into(), name: "Native NAS".into(), id_hint: 7 }];
        let candidate = provider_candidate(&runtime, ProviderKind::Plex, &servers).unwrap();
        let selected = candidate.profiles.profiles.iter().find(|p| Some(&p.id) == candidate.profiles.active_profile.as_ref()).unwrap();
        assert_eq!(selected.kind, ProviderKind::Plex);
        assert_eq!(selected.source, "native-nas");
        assert_eq!(candidate.settings["selectedProvider"], "plex");
        assert_eq!(candidate.plex_sources().unwrap().len(), 0, "metadata cannot copy account credentials");
        assert!(runtime.saved.profiles.profiles.is_empty(), "planning does not write or change runtime");
        runtime.saved = candidate;
        let original_id = runtime.saved.profiles.active_profile.clone();
        let current = runtime.saved.profiles.profiles.iter_mut().find(|p| p.kind == ProviderKind::Plex).unwrap();
        current.enabled = false;
        runtime.saved.profiles.active_profile = None;
        let again = provider_candidate(&runtime, ProviderKind::Plex, &servers).unwrap();
        assert_eq!(again.profiles.profiles.len(), 1);
        assert_eq!(again.profiles.active_profile, original_id);
        assert!(again.profiles.profiles[0].enabled);
        runtime.saved = again;
        let mut stale = runtime.saved.profiles.profiles[0].clone();
        stale.id = "stale-first".into(); stale.source = "retired-machine".into();
        runtime.saved.profiles.profiles.insert(0, stale);
        let current = provider_candidate(&runtime, ProviderKind::Plex, &servers).unwrap();
        assert_eq!(current.profiles.active_profile, original_id, "a stale first row cannot capture remote selection");
        let absent = provider_candidate(&Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test()), ProviderKind::Plex, &[]).unwrap();
        assert!(absent.profiles.profiles.is_empty());
        assert!(absent.profiles.active_profile.is_none());
    }


    #[test]
    fn provider_ids_and_indices_outrank_partial_labels() {
        assert_eq!(provider_matches("stalker"), vec![ProviderKind::Stalker]);
        for (index, kind) in kinds().into_iter().enumerate() {
            assert_eq!(provider_matches(kind.canonical_id()), vec![kind]);
            assert_eq!(provider_matches(&index.to_string()), vec![kind]);
        }
        assert!(provider_matches("").is_empty());
        assert!(provider_matches("missing-provider").is_empty());
    }

    #[test]
    fn binding_imported_folder_selects_exact_local_slot_without_changing_other_profiles() {
        let mut runtime =
            Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
        let tv_index = insert_profile(
            &mut runtime.saved,
            ProviderKind::M3u,
            1,
            "https://fixture.invalid/list.m3u".into(),
            Some("TV".into()),
        )
        .unwrap();
        let tv_id = runtime.saved.profiles.profiles[tv_index].id.clone();
        insert_profile(
            &mut runtime.saved,
            ProviderKind::Local,
            1,
            "/receiver/Other".into(),
            Some("Other folder".into()),
        )
        .unwrap();
        runtime.saved.profiles.active_profile = Some(tv_id.clone());
        runtime.saved.category = "Previously selected category".into();
        runtime.saved.favorites_selected = true;
        let document = json!({
            "version": 1,
            "provider": "offline",
            "name": "Imported movies",
            "binding_id": "01234567-89ab-4cde-8f01-23456789abcd",
            "sources": [{"key": "s0", "kind": "folder", "name": "Movies"}],
            "selected": null
        });
        runtime
            .saved
            .settings
            .insert("offlineProfile:15".into(), document.clone());
        let before = serde_json::to_value(&runtime.saved).unwrap();
        let previous_profiles = runtime.saved.profiles.profiles.clone();

        let candidate = bind_local(&runtime, 15, "/receiver/Movies").unwrap();
        let index = profile_index(&candidate, ProviderKind::Local, 15).unwrap();
        let bound = &candidate.profiles.profiles[index];
        assert_eq!(
            candidate.profiles.active_profile.as_deref(),
            Some(bound.id.as_str())
        );
        assert_ne!(bound.id, tv_id);
        assert_eq!(bound.kind, ProviderKind::Local);
        assert_eq!(bound.source, "/receiver/Movies");
        assert_eq!(bound.source_alias, "Movies");
        assert_eq!(
            &candidate.profiles.profiles[..previous_profiles.len()],
            &previous_profiles
        );
        assert_eq!(candidate.settings["offlineProfile:15"], document);
        assert_eq!(candidate.category, runtime.saved.category);
        assert_eq!(
            candidate.favorites_selected,
            runtime.saved.favorites_selected
        );
        assert_eq!(candidate.favorites, runtime.saved.favorites);
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    }

    #[test]
    fn failed_ui_folder_binding_does_not_change_active_profile_or_saved_state() {
        let mut runtime =
            Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
        let index = insert_profile(
            &mut runtime.saved,
            ProviderKind::M3u,
            1,
            "https://fixture.invalid/list.m3u".into(),
            Some("TV".into()),
        )
        .unwrap();
        runtime.saved.profiles.active_profile =
            Some(runtime.saved.profiles.profiles[index].id.clone());
        let before = serde_json::to_value(&runtime.saved).unwrap();
        assert_eq!(
            bind_local(&runtime, 15, "/receiver/Movies").err(),
            Some("Import this local profile first")
        );
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    }

    #[test]
    fn explicit_exit_retires_first_batch_channel_and_stale_play_receipt() {
        let mut runtime =
            Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
        runtime.remote = Remote::disabled_for_test("dev_test", "runtime");
        let invocation = runtime.remote.request_for_test("play", json!({"query":1}));
        runtime.catalog_loading = true;
        runtime.command_receipts.deferred_channel = Some(DeferredChannel {
            invocation: invocation.clone(),
            catalog_generation: runtime.generation,
            last_input: 0,
            context: selection_context(&runtime),
        });
        runtime.command_receipts.play_response = Some((
            invocation.id.clone(),
            ok(json!({
                "dispatched":true,"channel":{"id":"old-channel","name":"Old channel","number":1}
            })),
        ));
        let settings_receipt = runtime.command_receipts.record(
            "settings",
            "signature".into(),
            "context".into(),
            1,
            100,
        );

        cancel_pending_play(&mut runtime);
        // A first batch would now make the old request executable. Explicit exit
        // has removed that intent rather than waiting for its original deadline.
        assert_eq!(
            deferred_ready(invocation.deadline_ms, 100, true, true, true, false),
            Ok(true)
        );
        assert!(runtime.command_receipts.deferred_channel.is_none());
        assert!(runtime.command_receipts.play_response.is_none());
        assert_eq!(
            play_response(&runtime, &invocation),
            ok(json!({"dispatched":true}))
        );
        assert!(runtime.command_receipts.consume(
            &settings_receipt,
            "signature",
            "context",
            1,
            101
        ));

        cancel_pending_play(&mut runtime);
        assert!(runtime.command_receipts.deferred_channel.is_none());
        assert!(runtime.command_receipts.play_response.is_none());
    }

    #[test]
    fn deferred_channels_wait_for_data_but_never_outlive_their_source_or_deadline() {
        assert_eq!(deferred_ready(200, 100, true, true, true, true), Ok(false));
        assert_eq!(deferred_ready(200, 100, true, true, true, false), Ok(true));
        assert_eq!(deferred_ready(200, 100, true, true, false, true), Ok(true));
        assert!(deferred_ready(200, 200, true, true, true, true).is_err());
        assert!(deferred_ready(200, 100, false, true, true, false).is_err());
        assert!(deferred_ready(200, 100, true, false, false, false).is_err());
    }
    #[test]
    fn importing_provider_preserves_other_sources_and_favorites() {
        let mut saved = Saved::default();
        insert_profile(
            &mut saved,
            ProviderKind::M3u,
            1,
            "https://tv.example/list.m3u".into(),
            Some("Home".into()),
        )
        .unwrap();
        let original = saved.profiles.profiles[0].clone();
        insert_profile(
            &mut saved,
            ProviderKind::VPortal,
            1,
            "portal::[key:fixture]https://media.example/api/".into(),
            Some("Films".into()),
        )
        .unwrap();
        assert_eq!(saved.profiles.profiles[0], original);
        assert_eq!(saved.profiles.profiles.len(), 2);
        assert!(!metadata(&saved, ProviderKind::VPortal, 1)
            .to_string()
            .contains("fixture"));
    }
    fn ui_slot_profile(kind: ProviderKind, suffix: usize) -> ProviderProfile {
        ProviderProfile {
            id: format!("{}-{suffix}", kind.canonical_id()),
            name: format!("UI fixture {suffix}"),
            kind,
            source: match kind {
                ProviderKind::M3u => "https://fixture.invalid/list.m3u".into(),
                ProviderKind::Stalker => json!({
                    "portal": "https://fixture.invalid/c/", "mac": "02:00:00:00:00:01"
                })
                .to_string(),
                _ => unreachable!("bounded M3U/Stalker slot fixture"),
            },
            source_alias: String::new(),
            enabled: true,
        }
    }

    #[test]
    fn ui_add_after_remote_sparse_slot_keeps_every_profile_addressable() {
        for kind in [ProviderKind::M3u, ProviderKind::Stalker] {
            let mut runtime =
                Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
            let source = ui_slot_profile(kind, 3).source;
            insert_profile(
                &mut runtime.saved,
                kind,
                3,
                source,
                Some("Remote fixture".into()),
            )
            .unwrap();
            let remote_profile = runtime.saved.profiles.profiles[0].clone();
            for suffix in [1, 2] {
                let profile = ui_slot_profile(kind, suffix);
                let before = serde_json::to_value(&runtime.saved).unwrap();
                // Actual UI candidate seam; assignment models publication only.
                // No writer, protected store, provider worker or decoder is started.
                let candidate = runtime.profile_candidate(profile.clone(), None).unwrap();
                assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
                assert_eq!(
                    candidate.profiles.active_profile.as_deref(),
                    Some(profile.id.as_str())
                );
                runtime.saved = candidate;
            }
            assert_eq!(runtime.saved.profiles.profiles[0], remote_profile);
            assert_eq!(
                runtime.saved.settings[&format!("slot:{}", remote_profile.id)],
                json!(3)
            );
            assert_eq!(profile_index(&runtime.saved, kind, 3), Some(0));
            let addressed: Vec<_> = (1..=15)
                .filter_map(|number| {
                    profile_index(&runtime.saved, kind, number).map(|index| (number, index))
                })
                .collect();
            assert_eq!(
                addressed.len(),
                3,
                "each saved profile must have a numbered address"
            );
            for (index, profile) in runtime.saved.profiles.profiles.iter().enumerate() {
                let slots: Vec<_> = addressed
                    .iter()
                    .filter(|(_, found)| *found == index)
                    .collect();
                assert_eq!(slots.len(), 1, "a profile must be represented exactly once");
                let row = metadata(&runtime.saved, kind, slots[0].0);
                assert_eq!(row["name"], json!(profile.name));
                assert_eq!(
                    row["active"],
                    json!(Some(&profile.id) == runtime.saved.profiles.active_profile.as_ref())
                );
            }
        }
    }

    #[test]
    fn ui_add_preserves_existing_legacy_and_explicit_slot_assignments() {
        for kind in [ProviderKind::M3u, ProviderKind::Stalker] {
            let mut runtime =
                Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
            let legacy = ui_slot_profile(kind, 1);
            runtime.saved.profiles.profiles.push(legacy.clone());
            let mut sparse = ui_slot_profile(kind, 3);
            sparse.name = "Existing sparse fixture".into();
            runtime
                .saved
                .settings
                .insert(format!("slot:{}", sparse.id), json!(3));
            runtime.saved.profiles.profiles.push(sparse.clone());
            let other_kind = if kind == ProviderKind::M3u {
                ProviderKind::Stalker
            } else {
                ProviderKind::M3u
            };
            let other = ui_slot_profile(other_kind, 1);
            runtime.saved.profiles.profiles.push(other.clone());
            let added = ui_slot_profile(kind, 2);
            runtime.saved = runtime.profile_candidate(added.clone(), None).unwrap();
            assert_eq!(profile_index(&runtime.saved, kind, 1), Some(0));
            assert_eq!(profile_index(&runtime.saved, kind, 3), Some(1));
            assert_eq!(profile_index(&runtime.saved, other_kind, 1), Some(2));
            let new_number = (1..=15)
                .find(|number| profile_index(&runtime.saved, kind, *number) == Some(3))
                .expect("new UI profile must remain independently addressable");
            assert_ne!(new_number, 1);
            assert_ne!(new_number, 3);
            assert_eq!(
                &runtime.saved.profiles.profiles[..3],
                &[legacy, sparse, other]
            );
            let before_edit = runtime.saved.settings.clone();
            let mut edited = added.clone();
            edited.name = "Renamed UI fixture".into();
            runtime.saved = runtime
                .profile_candidate(edited.clone(), Some(&added))
                .unwrap();
            assert_eq!(
                runtime.saved.settings, before_edit,
                "an edit must not reallocate numbered slots"
            );
            assert_eq!(profile_index(&runtime.saved, kind, new_number), Some(3));
            assert_eq!(
                metadata(&runtime.saved, kind, new_number)["name"],
                json!(edited.name)
            );
            let before_invalid = serde_json::to_value(&runtime.saved).unwrap();
            let mut invalid = ui_slot_profile(kind, 4);
            invalid.source = "invalid provider source".into();
            assert!(runtime.profile_candidate(invalid, None).is_err());
            assert_eq!(
                serde_json::to_value(&runtime.saved).unwrap(),
                before_invalid
            );
        }
    }

    #[test]
    fn invalid_source_is_atomic_and_sparse_slots_keep_their_numbers() {
        let mut saved = Saved::default();
        assert!(insert_profile(
            &mut saved,
            ProviderKind::M3u,
            15,
            "https://tv.example/list".into(),
            None
        )
        .is_ok());
        assert_eq!(profile_index(&saved, ProviderKind::M3u, 15), Some(0));
        assert_eq!(profile_index(&saved, ProviderKind::M3u, 1), None);
        assert!(insert_profile(
            &mut saved,
            ProviderKind::M3u,
            1,
            "file:///private/secret".into(),
            None
        )
        .is_err());
        assert_eq!(saved.profiles.profiles.len(), 1);
        assert!(saved.validate().is_ok());
    }
    #[test]
    fn profile_revision_changes_with_private_source_without_exposing_it() {
        let mut saved = Saved::default();
        insert_profile(
            &mut saved,
            ProviderKind::M3u,
            1,
            "https://tv.example/one".into(),
            None,
        )
        .unwrap();
        let before = revision(&saved, ProviderKind::M3u, 1);
        insert_profile(
            &mut saved,
            ProviderKind::M3u,
            1,
            "https://tv.example/two".into(),
            None,
        )
        .unwrap();
        assert_ne!(before, revision(&saved, ProviderKind::M3u, 1));
        assert_eq!(before.len(), 64);
    }
    #[test]
    fn compare_receipts_are_single_use_expire_and_match_the_source() {
        let mut receipts = Receipts::default();
        let first = receipts.record("first", "state-a".into(), "source-a".into(), 15, 100);
        assert!(!receipts.consume(&first, "state-b", "source-a", 15, 101));
        assert!(!receipts.consume(&first, "state-a", "source-a", 15, 102));
        let second = receipts.record("second", "state-a".into(), "source-a".into(), 15, 100);
        assert!(!receipts.consume(&second, "state-a", "source-a", 15, 120100));
        let third = receipts.record("third", "state-a".into(), "source-a".into(), 15, 100);
        assert!(receipts.consume(&third, "state-a", "source-a", 15, 101));
        assert!(!receipts.consume(&third, "state-a", "source-a", 15, 102));
    }
}

/// Internal recovery uses the actual existing stream actuator and its receipt.
/// No remote invocation is synthesized and no accepted dispatch is guessed.
pub(super) fn restart_for_recovery(runtime: &mut Runtime, app: &mut App) -> bool {
    controls::restart(runtime, app, true)["data"]["dispatched"].as_bool() == Some(true)
}

pub(super) fn category_select_rejection(
    runtime: &Runtime,
    inv: &Invocation,
    code: &'static str,
) -> Option<Value> {
    catalog_remote::select_rejection(runtime, inv, code)
}
pub(super) fn category_select_completion_allowed(runtime: &Runtime, inv: &Invocation) -> bool {
    catalog_remote::select_completion_allowed(runtime, inv)
}
pub(super) fn category_select_settled(
    runtime: &Runtime,
    inv: &Invocation,
    saved: bool,
    applied: bool,
    response: Value,
) -> Value {
    catalog_remote::select_settled(runtime, inv, saved, applied, response)
}

#[cfg(all(test, feature = "hostsim"))]
#[path = "ott_commands_name_tests.rs"]
mod name_tests;
