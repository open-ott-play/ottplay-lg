//! Structural folder membership. Display labels never serve as path syntax.
use super::{Catalog, CatalogError, MAX_CATALOG_ENTRIES, MAX_CATALOG_MODEL_BYTES};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Provider-relative provenance, not a catalogue ID or permission to read media.
/// The caller separately binds its fingerprint to the authorized account/server.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case", deny_unknown_fields)]
pub enum CategorySourceIdentity {
    Plex { section: i64, kind: PlexCategoryKind, source_id: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlexCategoryKind { Section, Show, Season }

impl CategorySourceIdentity {
    pub fn validate(&self) -> Result<(), CatalogError> {
        match self {
            Self::Plex { section, kind, source_id } => {
                if *section <= 0 || source_id.is_empty() || source_id.len() > 256
                    || !source_id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
                    || (*kind == PlexCategoryKind::Section && *source_id != section.to_string())
                { return Err(CatalogError::InvalidFormat); }
            }
        }
        Ok(())
    }

    /// Stable for validated provenance across labels, runtime profile IDs and
    /// endpoint changes. This digest does not identify an account or server.
    pub fn fingerprint(&self) -> String {
        let mut bytes = b"ott-category-source-v1\0".to_vec();
        match self {
            Self::Plex { section, kind, source_id } => {
                bytes.extend_from_slice(b"plex\0");
                bytes.extend_from_slice(&section.to_be_bytes());
                bytes.push(match kind { PlexCategoryKind::Section => 0, PlexCategoryKind::Show => 1, PlexCategoryKind::Season => 2 });
                bytes.extend_from_slice(source_id.as_bytes());
            }
        }
        super::favorites_wire::hash(&bytes)
    }

    fn heap_bytes(&self) -> usize {
        match self { Self::Plex { source_id, .. } => source_id.len() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CategoryNode {
    pub id: String,
    pub parent: Option<String>,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anonymous_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_identity: Option<CategorySourceIdentity>,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CategoryMembership {
    pub entry_id: String,
    pub category_id: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CategoryHierarchy {
    pub nodes: Vec<CategoryNode>,
    pub memberships: Vec<CategoryMembership>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CategoryChoice {
    pub id: String,
    pub label: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CategoryGroup {
    pub id: String,
    pub label: String,
    pub indices: Vec<usize>,
}

impl CategoryHierarchy {
    pub fn nodes_only(&self) -> Self {
        Self { nodes: self.nodes.clone(), memberships: Vec::new() }
    }
    /// Retains the complete folder closure. A sender that has already emitted
    /// nodes_only() may clear `nodes` before sending subsequent row batches.
    pub fn for_entries<'a>(&self, ids: impl IntoIterator<Item = &'a str>) -> Self {
        let ids: BTreeSet<_> = ids.into_iter().collect();
        Self { nodes: self.nodes.clone(), memberships: self.memberships.iter()
            .filter(|member| ids.contains(member.entry_id.as_str())).cloned().collect() }
    }
    pub(super) fn byte_len(&self) -> Option<usize> {
        let mut bytes = std::mem::size_of::<Self>();
        for node in &self.nodes {
            bytes = bytes.checked_add(std::mem::size_of::<CategoryNode>())?
                .checked_add(node.id.len())?.checked_add(node.label.len())?
                .checked_add(node.parent.as_ref().map_or(0, String::len))?
                .checked_add(node.anonymous_fingerprint.as_ref().map_or(0, String::len))?
                .checked_add(node.source_identity.as_ref().map_or(0, CategorySourceIdentity::heap_bytes))?;
        }
        for member in &self.memberships {
            bytes = bytes.checked_add(std::mem::size_of::<CategoryMembership>())?
                .checked_add(member.entry_id.len())?.checked_add(member.category_id.len())?;
        }
        Some(bytes)
    }
    /// Merge into a private candidate; the caller validates the complete graph
    /// against candidate row IDs before publishing either side.
    pub(super) fn merge(&mut self, incoming: &Self) -> Result<(), CatalogError> {
        let mut positions: BTreeMap<String, usize> = self.nodes.iter().enumerate()
            .map(|(index, node)| (node.id.clone(), index)).collect();
        for node in &incoming.nodes {
            if let Some(&index) = positions.get(&node.id) {
                if self.nodes[index] != *node { return Err(CatalogError::InvalidFormat); }
                self.nodes[index] = node.clone();
            } else {
                if self.nodes.len() >= MAX_CATALOG_ENTRIES { return Err(CatalogError::LimitExceeded); }
                positions.insert(node.id.clone(), self.nodes.len());
                self.nodes.push(node.clone());
            }
        }
        let mut known: BTreeSet<_> = self.memberships.iter().cloned().collect();
        for member in &incoming.memberships {
            if known.insert(member.clone()) {
                if self.memberships.len() >= MAX_CATALOG_ENTRIES { return Err(CatalogError::LimitExceeded); }
                self.memberships.push(member.clone());
            }
        }
        if self.nodes.len() > MAX_CATALOG_ENTRIES || self.memberships.len() > MAX_CATALOG_ENTRIES {
            return Err(CatalogError::LimitExceeded);
        }
        Ok(())
    }
    fn paths(&self) -> Result<Vec<Vec<usize>>, CatalogError> {
        if self.nodes.len() > MAX_CATALOG_ENTRIES || self.memberships.len() > MAX_CATALOG_ENTRIES {
            return Err(CatalogError::LimitExceeded);
        }
        let mut positions = BTreeMap::new();
        let mut source_identities = BTreeSet::new();
        for (index, node) in self.nodes.iter().enumerate() {
            if node.id.is_empty() || node.id.len() > 256 || node.id.chars().any(char::is_control)
                || node.label.trim().is_empty() || node.label.len() > 4096
                || node.anonymous_fingerprint.as_ref().is_some_and(|value| value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()))
                || positions.insert(node.id.as_str(), index).is_some()
            { return Err(CatalogError::InvalidFormat); }
            if let Some(identity) = &node.source_identity {
                identity.validate()?;
                if !source_identities.insert(identity) { return Err(CatalogError::InvalidFormat); }
            }
        }
        let mut paths = Vec::with_capacity(self.nodes.len());
        let mut projected_bytes = 0usize;
        for index in 0..self.nodes.len() {
            let mut path = Vec::new();
            let mut visited = BTreeSet::new();
            let mut current = Some(index);
            let mut label_bytes = 0usize;
            while let Some(at) = current {
                // Matches the existing VPortal traversal nesting admission.
                if path.len() >= 128 || !visited.insert(at) { return Err(CatalogError::InvalidFormat); }
                label_bytes = label_bytes.checked_add(self.nodes[at].label.len() + 3)
                    .ok_or(CatalogError::LimitExceeded)?;
                if label_bytes.saturating_sub(3) > 4096 { return Err(CatalogError::LimitExceeded); }
                path.push(at);
                current = self.nodes[at].parent.as_deref().map(|parent|
                    positions.get(parent).copied().ok_or(CatalogError::InvalidFormat)).transpose()?;
            }
            projected_bytes = projected_bytes.checked_add(path.len() * std::mem::size_of::<usize>())
                .and_then(|bytes| bytes.checked_add(label_bytes.saturating_sub(3)))
                .ok_or(CatalogError::LimitExceeded)?;
            if projected_bytes > MAX_CATALOG_MODEL_BYTES { return Err(CatalogError::LimitExceeded); }
            path.reverse();
            paths.push(path);
        }
        Ok(paths)
    }
    // Retained independently for the test-only legacy merge oracle.
    #[cfg(test)]
    pub(super) fn projection_bytes(&self) -> Result<usize, CatalogError> {
        let paths = self.paths()?;
        let positions: BTreeMap<_, _> = self.nodes.iter().enumerate().map(|(i, node)| (node.id.as_str(), i)).collect();
        let mut bytes = self.memberships.len().checked_mul(std::mem::size_of::<usize>()
            + std::mem::size_of::<&str>() + std::mem::size_of::<Vec<usize>>())
            .and_then(|bytes| bytes.checked_add(self.nodes.len() * std::mem::size_of::<CategoryGroup>()))
            .ok_or(CatalogError::LimitExceeded)?;
        for path in &paths {
            bytes = bytes.checked_add(path.len() * std::mem::size_of::<usize>())
                .ok_or(CatalogError::LimitExceeded)?;
            for &at in path {
                bytes = bytes.checked_add(self.nodes[at].label.len() + 3).ok_or(CatalogError::LimitExceeded)?;
            }
        }
        for member in &self.memberships {
            let at = *positions.get(member.category_id.as_str()).ok_or(CatalogError::InvalidFormat)?;
            bytes = bytes.checked_add(paths[at].len() * std::mem::size_of::<usize>())
                .ok_or(CatalogError::LimitExceeded)?;
        }
        Ok(bytes)
    }
    #[cfg(test)]
    pub(super) fn validate(&self, entries: &BTreeSet<&str>) -> Result<(), CatalogError> {
        self.paths()?;
        let ids: BTreeSet<_> = self.nodes.iter().map(|node| node.id.as_str()).collect();
        let mut memberships = BTreeSet::new();
        for member in &self.memberships {
            if !entries.contains(member.entry_id.as_str()) || !ids.contains(member.category_id.as_str())
                || !memberships.insert(member)
            { return Err(CatalogError::InvalidFormat); }
        }
        Ok(())
    }
    fn projection_bytes_for(&self, paths: &[Vec<usize>]) -> Result<usize, CatalogError> {
        let positions: BTreeMap<_, _> = self.nodes.iter().enumerate().map(|(i, node)| (node.id.as_str(), i)).collect();
        let mut bytes = self.memberships.len().checked_mul(std::mem::size_of::<usize>()
            + std::mem::size_of::<&str>() + std::mem::size_of::<Vec<usize>>())
            .and_then(|bytes| bytes.checked_add(self.nodes.len() * std::mem::size_of::<CategoryGroup>()))
            .ok_or(CatalogError::LimitExceeded)?;
        for path in paths {
            bytes = bytes.checked_add(path.len() * std::mem::size_of::<usize>())
                .ok_or(CatalogError::LimitExceeded)?;
            for &at in path {
                bytes = bytes.checked_add(self.nodes[at].label.len() + 3).ok_or(CatalogError::LimitExceeded)?;
            }
        }
        for member in &self.memberships {
            let at = *positions.get(member.category_id.as_str()).ok_or(CatalogError::InvalidFormat)?;
            bytes = bytes.checked_add(paths[at].len() * std::mem::size_of::<usize>())
                .ok_or(CatalogError::LimitExceeded)?;
        }
        Ok(bytes)
    }
    fn validate_memberships(&self, entries: &BTreeSet<&str>) -> Result<(), CatalogError> {
        let ids: BTreeSet<_> = self.nodes.iter().map(|node| node.id.as_str()).collect();
        let mut memberships = BTreeSet::new();
        for member in &self.memberships {
            if !entries.contains(member.entry_id.as_str()) || !ids.contains(member.category_id.as_str())
                || !memberships.insert(member)
            { return Err(CatalogError::InvalidFormat); }
        }
        Ok(())
    }
    // One fresh graph traversal supplies both admission and the projection.
    // Catalog fields are public, so no validity survives a later caller mutation.
    fn checked_paths(&self, entries: &BTreeSet<&str>) -> Result<(Vec<Vec<usize>>, usize), CatalogError> {
        let paths = self.paths()?;
        self.validate_memberships(entries)?;
        let bytes = self.byte_len().and_then(|bytes|
            bytes.checked_add(self.projection_bytes_for(&paths).ok()?))
            .ok_or(CatalogError::LimitExceeded)?;
        Ok((paths, bytes))
    }
    pub(super) fn validated_model_bytes(&self, entries: &BTreeSet<&str>) -> Result<usize, CatalogError> {
        self.checked_paths(entries).map(|(_, bytes)| bytes)
    }
    fn choices(&self) -> Result<Vec<CategoryChoice>, CatalogError> {
        let paths = self.paths()?;
        Ok(self.nodes.iter().enumerate().map(|(i, node)| CategoryChoice {
            id: node.id.clone(),
            label: paths[i].iter().map(|at| self.nodes[*at].label.as_str()).collect::<Vec<_>>().join(" / "),
        }).collect())
    }
    fn indices(&self, catalog: &Catalog, selected: &str) -> Result<Vec<usize>, CatalogError> {
        let paths = self.paths()?;
        let Some(target) = self.nodes.iter().position(|node| node.id == selected) else { return Ok(Vec::new()); };
        let descendants: BTreeSet<_> = self.nodes.iter().enumerate()
            .filter(|(i, _)| paths[*i].contains(&target)).map(|(_, node)| node.id.as_str()).collect();
        let entries: BTreeSet<_> = self.memberships.iter().filter(|member| descendants.contains(member.category_id.as_str()))
            .map(|member| member.entry_id.as_str()).collect();
        Ok(catalog.entries.iter().enumerate().filter(|(_, entry)| entries.contains(entry.id.as_str()))
            .map(|(index, _)| index).collect())
    }
    fn groups(&self, catalog: &Catalog) -> Result<Vec<CategoryGroup>, CatalogError> {
        let paths = self.paths()?;
        self.groups_for(catalog, &paths)
    }
    fn groups_for(&self, catalog: &Catalog, paths: &[Vec<usize>]) -> Result<Vec<CategoryGroup>, CatalogError> {
        let nodes: BTreeMap<_, _> = self.nodes.iter().enumerate().map(|(i, node)| (node.id.as_str(), i)).collect();
        let mut membership: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for member in &self.memberships {
            let at = *nodes.get(member.category_id.as_str()).ok_or(CatalogError::InvalidFormat)?;
            membership.entry(&member.entry_id).or_default().push(at);
        }
        let mut groups: Vec<_> = self.nodes.iter().enumerate().map(|(i, node)| CategoryGroup {
            id: node.id.clone(),
            label: paths[i].iter().map(|at| self.nodes[*at].label.as_str()).collect::<Vec<_>>().join(" / "),
            indices: Vec::new(),
        }).collect();
        for (index, entry) in catalog.entries.iter().enumerate() {
            if let Some(direct) = membership.get(entry.id.as_str()) {
                let ancestors: BTreeSet<_> = direct.iter().flat_map(|at| paths[*at].iter().copied()).collect();
                for at in ancestors { groups[at].indices.push(index); }
            }
        }
        Ok(groups)
    }
}

impl Catalog {
    /// Fallible projection for a public, possibly unvalidated catalog. Performs
    /// the same current-image admission as merge(empty), without cloning media
    /// payloads or constructing a merge candidate. Groups retain raw row order.
    pub fn checked_category_groups(&self) -> Result<Vec<CategoryGroup>, CatalogError> {
        // A merge's model budget counts the last row for each repeated ID. Keep
        // that contract here; consumers requiring unique row IDs check them too.
        let winners: BTreeMap<_, _> = self.entries.iter().map(|entry| (entry.id.as_str(), entry)).collect();
        if winners.len() > MAX_CATALOG_ENTRIES { return Err(CatalogError::LimitExceeded); }
        let rows_bytes = super::row_model_bytes(winners.values().copied())?;
        if let Some(hierarchy) = &self.hierarchy {
            let ids = winners.keys().copied().collect();
            let (paths, hierarchy_bytes) = hierarchy.checked_paths(&ids)?;
            if rows_bytes.checked_add(hierarchy_bytes).is_none_or(|bytes| bytes > MAX_CATALOG_MODEL_BYTES) {
                return Err(CatalogError::LimitExceeded);
            }
            hierarchy.groups_for(self, &paths)
        } else {
            if rows_bytes > MAX_CATALOG_MODEL_BYTES { return Err(CatalogError::LimitExceeded); }
            Ok(self.category_groups())
        }
    }
    /// One projection pass for callers publishing all categories and counts.
    pub fn category_groups(&self) -> Vec<CategoryGroup> {
        if let Some(hierarchy) = &self.hierarchy { return hierarchy.groups(self).unwrap_or_default(); }
        let mut groups: Vec<CategoryGroup> = Vec::new();
        let mut positions = BTreeMap::new();
        for (index, entry) in self.entries.iter().enumerate() {
            if entry.category.is_empty() { continue; }
            let at = *positions.entry(&entry.category).or_insert_with(|| {
                groups.push(CategoryGroup { id: entry.category.clone(), label: entry.category.clone(), indices: Vec::new() });
                groups.len() - 1
            });
            groups[at].indices.push(index);
        }
        groups
    }
    pub fn category_choices(&self) -> Vec<CategoryChoice> {
        if let Some(hierarchy) = &self.hierarchy { return hierarchy.choices().unwrap_or_default(); }
        let mut seen = BTreeSet::new();
        self.entries.iter().filter(|entry| !entry.category.is_empty() && seen.insert(&entry.category))
            .map(|entry| CategoryChoice { id: entry.category.clone(), label: entry.category.clone() }).collect()
    }
    pub fn resolve_category(&self, selection: &str) -> Option<String> {
        if selection.is_empty() { return Some(String::new()); }
        if self.hierarchy.is_none() {
            return self.entries.iter().any(|entry| entry.category == selection).then(|| selection.into());
        }
        if self.hierarchy.as_ref().is_some_and(|hierarchy| hierarchy.nodes.iter().any(|node| node.id == selection)) {
            return Some(selection.into());
        }
        let choices = self.category_choices();
        if choices.iter().any(|choice| choice.id == selection) { return Some(selection.into()); }
        let mut labels = choices.iter().filter(|choice| choice.label == selection);
        let found = labels.next()?;
        labels.next().is_none().then(|| found.id.clone())
    }
    pub fn category_indices(&self, selection: &str) -> Vec<usize> {
        if selection.is_empty() { return (0..self.entries.len()).collect(); }
        let Some(id) = self.resolve_category(selection) else { return Vec::new(); };
        if let Some(hierarchy) = &self.hierarchy { return hierarchy.indices(self, &id).unwrap_or_default(); }
        self.entries.iter().enumerate().filter(|(_, entry)| entry.category == id).map(|(index, _)| index).collect()
    }
}

#[cfg(test)]
#[path = "category_hierarchy_tests.rs"]
mod tests;
