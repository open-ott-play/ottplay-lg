//! Application worker for a structural, paged native Plex catalogue. Authentication,
//! endpoint admission, TLS and paging stay with the existing typed PMS client.
use plx_data::ott::{Catalog, CatalogEntry, CategoryHierarchy, CategoryMembership, CategoryNode, CategorySourceIdentity, MediaLocator, PlexCategoryKind, ProviderProfile};
use std::collections::BTreeMap;
use plx_plex::plex::{self, Metadata, SectionQuery};
use std::sync::{atomic::{AtomicBool, Ordering}, Arc};

/// Receipt of the native client's authority at navigation time. This neither
/// imports a server nor changes the active session, and is never a wire value.
#[derive(Clone)]
pub(super) struct ClassicRead {
    authority: Arc<super::ott_plex_prepare::CapturedAuthority>,
    scope: plex::grant::GrantScope,
    read: plex::BoundedRead,
}
impl ClassicRead {
    pub(super) fn capture(client: &plex::Client) -> Result<Self, &'static str> {
        let scope = plex::grant::scope();
        let authority = super::ott_plex_prepare::CapturedAuthority::capture(client)
            .map_err(|_| "Plex server authorization is unavailable")?;
        let registered = plex::client_for(authority.server)
            .filter(|registered| authority.matches(registered))
            .ok_or("Plex source is no longer available")?;
        let read = registered.capture_current_bounded_read(authority.client_key.token)
            .ok_or("Plex source is no longer available")?;
        let captured = Self { authority: Arc::new(authority), scope, read };
        if !captured.current() { return Err("Plex source is no longer available"); }
        Ok(captured)
    }
    pub(super) fn current(&self) -> bool {
        self.scope == plex::grant::scope() && self.read.current()
            && plex::client_for(self.authority.server).is_some_and(|client| self.authority.matches(client))
            && self.scope == plex::grant::scope()
    }
    pub(super) fn matches(&self, machine_id: &str) -> bool { self.authority.machine_id == machine_id }
    pub(super) fn authority_scope(&self)
        -> Option<(Arc<super::ott_plex_prepare::CapturedAuthority>, plex::grant::GrantScope)>
    {
        self.current().then(|| (Arc::clone(&self.authority), self.scope))
    }
    fn sections(&self) -> Option<plex::MediaContainer> {
        if !self.current() { return None; }
        let result = self.read.sections();
        self.current().then_some(result).flatten()
    }
    fn items(&self, query: &SectionQuery) -> Option<plex::MediaContainer> {
        if !self.current() { return None; }
        let result = self.read.section_items(query);
        self.current().then_some(result).flatten()
    }
    pub(super) fn metadata(&self, rating_key: &str) -> Option<Metadata> {
        if !self.current() { return None; }
        let result = self.read.metadata(rating_key);
        self.current().then_some(result).flatten()
    }
}

