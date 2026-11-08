//! Source-fenced UI metadata and explicit focus admission. No I/O or ownership changes.
use super::*;

const STALE: &str = "The preview source changed; select the item again";

/// Reuse the category/source epoch and the actual mixed-source binding inputs.
/// A display repaint or progressive catalogue append alone does not retire it.
pub(super) fn context(runtime: &Runtime) -> Option<String> {
    let (_, favorites) = runtime.view_selection();
    if !matches!((runtime.page, favorites), ("channels", false) | ("favorites", true))
        || !runtime.loaded || runtime.persistence_uncertain || !runtime.source_work_allowed()
    { return None; }
    let value = json!([
        runtime.category_action_context(), runtime.page, runtime.view_selection(),
        favorites.then_some(&runtime.saved.profiles),
        favorites.then_some(&runtime.saved.favorites),
        favorites.then_some(&runtime.saved.local_favorite_proofs),
        runtime.local_inventory.generation(), runtime.m3u_attachments.context_revision(),
        runtime.saved.settings.get("nativePlexSources"),
    ]);
    Some(plx_data::ott::favorites_wire::hash(value.to_string().as_bytes()))
}

fn visible(runtime: &Runtime, id: &str) -> bool {
    let search = runtime.search_filter();
    if runtime.page == "favorites" {
        let Some(identity) = id.strip_prefix("favorite:") else { return false; };
        runtime.saved.favorites.entries().iter()
            .any(|favorite| search(&favorite.title) && favorite.identity() == identity)
    } else {
        runtime.ordered_ids().iter().any(|current| current == id)
            && runtime.catalog.entries.iter().any(|entry| entry.id == id && search(&entry.title))
    }
}

/// Normalization occurs before any cancellation, settings or metadata mutation.
/// Internal callers keep their existing unscoped Preview behavior.
pub(super) fn resolve(runtime: &Runtime, action: OttAction) -> Result<OttAction, &'static str> {
    let (expected, id) = match &action {
        OttAction::PreviewAt { context, id } | OttAction::FocusAt { context, id } => (context, id),
        _ => return Ok(action),
    };
    if context(runtime).as_deref() != Some(expected.as_str()) || !visible(runtime, id) {
        return Err(STALE);
    }
    match action {
        OttAction::PreviewAt { id, .. } => Ok(OttAction::Preview(id)),
        focus => Ok(focus),
    }
}
