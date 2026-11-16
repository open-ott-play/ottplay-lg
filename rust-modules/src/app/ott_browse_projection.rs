//! Presentation facts for native provider/profile/category cards. No IO, source
//! selection, persistence or playback requests are owned by this projection.
use super::*;
use plx_machine::machine::UiMode;
use plx_platform::i18n::msg;
use std::collections::HashMap;

const ART_VISITS: usize = 256;
const ART_BYTES: usize = 256 * 1024;

fn eligible(runtime: &Runtime) -> bool {
    runtime.interface_mode() == UiMode::Modern
        && matches!(runtime.page, "providers" | "profiles" | "categories")
}

pub(super) fn context(runtime: &Runtime) -> Option<String> {
    if !eligible(runtime) { return None; }
    // This digest only retires presentation memory. It is not PreviewAt or
    // playback authority, and it never exposes the serialized source values.
    Some(plx_data::ott::favorites_wire::hash(json!([
        runtime.remote.runtime(), runtime.generation, runtime.page, runtime.provider,
        runtime.effective_profile(), runtime.saved.profiles,
        runtime.local_inventory.generation(), runtime.m3u_attachments.context_revision(),
    ]).to_string().as_bytes()))
}

struct Summary { count: usize, poster: Option<OttPoster> }
struct Budget { visits: usize, bytes: usize }
impl Budget {
    fn poster<'a>(&mut self, runtime: &Runtime,
        entries: impl Iterator<Item = &'a CatalogEntry>) -> Option<OttPoster>
    {
        for entry in entries {
            if self.visits == 0 { break; }
            self.visits -= 1;
            let art = if matches!(entry.media, MediaLocator::Local { .. }) {
                runtime.local_entry_art(entry)
            } else { entry_poster(entry) };
            let Some(art) = art else { continue; };
            let bytes = match &art {
                OttPoster::Http { url } => url.len(),
                OttPoster::Plex { key, .. } => key.len(),
                OttPoster::Local { .. } => std::mem::size_of::<OttPoster>(),
            };
            if bytes > self.bytes { continue; }
            self.bytes -= bytes;
            return Some(art);
        }
        None
    }
}

fn categories(runtime: &Runtime) -> HashMap<String, Summary> {
    let mut out = HashMap::new();
    let mut budget = Budget { visits: ART_VISITS, bytes: ART_BYTES };
    let all = runtime.organization_rows("");
    out.insert(String::new(), Summary { count: all.len(),
        poster: budget.poster(runtime, all.iter().map(|(_, entry)| *entry)) });
    if let Some(projection) = runtime.organization_projection() {
        // One index per publication; never rescan the entire catalogue once for
        // every category. Overlap shares borrowed canonical metadata.
        let entries: HashMap<_, _> = runtime.catalog.entries.iter()
            .map(|entry| (entry.id.as_str(), entry)).collect();
        for category in &projection.categories {
            out.insert(category.id.clone(), Summary { count: category.members.len(),
                poster: budget.poster(runtime, category.members.iter()
                    .filter_map(|id| entries.get(id.as_str()).copied())) });
        }
    } else if let Ok(groups) = runtime.catalog.checked_category_groups() {
        for category in groups {
            out.insert(category.id, Summary { count: category.indices.len(),
                poster: budget.poster(runtime, category.indices.into_iter()
                    .filter_map(|index| runtime.catalog.entries.get(index))) });
        }
    }
    out
}

pub(super) fn decorate(runtime: &Runtime, rows: &mut [plx_screens::ott::OttRow]) {
    if !eligible(runtime) { return; }
    let summaries = (runtime.page == "categories").then(|| categories(runtime));
    for item in rows {
        match &item.action {
            OttAction::SelectProvider(id) => {
                let count = runtime.saved.profiles.profiles.iter()
                    .filter(|profile| profile.enabled && profile.kind.canonical_id() == id.as_str()
                        && !managed::shadows_profile(runtime, profile)).count()
                    + usize::from(id == ProviderKind::Plex.canonical_id() && managed::row(runtime).is_some());
                item.content_title = Some(item.title.clone());
                item.detail = format!("{} · {count}", msg::ott_profiles());
            }
            OttAction::SelectProfile(_) | OttAction::SelectManagedPlex { .. } => {
                item.content_title = Some(item.title.clone());
                // Existing profile rows already own their redacted detail.
                // Do not borrow artwork from another profile's last catalogue.
            }
            OttAction::SelectCategoryAt { category, .. } => {
                item.content_title = Some(if category.is_empty() {
                    msg::browse_library_all().into()
                } else { item.title.clone() });
                if let Some(summary) = summaries.as_ref().and_then(|map| map.get(category)) {
                    item.detail = msg::browse_search_items(summary.count.min(i64::MAX as usize) as i64);
                    // Representative member artwork is a collection thumbnail,
                    // not an invented folder synopsis or new playable locator.
                    item.poster = summary.poster.clone();
                }
            }
            OttAction::ShowFavorites if runtime.page == "categories" => {
                item.content_title = Some(msg::ott_favorites().into());
                item.detail = msg::browse_search_items(runtime.saved.favorites.entries().len()
                    .min(i64::MAX as usize) as i64);
            }
            _ => {}
        }
    }
}
