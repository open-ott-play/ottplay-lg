//! Account-scoped presentation hints for the temporary native Plex catalogue.
//! These never select a provider, authorize a source or start playback. The
//! current ClassicRead must admit the source again before a hint is considered.
use super::*;
use plx_platform::storage::wire::ott::ClassicBookmark;

#[path = "ott_classic_bookmark_identity.rs"]
mod identity;

const SETTLE_MS: u64 = 500;

struct View {
    profile: ProviderProfile,
    source: String,
    value: Option<ClassicBookmark>,
    loaded: bool,
    category_applied: bool,
    selection: Option<String>,
    last_catalog: Option<(u64, u64)>,
    dirty: bool,
    due: Option<u64>,
}
#[derive(Default)]
pub(super) struct State {
    view: Option<View>,
    // Only the existing Runtime writer can own this receipt. A retired view
    // cannot publish a late acknowledgement into its replacement.
    writing: Option<Write>,
}

pub(super) struct Write {
    pub(super) value: ClassicBookmark,
    pub(super) category_choice: bool,
}

pub(super) fn bind(runtime: &mut Runtime, app: &App) {
    runtime.classic_bookmark.view = None;
    let Some(profile) = classic_source::binding_profile(runtime).cloned() else { return; };
    if classic_source::guard_profile(runtime, app, &profile).is_err() { return; }
    let Some(source) = identity::capture(app, &profile.source) else { return; };
    // Identity capture reads the session owner, then rechecks its media grant.
    if classic_source::guard_profile(runtime, app, &profile).is_err() { return; }
    runtime.classic_bookmark.view = Some(View {
        profile, source, value: None, loaded: false, category_applied: false,
        selection: None, last_catalog: None,
        dirty: false, due: None,
    });
}
pub(super) fn release(runtime: &mut Runtime) { runtime.classic_bookmark.view = None; }

pub(super) fn current(runtime: &Runtime, app: &App) -> bool {
    runtime.classic_bookmark.view.as_ref().is_some_and(|view|
        classic_source::binding_profile(runtime) == Some(&view.profile)
            && classic_source::guard_profile(runtime, app, &view.profile).is_ok())
}

fn category_key(catalog: &Catalog, selection: &str) -> Option<Option<String>> {
    if selection.is_empty() { return Some(None); }
    let id = catalog.resolve_category(selection)?;
    let node = catalog.hierarchy.as_ref()?.nodes.iter().find(|node| node.id == id)?;
    let identity = node.source_identity.as_ref()?;
    identity.validate().ok()?;
    Some(Some(identity.fingerprint()))
}
fn category_id(catalog: &Catalog, digest: Option<&str>) -> Option<String> {
    let Some(digest) = digest else { return Some(String::new()); };
    let mut nodes = catalog.hierarchy.as_ref()?.nodes.iter().filter(|node|
        node.source_identity.as_ref().is_some_and(|identity|
            identity.validate().is_ok() && identity.fingerprint() == digest));
    let node = nodes.next()?;
    nodes.next().is_none().then(|| node.id.clone())
}

fn update(runtime: &mut Runtime, category: Option<String>, focus: Option<String>) {
    let due = runtime.now_ms().saturating_add(SETTLE_MS);
    let selection = runtime.effective_category().to_owned();
    let Some(view) = runtime.classic_bookmark.view.as_mut() else { return; };
    let value = ClassicBookmark { source: view.source.clone(), category, focus };
    if value.validate().is_err() { return; }
    view.loaded = true; // A user action always wins over a delayed load.
    view.category_applied = true;
    view.selection = Some(selection);
    if view.value.as_ref() != Some(&value) {
        view.value = Some(value);
        view.dirty = true;
        view.due = Some(due);
    }
}

/// A category-only edit of this temporary source has no protected state change.
/// Leaving global Favorites still uses its existing protected transaction.
pub(super) fn category_patch(runtime: &Runtime, category: &str)
    -> Result<Option<ClassicBookmark>, &'static str>
{
    let Some(view) = runtime.classic_bookmark.view.as_ref() else { return Ok(None); };
    if runtime.saved.favorites_selected { return Ok(None); }
    if classic_source::binding_profile(runtime) != Some(&view.profile) {
        return Err("Plex source is no longer available");
    }
    let key = category_key(&runtime.catalog, category).ok_or("The category list changed")?;
    let focus = view.value.as_ref().filter(|old| old.category == key).and_then(|old| old.focus.clone());
    let value = ClassicBookmark { source: view.source.clone(), category: key, focus };
    value.validate().map_err(|_| "Invalid Classic bookmark")?;
    Ok(Some(value))
}
pub(super) fn category_started(runtime: &mut Runtime, value: ClassicBookmark) {
    runtime.classic_bookmark.writing = Some(Write { value, category_choice: true });
}

pub(super) fn record_category(runtime: &mut Runtime, category: &str) {
    if runtime.classic_bookmark.view.is_none() { return; }
    let Some(key) = category_key(&runtime.catalog, category) else { return; };
    let focus = runtime.classic_bookmark.view.as_ref().and_then(|view| view.value.as_ref())
        .filter(|old| old.category == key).and_then(|old| old.focus.clone());
    update(runtime, key, focus);
    request_flush(runtime);
}

