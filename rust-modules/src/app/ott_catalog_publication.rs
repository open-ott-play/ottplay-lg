//! A successful folder-node-only admission can leave the displayed All list unchanged.
//! This is a per-tick proof, not a cached view, deferred publication or read authority.
use super::*;

pub(super) struct UnchangedAll {
    source: String,
    generation: u64,
    navigation: u64,
    owner: InstanceId,
    entries: usize,
}

fn eligible(runtime: &Runtime, app: &App) -> bool {
    runtime.page == "channels"
        && !modern_shelves::eligible(runtime)
        && runtime.view_selection() == ("", false)
        && runtime.loaded && !runtime.persistence_uncertain && !runtime.saving
        && runtime.catalog_loading && !runtime.catalog_complete
        && !runtime.catalog.entries.is_empty()
        && runtime.status == format!("{} videos", runtime.catalog.entries.len())
        && !runtime.saved.settings.contains_key(catalog_organization::KEY)
        && runtime.edit_session.is_none() && runtime.setup.is_none()
        && runtime.pending_category_choice.is_none()
        && runtime.transient.accepted.is_none() && runtime.transient.candidate.is_none()
        && managed::profile(runtime).is_none()
        && runtime.effective_profile().is_some_and(|profile| profile.kind == ProviderKind::VPortal)
        && runtime.search_view().is_some_and(|search| search.query.is_empty())
        && runtime.reply.is_some() && runtime.reply == current_ui_owner(app)
}

impl UnchangedAll {
    pub(super) fn capture(runtime: &Runtime, app: &App, incoming: &Catalog, before: u64) -> Option<Self> {
        if runtime.revision != before || !incoming.entries.is_empty()
            || !incoming.hierarchy.as_ref().is_some_and(|graph|
                !graph.nodes.is_empty() && graph.memberships.is_empty())
            || !eligible(runtime, app)
        { return None; }
        Some(Self { source: runtime.category_source_context(), generation: runtime.generation,
            navigation: runtime.navigation_epoch, owner: runtime.reply?, entries: runtime.catalog.entries.len() })
    }

    pub(super) fn current(&self, runtime: &Runtime, app: &App) -> bool {
        eligible(runtime, app) && runtime.generation == self.generation
            && runtime.navigation_epoch == self.navigation && runtime.reply == Some(self.owner)
            && runtime.catalog.entries.len() == self.entries
            && runtime.category_source_context() == self.source
    }
}
