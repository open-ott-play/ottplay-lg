//! Decode the sole Saved.settings kiosk value. No IO, cache, counter or writes.
//! A malformed PRESENT value is restricted, never silently treated as Off.
use super::ott_kiosk_policy::State;
use super::ott_m3u_attachment::KioskBinding;
use plx_data::ott::favorites::{Favorite, Origin, DOCUMENT_BYTES, ENTRY_COUNT};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub(super) const KEY: &str = "nativeKiosk";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Stored {
    version: u32,
    strict: bool,
    /// A source scope, using the existing portable Origin fields. This is not
    /// permission to resolve a source: the existing binder must still match it.
    origin: Origin,
    target: Target,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Target {
    Armed {},
    Item {
        entry: Favorite,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attachment: Option<KioskBinding>,
    },
    /// Ordered pins only. Cursor/playback identity remain in the media owner.
    /// Repeated members are retained; this is not a Favorites document.
    Queue {
        entries: Vec<Favorite>,
    },
    LocalFolder {
        source_id: String,
        source_fingerprint: String,
    },
}

fn label(value: &str, limit: usize) -> bool {
    !value.is_empty()
        && value.encode_utf16().count() <= limit
        && !value.contains("://")
        && !value.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}')
        && !value.starts_with(trim_space)
        && !value.ends_with(trim_space)
}

fn trim_space(c: char) -> bool {
    matches!(c, '\u{09}'..='\u{0d}' | '\u{20}' | '\u{a0}' | '\u{1680}'
        | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}'
        | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

fn valid_origin(origin: &Origin) -> bool {
    // Native providers supported by the existing binder. Future source kinds
    // require an explicit decoder update; unknown kinds stay restricted.
    matches!(
        origin.provider.as_str(),
        "m3u" | "xtream" | "stalker" | "vportal" | "local" | "plex"
    ) && label(&origin.profile, 256)
        && label(&origin.source_alias, 256)
        && origin.subprofile.as_ref().is_none_or(|v| label(v, 256))
}

#[derive(Clone, Copy)]
pub(super) enum TargetRef<'a> {
    Armed,
    Item(&'a Favorite),
    Queue(&'a [Favorite]),
    LocalFolder {
        source_id: &'a str,
        source_fingerprint: &'a str,
    },
}

fn valid_source_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
}
fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl Stored {
    pub(super) fn armed(origin: Origin, strict: bool) -> Result<Self, ()> {
        Self::checked(origin, strict, Target::Armed {})
    }
    pub(super) fn item(entry: Favorite, strict: bool) -> Result<Self, ()> {
        Self::checked(entry.origin.clone(), strict, Target::Item { entry, attachment: None })
    }
    pub(super) fn attached_item(entry: Favorite, strict: bool, binding: KioskBinding) -> Result<Self, ()> {
        Self::checked(entry.origin.clone(), strict, Target::Item { entry, attachment: Some(binding) })
    }
    pub(super) fn attachment_binding(&self) -> Option<&KioskBinding> {
        match &self.target {
            Target::Item { attachment, .. } => attachment.as_ref(),
            _ => None,
        }
    }
    pub(super) fn queue(entries: Vec<Favorite>, strict: bool) -> Result<Self, ()> {
        let origin = entries.first().ok_or(())?.origin.clone();
        Self::checked(origin, strict, Target::Queue { entries })
    }
    pub(super) fn local_folder(
        origin: Origin,
        strict: bool,
        source_id: String,
        source_fingerprint: String,
    ) -> Result<Self, ()> {
        Self::checked(
            origin,
            strict,
            Target::LocalFolder {
                source_id,
                source_fingerprint,
            },
        )
    }
    fn checked(origin: Origin, strict: bool, target: Target) -> Result<Self, ()> {
        let value = Self {
            version: 1,
            strict,
            origin,
            target,
        };
        value.encode()?;
        Ok(value)
    }
    pub(super) fn strict(&self) -> bool {
        self.strict
    }
    pub(super) fn origin(&self) -> &Origin {
        &self.origin
    }
    pub(super) fn same_target(&self, other: &Self) -> bool {
        self.version == other.version && self.origin == other.origin && self.target == other.target
    }
    pub(super) fn with_strict(&self, strict: bool) -> Self {
        let mut next = self.clone();
        next.strict = strict;
        next
    }
    pub(super) fn target(&self) -> TargetRef<'_> {
        match &self.target {
            Target::Armed {} => TargetRef::Armed,
            Target::Item { entry, .. } => TargetRef::Item(entry),
            Target::Queue { entries } => TargetRef::Queue(entries),
            Target::LocalFolder {
                source_id,
                source_fingerprint,
            } => TargetRef::LocalFolder {
                source_id,
                source_fingerprint,
            },
        }
    }
    /// Borrow pins for the existing proof owner's retained-union checks.
    pub(super) fn pins(&self) -> &[Favorite] {
        match &self.target {
            Target::Item { entry, .. } => std::slice::from_ref(entry),
            Target::Queue { entries } => entries,
            _ => &[],
        }
    }
    pub(super) fn encode(&self) -> Result<Value, ()> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(|_| ())?;
        if serde_json::to_vec(&value).map_err(|_| ())?.len() > DOCUMENT_BYTES {
            return Err(());
        }
        Ok(value)
    }

    fn validate(&self) -> Result<(), ()> {
        if self.version != 1 || !valid_origin(&self.origin) {
            return Err(());
        }
        let valid_entry =
            |entry: &Favorite| entry.origin == self.origin && entry.validate().is_ok();
        match &self.target {
            Target::Armed {} => Ok(()),
            Target::Item { entry, attachment } if valid_entry(entry)
                && attachment.as_ref().is_none_or(|binding| binding.validate().is_ok()
                    && entry.origin.provider == "m3u"
                    && matches!(entry.locator, plx_data::ott::favorites::Locator::VPortal { .. })) => Ok(()),
            Target::Queue { entries }
                if !entries.is_empty()
                    && entries.len() <= ENTRY_COUNT
                    && entries.iter().all(valid_entry) =>
            {
                Ok(())
            }
            Target::LocalFolder {
                source_id,
                source_fingerprint,
            } if self.origin.provider == "local"
                && valid_source_id(source_id)
                && valid_digest(source_fingerprint) =>
            {
                Ok(())
            }
            _ => Err(()),
        }
    }

    #[cfg(test)]
    pub(super) fn state(&self) -> State {
        match self.target {
            Target::Armed {} => State::Armed {
                strict: self.strict,
            },
            Target::Item { .. } | Target::Queue { .. } | Target::LocalFolder { .. } => {
                State::Locked {
                    strict: self.strict,
                }
            }
        }
    }
}