/// Called only after the real UI row action has been admitted. Obtain the
/// ratingKey from its typed locator, never by splitting or reusing a row ID.
pub(super) fn record_focus(runtime: &mut Runtime, id: &str) {
    let Some(view) = runtime.classic_bookmark.view.as_ref() else { return; };
    if classic_source::binding_profile(runtime) != Some(&view.profile) { return; }
    let Some(key) = category_key(&runtime.catalog, runtime.effective_category()) else { return; };
    let Some(entry) = runtime.catalog.entries.iter().find(|entry| entry.id == id) else { return; };
    let plx_data::ott::MediaLocator::Plex { server_id, rating_key } = &entry.media else { return; };
    if server_id != &view.profile.source { return; }
    update(runtime, key, Some(rating_key.clone()));
}

pub(super) fn selected(runtime: &Runtime) -> Option<String> {
    let view = runtime.classic_bookmark.view.as_ref()?;
    if classic_source::binding_profile(runtime) != Some(&view.profile) || !view.category_applied { return None; }
    let value = view.value.as_ref()?;
    if view.selection.as_deref() != Some(runtime.effective_category()) { return None; }
    Some(format!("{}:{}", view.profile.id, value.focus.as_ref()?))
}

pub(super) fn request_flush(runtime: &mut Runtime) {
    let now = runtime.now_ms();
    if let Some(view) = runtime.classic_bookmark.view.as_mut().filter(|view| view.dirty) {
        view.due = Some(now);
    }
}

/// Reconcile once after each batch, not by scanning the catalogue on every
/// frame once resolved. Missing categories fall back visibly to All but keep
/// their stored hint, so an unavailable library is not silently forgotten.
fn restore(runtime: &mut Runtime) {
    let Some(view) = runtime.classic_bookmark.view.as_ref() else { return; };
    if !view.loaded {
        let Some(value) = public_preferences::classic_bookmark(&runtime.public_preferences, &view.source) else { return; };
        let view = runtime.classic_bookmark.view.as_mut().unwrap();
        view.value = value;
        view.loaded = true;
    }
    let view = runtime.classic_bookmark.view.as_ref().unwrap();
    if view.category_applied || view.last_catalog == Some((runtime.generation, runtime.catalog_revision)) { return; }
    let Some(value) = &view.value else {
        runtime.classic_bookmark.view.as_mut().unwrap().category_applied = true;
        return;
    };
    let category = category_id(&runtime.catalog, value.category.as_deref());
    runtime.classic_bookmark.view.as_mut().unwrap().last_catalog = Some((runtime.generation, runtime.catalog_revision));
    if let Some(category) = category {
        classic_source::set_view(runtime, &category);
        let view = runtime.classic_bookmark.view.as_mut().unwrap();
        view.category_applied = true;
        view.selection = Some(category);
        runtime.changed();
    }
}

pub(super) fn tick(runtime: &mut Runtime, app: &App) {
    if !current(runtime, app) { release(runtime); return; }
    restore(runtime);
    if runtime.saving || public_preferences::write_refusal(runtime).is_some() { return; }
    let Some(view) = runtime.classic_bookmark.view.as_ref() else { return; };
    if !view.dirty || !view.due.is_some_and(|due| runtime.now_ms() >= due) { return; }
    let Some(value) = view.value.clone() else { return; };
    // One attempt per user boundary. A known failure retains the RAM value for
    // the next boundary; an uncertain public write keeps the existing lockout.
    runtime.classic_bookmark.view.as_mut().unwrap().due = None;
    let status = runtime.status.clone();
    if runtime.save_public_preference(runtime.saved.clone(),
        public_preferences::Patch::ClassicBookmark(value.clone()), None, Value::Null).is_ok() {
        runtime.classic_bookmark.writing = Some(Write { value, category_choice: false });
        runtime.status = status;
    }
}

pub(super) fn take_write(runtime: &mut Runtime) -> Option<Write> {
    runtime.classic_bookmark.writing.take()
}
pub(super) fn acknowledged(runtime: &mut Runtime, sent: &ClassicBookmark, success: bool) {
    if !success { return; }
    if let Some(view) = runtime.classic_bookmark.view.as_mut() {
        if view.source == sent.source && view.value.as_ref() == Some(sent) {
            view.dirty = false;
            view.due = None;
        }
    }
}

/// Capture while the source is still current, before shutdown cancels the
/// catalogue. The caller drains the same writer before committing this hint.
pub(super) fn shutdown_candidate(runtime: &Runtime, app: &App) -> Option<ClassicBookmark> {
    if !current(runtime, app) || runtime.pending_category_choice.is_some() { return None; }
    // A newer admitted category transaction owns its own receipt. An older
    // dirty focus must not overwrite it after the shutdown writer drain.
    let view = runtime.classic_bookmark.view.as_ref()?;
    view.dirty.then(|| view.value.clone()).flatten()
}

#[cfg(test)]
#[path = "ott_classic_bookmark_tests.rs"]
mod tests;
