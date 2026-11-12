//! One source-bound number entry owner for bare live video and OTT catalogues.
//! No timer thread, catalogue mutation, decoder call or screen-local number map.
use super::*;
use crate::app::events;
use std::collections::BTreeSet;

const DIGIT_WINDOW_MS: u64 = 2_000;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Owner {
    instance: InstanceId,
    navigation: u64,
    generation: u64,
    catalog: u64,
    playback: u64,
    page: &'static str,
    favorites: bool,
    list_ordinals: bool,
}
struct Pending {
    owner: Owner,
    context: String,
    rows: commands::NumberedChannels,
    digits: String,
    until: u64,
}
#[derive(Default)]
pub(super) struct State {
    pending: Option<Pending>,
    held: BTreeSet<u32>,
}

fn key(bytes: &[u8]) -> Option<(u32, u32, bool)> {
    if bytes.len() < 28 || !matches!(events::rd_u32(bytes, 0), 0x300 | 0x301) { return None; }
    let (state, _, sym) = events::decode_key(bytes);
    let plain = if cfg!(feature = "hostsim") && events::rd_u32(bytes, 8) != events::SYNTH_WINDOW {
        u16::from_ne_bytes([bytes[24], bytes[25]]) & (0x0003 | 0x00c0 | 0x0300 | 0x0c00) == 0
    } else { true };
    Some((state, sym, plain))
}
fn digit(sym: u32) -> bool { (b'0' as u32..=b'9' as u32).contains(&sym) }

fn ready(runtime: &Runtime, app: &App) -> bool {
    app.running && playback_allowed(app) && runtime.loaded && !runtime.persistence_uncertain
        && runtime.numeric_write_ready() && runtime.edit_session.is_none() && runtime.setup.is_none()
        && runtime.clock_sample.is_some() && runtime.play_work_allowed()
        && input_policy::navigation(runtime).is_ok() && !app.pages.has_pending_navigation()
        && app.pages.input.arm.is_none() && !app.pages.input.press.is_active()
}

fn owner(runtime: &Runtime, app: &App, rendered: bool) -> Option<Owner> {
    if !ready(runtime, app) { return None; }
    let torrent = torrent_page(runtime.page);
    let favorites = !torrent && runtime.view_selection().1;
    let (instance, list_ordinals) = if let Some(instance) = bridge::ott_ui_target(&app.pages) {
        let screen = app.pages.nav.instance(instance)?.screen.as_any()?
            .downcast_ref::<plx_screens::ott::OttScreen>()?;
        if !screen.accepts_channel_number() { return None; }
        if rendered && !screen.channel_number_context(
            &runtime.search_page_id().unwrap_or_else(|| runtime.page.into()), runtime.revision) {
            return None;
        }
        (instance, screen.channel_number_uses_list_order())
    } else {
        // A companion session is numbered only in its rendered list, never
        // through an unrelated retained Runtime::playing entry.
        if torrent { return None; }
        if !matches!(app.route(), AppArg::Player) || app.pages.surface_up() { return None; }
        let screen = bridge::player(&app.pages)?;
        if !screen.accepts_channel_number() {
            return None;
        }
        let recovery = failed_playback::ready(runtime, app);
        let playing = runtime.playing.as_ref().or_else(||
            recovery.then(|| failed_playback::admissible(runtime, app)).flatten())?;
        // Archive playback retains its live channel metadata but owns VOD transport keys.
        if !favorites && (playing.archive.is_some() || !guide_ui::is_live(&playing.profile, &playing.entry)) { return None; }
        if !recovery && !playing.identity.as_ref().is_some_and(|identity|
            plx_media::route::playback_identity_matches(&app.player.session, identity)) { return None; }
        (app.pages.top_page()?, false)
    };
    if !torrent && !matches!((runtime.page, favorites), ("channels", false) | ("favorites", true)) { return None; }
    Some(Owner { instance, navigation: runtime.navigation_epoch, generation: runtime.generation,
        catalog: runtime.catalog_revision, playback: runtime.playback_epoch, page: runtime.page, favorites, list_ordinals })
}

fn numbered_rows(runtime: &Runtime, owner: &Owner) -> commands::NumberedChannels {
    if torrent_page(runtime.page) {
        // Torrent ordinals are assigned before disabled rows are removed;
        // applying with_list_ordinals again would silently close those gaps.
        return companion_adapter::view(runtime)
            .map_or_else(|| commands::NumberedChannels::from_numbered(Vec::new()), |view| torrent_numbers(&view));
    }
    let rows = commands::numbered_channels(runtime);
    if owner.list_ordinals { rows.with_list_ordinals() } else { rows }
}

fn torrent_page(page: &str) -> bool {
    matches!(page, "torrent-files" | "torrent-results")
}