pub(super) fn visit_classic(profile: &ProviderProfile, read: &ClassicRead,
    cancel: &AtomicBool, emit: &mut dyn FnMut(Catalog) -> Result<(), &'static str>) -> Result<(), &'static str> {
    profile.validate().map_err(|_| "Invalid provider profile")?;
    if profile.kind != plx_data::ott::ProviderKind::Plex || !read.matches(&profile.source) {
        return Err("Plex source does not match");
    }
    visit_with(profile, cancel, emit, &Read::Classic(read))
}

pub(super) fn visit(
    profile: &ProviderProfile,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    // Favorite lookup and other row-only callers retain their existing interface.
    visit_catalog(profile, cancel, &mut |catalog| {
        if catalog.entries.is_empty() { Ok(()) } else { emit(catalog.entries) }
    })
}

pub(super) fn visit_catalog(
    profile: &ProviderProfile,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(Catalog) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    if cancel.load(Ordering::Acquire) { return Ok(()); }
    let sid = plex::id_of_machine(&profile.source).ok_or("Plex server is not connected")?;
    let client = plex::client_for(sid).ok_or("Plex server authorization is unavailable")?;
    visit_with(profile, cancel, emit, &Read::Live(client))
}

pub(super) fn visit_managed(profile: &ProviderProfile, read: &super::adapters::session::ManagedRead,
    cancel: &AtomicBool, emit: &mut dyn FnMut(Catalog) -> Result<(), &'static str>) -> Result<(), &'static str> {
    profile.validate().map_err(|_| "Invalid provider profile")?;
    if profile.kind != plx_data::ott::ProviderKind::Plex || !read.matches(&profile.source) {
        return Err("Plex source does not match");
    }
    visit_with(profile, cancel, emit, &Read::Managed(read))
}
enum Read<'a> {
    Live(&'static plex::Client), Managed(&'a super::adapters::session::ManagedRead), Classic(&'a ClassicRead),
}
trait CatalogRead {
    fn current(&self) -> bool;
    fn sections(&self) -> Option<plex::MediaContainer>;
    fn items(&self, query: &SectionQuery) -> Option<plex::MediaContainer>;
}
impl CatalogRead for Read<'_> {
    fn current(&self) -> bool { match self { Self::Live(_) => true, Self::Managed(read) => read.current(), Self::Classic(read) => read.current() } }
    fn sections(&self) -> Option<plex::MediaContainer> { match self {
        Self::Live(client) => client.sections_bounded(), Self::Managed(read) => read.sections(), Self::Classic(read) => read.sections() } }
    fn items(&self, query: &SectionQuery) -> Option<plex::MediaContainer> { match self {
        Self::Live(client) => client.section_items_query_bounded(query), Self::Managed(read) => read.section_items(query), Self::Classic(read) => read.items(query) } }
}
fn visit_with(profile: &ProviderProfile, cancel: &AtomicBool,
    emit: &mut dyn FnMut(Catalog) -> Result<(), &'static str>, read: &impl CatalogRead) -> Result<(), &'static str> {
    if cancel.load(Ordering::Acquire) { return Ok(()); }
    if !read.current() { return Err("Plex source is no longer available"); }
    let sections = read.sections().ok_or("Plex libraries could not be loaded")?;
    if cancel.load(Ordering::Acquire) { return Ok(()); }
    if !read.current() { return Err("Plex source is no longer available"); }
    let mut projection = Projection::default();
    let mut scanned = 0usize;
    let mut pages = 0usize;
    let mut first = true;
    for section in sections.directory {
        if cancel.load(Ordering::Relaxed) { return Ok(()); }
        let media_type = match section.kind.as_str() {
            "movie" => "1",
            "show" => "4",
            _ => continue,
        };
        let key = section.key.parse::<i64>().map_err(|_| "Plex returned an invalid library key")?;
        let mut section_batch = Catalog::default();
        let section_id = projection.node(profile, key, ("section", &key.to_string()), None,
            label_or(&section.title, format!("Library {key}")), &mut section_batch)?;
        // A supported library is browsable even if it has no playable entries.
        if !read.current() { return Err("Plex source is no longer available"); }
        if section_batch.hierarchy.is_some() {
            if cancel.load(Ordering::Acquire) { return Ok(()); }
            emit(section_batch)?;
            if !read.current() { return Err("Plex source is no longer available"); }
        }
        let filters = vec![("type".to_owned(), media_type.to_owned())];
        let mut offset = 0;
        loop {
            if cancel.load(Ordering::Relaxed) { return Ok(()); }
            if !read.current() { return Err("Plex source is no longer available"); }
            pages += 1;
            if pages > 4096 { return Err("Plex catalogue page limit reached"); }
            let page = read.items(&SectionQuery {
                section_key: key, sort: "titleSort", filters: &filters,
                start: offset, size: 128, include_meta: false,
            }).ok_or("Plex library page could not be loaded")?;
            if cancel.load(Ordering::Acquire) { return Ok(()); }
            if !read.current() { return Err("Plex source is no longer available"); }
            scanned = scanned.saturating_add(page.metadata.len());
            if scanned > plx_data::ott::MAX_CATALOG_ENTRIES { return Err("Plex catalogue entry limit reached"); }
            let count = page.metadata.len() as i64;
            let (next, complete) = page_progress(offset, page.offset, count, page.total_size)?;
            let mut batch = Catalog::default();
            for item in page.metadata {
                if cancel.load(Ordering::Acquire) { return Ok(()); }
                if !read.current() { return Err("Plex source is no longer available"); }
                projection.push(profile, key, &section_id, &section.title, item, &mut batch)?;
                if first && !batch.entries.is_empty() {
                    // Keep the first playable row available before the rest of its page.
                    if cancel.load(Ordering::Acquire) { return Ok(()); }
                    if !read.current() { return Err("Plex source is no longer available"); }
                    emit(std::mem::take(&mut batch))?;
                    if !read.current() { return Err("Plex source is no longer available"); }
                    first = false;
                }
            }
            if !batch.entries.is_empty() {
                if cancel.load(Ordering::Acquire) { return Ok(()); }
                if !read.current() { return Err("Plex source is no longer available"); }
                emit(batch)?;
                if !read.current() { return Err("Plex source is no longer available"); }
            }
            offset = next;
            if complete { break; }
        }
    }
    if !read.current() { return Err("Plex source is no longer available"); }
    Ok(())
}

/// Source identity owns membership; titles remain literal display text. Keep the
/// first label for a repeated node so later pages cannot rename an emitted graph.
#[derive(Default)]
struct Projection { parents: BTreeMap<String, Option<String>> }
impl Projection {
    fn node(&mut self, profile: &ProviderProfile, section: i64, source: (&str, &str),
        parent: Option<&str>, label: String, batch: &mut Catalog) -> Result<String, &'static str> {
        let (kind, source_id) = source;
        let source_identity = CategorySourceIdentity::Plex { section,
            kind: match kind {
                "section" => PlexCategoryKind::Section,
                "show" => PlexCategoryKind::Show,
                "season" => PlexCategoryKind::Season,
                _ => return Err("Plex returned an invalid category kind"),
            }, source_id: source_id.into() };
        source_identity.validate().map_err(|_| "Plex returned an invalid category identity")?;
        let scope = serde_json::to_vec(&(profile.id.as_str(), profile.source.as_str(), section, kind, source_id))
            .map_err(|_| "Plex catalogue identity could not be encoded")?;
        let id = format!("plex:{kind}:{}", plx_data::ott::favorites_wire::hash(&scope));
        let parent = parent.map(str::to_owned);
        if let Some(known) = self.parents.get(&id) {
            if known != &parent { return Err("Plex returned a different catalogue parent"); }
            return Ok(id);
        }
        if self.parents.len() >= plx_data::ott::MAX_CATALOG_ENTRIES {
            return Err("Plex catalogue category limit reached");
        }
        self.parents.insert(id.clone(), parent.clone());
        batch.hierarchy.get_or_insert_with(CategoryHierarchy::default).nodes.push(CategoryNode {
            id: id.clone(), parent, label, anonymous_fingerprint: None, source_identity: Some(source_identity),
        });
        Ok(id)
    }

    fn push(&mut self, profile: &ProviderProfile, section: i64, section_id: &str,
        library: &str, item: Metadata, batch: &mut Catalog) -> Result<(), &'static str> {
        if !playable(&item) { return Ok(()); }
        let mut parent = section_id.to_owned();
        if item.kind == "episode" && valid_rating_key(&item.grandparent_rating_key) {
            parent = self.node(profile, section, ("show", &item.grandparent_rating_key), Some(&parent),
                label_or(&item.grandparent_title, format!("Series {}", item.grandparent_rating_key)), batch)?;
            // A season requires a source-attested show -> season chain. Missing
            // IDs leave the row under its nearest proven ancestor, never a title-derived ID.
            if valid_rating_key(&item.parent_rating_key) {
                parent = self.node(profile, section, ("season", &item.parent_rating_key), Some(&parent),
                    label_or(&item.parent_title, format!("Season {}", item.parent_index.unwrap_or(0))), batch)?;
            }
        }
        if let Some(row) = entry(profile, library, item) {
            batch.hierarchy.get_or_insert_with(CategoryHierarchy::default).memberships.push(CategoryMembership {
                entry_id: row.id.clone(), category_id: parent,
            });
            batch.entries.push(row);
        }
        Ok(())
    }
}
fn label_or(label: &str, fallback: String) -> String {
    if label.trim().is_empty() { fallback } else { label.to_owned() }
}
fn valid_rating_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 256
        && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