pub(super) fn read(settings: &BTreeMap<String, Value>) -> Result<Option<Stored>, ()> {
    let Some(value) = settings.get(KEY) else {
        return Ok(None);
    };
    // Reuse the portable locator document's bound; never allocate an unbounded
    // second state owner. Saved already bounds its whole settings value too.
    if serde_json::to_vec(value).map_err(|_| ())?.len() > DOCUMENT_BYTES {
        return Err(());
    }
    let stored: Stored = serde_json::from_value(value.clone()).map_err(|_| ())?;
    stored.validate()?;
    Ok(Some(stored))
}

#[cfg(test)]
pub(super) fn state(settings: &BTreeMap<String, Value>) -> Result<State, ()> {
    Ok(read(settings)?.map_or(State::Off, |stored| stored.state()))
}

/// Allocation-free access to an image already accepted by Saved::validate.
/// This is a projection, not a second admission path. All load/import/write
/// publication must run the full bounded `read` validator first.
#[derive(Clone, Copy)]
pub(super) struct Borrowed<'a> {
    strict: bool,
    target: BorrowedTarget<'a>,
}
#[derive(Clone, Copy)]
pub(super) enum BorrowedTarget<'a> {
    Armed,
    Item(PinEntry<'a>),
    Queue(PinEntries<'a>),
    LocalFolder {
        source_id: &'a str,
        source_fingerprint: &'a str,
    },
}
#[derive(Clone, Copy)]
pub(super) struct PinEntries<'a>(&'a [Value]);
#[derive(Clone, Copy)]
pub(super) struct PinEntry<'a>(&'a Value);
impl<'a> PinEntries<'a> {
    pub(super) fn len(self) -> usize {
        self.0.len()
    }
    #[cfg(test)]
    pub(super) fn get(self, index: usize) -> Option<PinEntry<'a>> {
        self.0.get(index).map(PinEntry)
    }
    pub(super) fn iter(self) -> impl ExactSizeIterator<Item = PinEntry<'a>> {
        self.0.iter().map(PinEntry)
    }
}
fn text_at<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str()
}
fn valid_item_attachment(target: &Value) -> bool {
    match target.get("attachment") {
        None | Some(Value::Null) => true,
        Some(binding) => KioskBinding::validate_json(binding).is_ok()
            && target.get("entry").and_then(|entry| entry.get("origin"))
                .and_then(|origin| text_at(origin, "provider")) == Some("m3u")
            && target.get("entry").and_then(|entry| entry.get("locator"))
                .and_then(|locator| text_at(locator, "kind")) == Some("vportal-media"),
    }
}
fn optional_text_eq(value: Option<&Value>, text: Option<&str>) -> bool {
    match (value, text) {
        (Some(Value::Null), None) => true,
        (Some(v), Some(s)) => v.as_str() == Some(s),
        _ => false,
    }
}
fn origin_eq(value: &Value, origin: &Origin) -> bool {
    value.as_object().is_some_and(|v| v.len() == 4)
        && text_at(value, "provider") == Some(origin.provider.as_str())
        && text_at(value, "profile") == Some(origin.profile.as_str())
        && text_at(value, "sourceAlias") == Some(origin.source_alias.as_str())
        && optional_text_eq(value.get("subprofile"), origin.subprofile.as_deref())
}
impl<'a> PinEntry<'a> {
    pub(super) fn title(self) -> Option<&'a str> {
        text_at(self.0, "title")
    }
    /// Same identity as Favorite::identity, without serializing either side.
    pub(super) fn matches(self, entry: &Favorite) -> bool {
        use plx_data::ott::favorites::Locator;
        if !self.0.as_object().is_some_and(|v| v.len() == 3)
            || !self
                .0
                .get("origin")
                .is_some_and(|v| origin_eq(v, &entry.origin))
        {
            return false;
        }
        let Some(locator) = self.0.get("locator") else {
            return false;
        };
        let Some(object) = locator.as_object() else {
            return false;
        };
        let pair = |kind: &str, a: (&str, &str), b: (&str, &str)| {
            object.len() == 3
                && text_at(locator, "kind") == Some(kind)
                && text_at(locator, a.0) == Some(a.1)
                && text_at(locator, b.0) == Some(b.1)
        };
        match &entry.locator {
            Locator::Plex {
                server_id,
                rating_key,
            } => pair(
                "plex-media",
                ("serverId", server_id),
                ("ratingKey", rating_key),
            ),
            Locator::Local {
                root_alias,
                relative_path,
            } => pair(
                "local-media",
                ("rootAlias", root_alias),
                ("relativePath", relative_path),
            ),
            Locator::VPortal { id_field, id } => {
                pair("vportal-media", ("idField", id_field), ("id", id))
            }
            Locator::Channel { channel_id } => {
                object.len() == 2
                    && text_at(locator, "kind") == Some("channel")
                    && text_at(locator, "channelId") == Some(channel_id)
            }
            Locator::ChannelName { name, group } => {
                object.len() == 3
                    && text_at(locator, "kind") == Some("channel-name")
                    && text_at(locator, "name") == Some(name)
                    && optional_text_eq(locator.get("group"), group.as_deref())
            }
        }
    }
    pub(super) fn matches_plex(self, server_id: &str, rating_key: &str) -> bool {
        let Some(origin) = self.0.get("origin") else {
            return false;
        };
        let Some(locator) = self.0.get("locator") else {
            return false;
        };
        self.0.as_object().is_some_and(|v| v.len() == 3)
            && origin.as_object().is_some_and(|v| v.len() == 4)
            && text_at(origin, "provider") == Some("plex")
            && text_at(origin, "profile") == Some("Plex")
            && origin.get("subprofile") == Some(&Value::Null)
            && text_at(origin, "sourceAlias") == Some(server_id)
            && locator.as_object().is_some_and(|v| v.len() == 3)
            && text_at(locator, "kind") == Some("plex-media")
            && text_at(locator, "serverId") == Some(server_id)
            && text_at(locator, "ratingKey") == Some(rating_key)
    }
    pub(super) fn equals(self, entry: &Favorite) -> bool {
        self.title() == Some(entry.title.as_str()) && self.matches(entry)
    }
}
impl<'a> Borrowed<'a> {
    pub(super) fn target(self) -> BorrowedTarget<'a> {
        self.target
    }
    pub(super) fn state(self) -> State {
        match self.target {
            BorrowedTarget::Armed => State::Armed {
                strict: self.strict,
            },
            _ => State::Locked {
                strict: self.strict,
            },
        }
    }
}
pub(super) fn borrow_validated(
    settings: &BTreeMap<String, Value>,
) -> Result<Option<Borrowed<'_>>, ()> {
    let Some(value) = settings.get(KEY) else {
        return Ok(None);
    };
    let object = value.as_object().ok_or(())?;
    if object.len() != 4
        || value.get("version").and_then(Value::as_u64) != Some(1)
        || !object.contains_key("origin")
    {
        return Err(());
    }
    let strict = value.get("strict").and_then(Value::as_bool).ok_or(())?;
    let target = value.get("target").ok_or(())?;
    let fields = target.as_object().ok_or(())?;
    let target = match text_at(target, "kind").ok_or(())? {
        "armed" if fields.len() == 1 => BorrowedTarget::Armed,
        "item" if (fields.len() == 2 || (fields.len() == 3 && fields.contains_key("attachment")))
            && valid_item_attachment(target) => {
            BorrowedTarget::Item(PinEntry(target.get("entry").ok_or(())?))
        }
        "queue" if fields.len() == 2 => {
            let entries = target.get("entries").and_then(Value::as_array).ok_or(())?;
            if entries.is_empty() || entries.len() > ENTRY_COUNT {
                return Err(());
            }
            BorrowedTarget::Queue(PinEntries(entries))
        }
        "local_folder" if fields.len() == 3 => {
            let source_id = text_at(target, "source_id")
                .filter(|id| valid_source_id(id))
                .ok_or(())?;
            let source_fingerprint = text_at(target, "source_fingerprint")
                .filter(|h| valid_digest(h))
                .ok_or(())?;
            BorrowedTarget::LocalFolder {
                source_id,
                source_fingerprint,
            }
        }
        _ => return Err(()),
    };
    Ok(Some(Borrowed { strict, target }))
}

#[cfg(test)]
#[path = "ott_kiosk_stored_tests.rs"]
mod tests;