/// Numeric input uses the same actual content rows as Classic, not the unrelated
/// saved provider catalogue. Sparse backend indices remain in the action IDs.
pub(super) fn torrent_numbers(view: &OttView) -> commands::NumberedChannels {
    commands::NumberedChannels::from_numbered(view.rows.iter()
        .filter(|row| plx_screens::ott::torrent_catalogue_row(&view.page_id, row))
        .enumerate()
        .filter(|(_, row)| row.enabled)
        .filter_map(|(index, row)| match &row.action {
            OttAction::Play(id) => Some((index + 1, id.clone(),
                row.content_title.clone().unwrap_or_else(|| row.title.clone()))),
            _ => None,
        }))
}

pub(super) fn torrent_context(view: &OttView, settings: &BTreeMap<String, Value>) -> Option<String> {
    if !torrent_page(&view.page_id) { return None; }
    let rows: Vec<_> = view.rows.iter().map(|row|
        (&row.id, &row.title, &row.content_title, &row.description, row.enabled, format!("{:?}", row.action))).collect();
    let value = json!([view.page_id, rows,
        settings.get("nativeCompanion"), settings.get("nativeCompanionRevision")]);
    Some(plx_data::ott::favorites_wire::hash(&serde_json::to_vec(&value).ok()?))
}

// Bare live video and mixed Favorites keep their canonical number owner. Only the actual
// finite/archive player can use 1/3; the same pre-press HUD and policy gates apply.
fn seek13(runtime: &mut Runtime, app: &mut App, sym: u32) -> bool {
    let direction = match sym {
        value if value == u32::from(b'1') => -1,
        value if value == u32::from(b'3') => 1,
        _ => return false,
    };
    if !ready(runtime, app) || runtime.view_selection().1
        || !matches!(app.route(), AppArg::Player) || app.pages.surface_up()
        || !commands::playback_ready(app)
    { return false; }
    let Some(playing) = runtime.playing.as_ref() else { return false; };
    if (playing.archive.is_none() && guide_ui::is_live(&playing.profile, &playing.entry))
        || !playing.identity.as_ref().is_some_and(|identity|
            plx_media::route::playback_identity_matches(&app.player.session, identity))
    { return false; }
    let now = clock::now();
    let Some(page) = bridge::player_mut(&mut app.pages) else { return false; };
    if !page.accepts_numeric_seek(&app.player.session, now, lifecycle::viewer_paused()) {
        return false;
    }
    let step = i64::from(runtime.classic_preferences.seek13_seconds()) * 1_000_000_000 * direction;
    let Some(target) = page.jump_by(&app.player.session, step, false, now) else { return false; };
    cancel(runtime, app);
    runtime.numeric_input.held.insert(sym);
    app.down_sym = sym;
    app.last_input = now;
    app.rec.refuse("Numeric seek input is not bound in this recording format");
    playback::commit_seek(target);
    runtime.viewer_seek(app);
    runtime.changed();
    true
}

// Categories select immediately, using the action of the currently rendered row.
// The raw ingress below owns modifiers/repeats/releases; no new key protocol is used.
fn category_number(runtime: &mut Runtime, app: &mut App, sym: u32) -> bool {
    if !(u32::from(b'1')..=u32::from(b'9')).contains(&sym)
        || !ready(runtime, app) || runtime.page != "categories"
        || runtime.interface_mode() != plx_machine::machine::UiMode::Classic
    { return false; }
    let Some(instance) = bridge::ott_ui_target(&app.pages) else { return false; };
    if runtime.reply != Some(instance) { return false; }
    let Some(screen) = app.pages.nav.instance(instance)
        .and_then(|instance| instance.screen.as_any())
        .and_then(|screen| screen.downcast_ref::<plx_screens::ott::OttScreen>())
    else { return false; };
    if !screen.channel_number_context("categories", runtime.revision) { return false; }
    let Some(action) = screen.category_number_action((sym - u32::from(b'0')) as u8)
        else { return false; };
    cancel(runtime, app);
    runtime.numeric_input.held.insert(sym);
    app.down_sym = sym;
    app.last_input = clock::now();
    app.rec.refuse("Numeric category input is not bound in this recording format");
    apply_ui_action(runtime, app, action, true, false, false);
    true
}