fn playable(item: &Metadata) -> bool {
    matches!(item.kind.as_str(), "movie" | "episode" | "clip") && valid_rating_key(&item.rating_key)
}

fn page_progress(
    offset: i64,
    reported: i64,
    count: i64,
    total: i64,
) -> Result<(i64, bool), &'static str> {
    if reported != offset || offset < 0 || count < 0 || total < 0 {
        return Err("Plex returned a different catalogue page");
    }
    if count == 0 && total > offset {
        return Err("Plex catalogue is incomplete");
    }
    let next = offset
        .checked_add(count)
        .ok_or("Plex catalogue is too large")?;
    Ok((
        next,
        count == 0 || (total > 0 && next >= total) || (total == 0 && count < 128),
    ))
}

fn entry(profile: &ProviderProfile, library: &str, item: Metadata) -> Option<CatalogEntry> {
    if !playable(&item) { return None; }
    let category = if item.kind == "episode" && !item.grandparent_title.is_empty() {
        format!(
            "{library} / {} / {}",
            item.grandparent_title,
            if item.parent_title.is_empty() {
                format!("Season {}", item.parent_index.unwrap_or(0))
            } else {
                item.parent_title.clone()
            }
        )
    } else {
        library.to_owned()
    };
    let title = if item.kind == "episode" && !item.grandparent_title.is_empty() {
        format!("{} — {}", item.grandparent_title, item.title)
    } else {
        item.title
    };
    Some(CatalogEntry {
        id: format!("{}:{}", profile.id, item.rating_key),
        title,
        category,
        description: item.summary,
        poster: if !item.thumb.is_empty() {
            item.thumb
        } else {
            item.art
        },
        duration_ms: u64::try_from(item.duration)
            .ok()
            .filter(|duration| *duration > 0),
        provider_id: Some(item.rating_key.clone()),
        provider_id_field: None,
        live: None,
        media: MediaLocator::Plex {
            server_id: profile.source.clone(),
            rating_key: item.rating_key,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn classic_metadata_rejects_invalid_keys_and_revoked_read_before_io() {
        let _serial = plx_base::testlock::serial();
        struct Reset;
        impl Drop for Reset { fn drop(&mut self) { plex::reset_servers_for_test(); } }
        let _reset = Reset;
        let sid = plex::register_pinned_with_client_id("classic-metadata",
            &plex::Origin::parse("https://classic-metadata.invalid:32400").unwrap(),
            "original-token", None, "fixture", Default::default());
        let client = plex::client_for(sid).unwrap();
        let read = ClassicRead::capture(client).unwrap();
        // BoundedRead validates keys before entering its HTTP path.
        for key in ["", "../42", "42?token=other", "123456789012345678901"] {
            assert!(read.metadata(key).is_none());
        }
        client.set_token("replacement-token");
        assert!(!read.current());
        // The valid key still cannot reach HTTP through this revoked receipt.
        assert!(read.metadata("42").is_none());
    }

    #[test]
    fn paging_does_not_mistake_truncation_or_repeated_first_page_for_completion() {
        assert_eq!(page_progress(0, 0, 128, 300).unwrap(), (128, false));
        assert_eq!(page_progress(256, 256, 44, 300).unwrap(), (300, true));
        assert!(page_progress(128, 128, 0, 300).is_err());
        assert!(page_progress(128, 0, 128, 300).is_err());
        assert!(page_progress(0, 128, 128, 300).is_err());
        assert!(page_progress(i64::MAX, i64::MAX, 1, 0).is_err());
    }
    #[test]
    fn episode_row_keeps_full_description_and_native_server_identity() {
        let profile = ProviderProfile {
            id: "plex-1".into(),
            name: "Home".into(),
            kind: plx_data::ott::ProviderKind::Plex,
            source: "server-1".into(),
            source_alias: String::new(),
            enabled: true,
        };
        let item:Metadata=serde_json::from_str(r#"{"type":"episode","ratingKey":"42","title":"Episode","grandparentTitle":"Series","parentTitle":"Season 2","summary":"First sentence. Second sentence.","thumb":"/library/metadata/42/thumb/1","duration":120000}"#).unwrap();
        let row = entry(&profile, "TV", item).unwrap();
        assert_eq!(row.category, "TV / Series / Season 2");
        assert_eq!(row.description, "First sentence. Second sentence.");
        assert_eq!(row.duration_ms, Some(120000));
        assert_eq!(row.poster, "/library/metadata/42/thumb/1");
        assert_eq!(
            row.media,
            MediaLocator::Plex {
                server_id: "server-1".into(),
                rating_key: "42".into()
            }
        );
    }
}

#[cfg(test)]
#[path = "ott_plex_catalog_tests.rs"]
mod catalog_tests;