// Called at the first/next digit and commit, never on every idle frame. The
// potentially large canonical row projection is separately cached once.
fn context(runtime: &Runtime) -> Option<String> {
    if torrent_page(runtime.page) {
        return torrent_context(&companion_adapter::view(runtime)?, &runtime.saved.settings);
    }
    let favorites = runtime.view_selection().1;
    let query = runtime.search_view()?.query;
    let value = json!([runtime.effective_profile(), runtime.view_selection(), query,
        runtime.transient.accepted.as_ref().map(transient::TransientMedia::owner).map(|o| o.0),
        runtime.saved.settings.get(catalog_organization::KEY),
        favorites.then_some(&runtime.saved.favorites), favorites.then_some(&runtime.saved.profiles)]);
    Some(plx_data::ott::favorites_wire::hash(&serde_json::to_vec(&value).ok()?))
}
fn notice(app: &mut App, instance: InstanceId, value: Option<(String, String)>) {
    // Presentation only, applied in input order just like volume/reminder
    // feedback. A delayed screen message must not erase a later volume notice.
    let now = clock::now();
    let Some(body) = app.pages.nav.instance_mut(instance) else { return; };
    let Some(screen) = body.screen.as_any_mut() else { return; };
    if let Some(player) = screen.downcast_mut::<plx_screens::player::PlayerScreen>() {
        player.set_number_notice(&value, now);
    } else if let Some(catalog) = screen.downcast_mut::<plx_screens::ott::OttScreen>() {
        catalog.set_number_notice(&value, now);
    }
}
fn cancel(runtime: &mut Runtime, app: &mut App) {
    if let Some(pending) = runtime.numeric_input.pending.take() { notice(app, pending.owner.instance, None); }
}

/// Retire our release even when outer lifecycle/kiosk gates consume that edge.
/// Other input is cancellation only: it always keeps its existing handler.
pub(super) fn boundary(app: &mut App) -> bool {
    let event = events::rd_u32(&app.ev, 0);
    let raw = key(&app.ev);
    let Some(mut runtime) = app.ott.take() else { return false; };
    let mut consumed = false;
    if let Some((state, sym, plain)) = raw {
        if state & 0xff != 1 {
            consumed = runtime.numeric_input.held.remove(&sym);
            if consumed && app.down_sym == sym { app.down_sym = 0; }
        } else if state & 0x100 == 0 {
            runtime.numeric_input.held.remove(&sym);
            if !plain || !digit(sym) { cancel(&mut runtime, app); }
        }
    } else if matches!(event, 0x401 | 0x402 | 0x403 | 0x200 | 0x100 | 0x106) {
        cancel(&mut runtime, app);
    }
    app.ott = Some(runtime);
    consumed
}

pub(super) fn ingest(app: &mut App) -> bool {
    let Some((state, sym, plain)) = key(&app.ev) else { return false; };
    if !plain || !digit(sym) { return false; }
    let Some(mut runtime) = app.ott.take() else { return false; };
    let consumed = (|| {
        if state & 0xff != 1 { return runtime.numeric_input.held.remove(&sym); }
        if state & 0x100 != 0 { return runtime.numeric_input.held.contains(&sym); }
        if seek13(&mut runtime, app, sym) { return true; }
        if category_number(&mut runtime, app, sym) { return true; }
        let Some(current) = owner(&runtime, app, true) else { cancel(&mut runtime, app); return false; };
        let Some(fingerprint) = context(&runtime) else { cancel(&mut runtime, app); return false; };
        let now = runtime.now_ms();
        if runtime.numeric_input.pending.as_ref().is_some_and(|p|
            p.owner != current || p.context != fingerprint || now >= p.until) { cancel(&mut runtime, app); }
        if runtime.numeric_input.pending.is_none() {
            if sym == b'0' as u32 { return false; }
            let rows = numbered_rows(&runtime, &current);
            runtime.numeric_input.pending = Some(Pending { owner: current,
                context: fingerprint, rows,
                digits: String::new(), until: now });
        }
        runtime.numeric_input.held.insert(sym);
        app.down_sym = sym;
        app.last_input = clock::now();
        app.rec.refuse("Numeric channel input is not bound in this recording format");
        let pending = runtime.numeric_input.pending.as_mut().unwrap();
        if pending.digits.len() == 4 { return true; }
        pending.digits.push(char::from_u32(sym).unwrap());
        pending.until = now.saturating_add(DIGIT_WINDOW_MS);
        let title = pending.rows.resolve(&pending.digits).map(|(_, title)| title).unwrap_or_default();
        notice(app, pending.owner.instance, Some((pending.digits.clone(), title)));
        true
    })();
    app.ott = Some(runtime);
    consumed
}

/// Called after ordinary UI/remote commands. A concurrent source change wins.
pub(super) fn tick(runtime: &mut Runtime, app: &mut App, now: u64) {
    let Some(pending) = runtime.numeric_input.pending.as_ref() else { return; };
    if owner(runtime, app, false).as_ref() != Some(&pending.owner) { cancel(runtime, app); return; }
    if now < pending.until { return; }
    if owner(runtime, app, true).as_ref() != Some(&pending.owner) { cancel(runtime, app); return; }
    let pending = runtime.numeric_input.pending.take().unwrap();
    notice(app, pending.owner.instance, None);
    if context(runtime).as_deref() != Some(pending.context.as_str()) { return; }
    let Some((cached_id, _)) = pending.rows.resolve(&pending.digits) else { return; };
    let Some((id, _)) = numbered_rows(runtime, &pending.owner).resolve(&pending.digits) else { return; };
    if id != cached_id { return; }
    apply_ui_action(runtime, app, OttAction::Play(id), true, true, false);
}
