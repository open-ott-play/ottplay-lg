//! Native HLS selection settings, independent of Plex and interface language.
use super::*;
use plx_media::route::{HlsSubtitleMode, HlsSubtitlePolicy};

pub(in crate::app::ott_runtime) const MODE: &str = "nativeSubtitleMode";
pub(in crate::app::ott_runtime) const LANGUAGE: &str = "nativeSubtitleLanguage";
const LANGUAGES: &[(&str, &str)] = &[("", "Manifest default"), ("en", "English"),
    ("ru", "Russian"), ("uk", "Ukrainian"), ("de", "German"), ("fr", "French"),
    ("es", "Spanish"), ("it", "Italian"), ("pt", "Portuguese"), ("pl", "Polish"),
    ("ja", "Japanese"), ("ko", "Korean"), ("zh", "Chinese")];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app::ott_runtime) enum Field { Mode, Language }
impl Field {
    pub(in crate::app::ott_runtime) const ALL: [Self; 2] = [Self::Mode, Self::Language];
    pub(in crate::app::ott_runtime) fn id(self) -> &'static str { match self { Self::Mode => MODE, Self::Language => LANGUAGE } }
    pub(in crate::app::ott_runtime) fn default_value(self) -> Value { match self { Self::Mode => json!("off"), Self::Language => json!("") } }
    pub(in crate::app::ott_runtime) fn value(self, policy: &HlsSubtitlePolicy) -> Value {
        match self { Self::Mode => json!(policy.mode), Self::Language => json!(policy.language) }
    }
    pub(in crate::app::ott_runtime) fn valid(self, value: &Value) -> bool {
        match self {
            Self::Mode => matches!(value.as_str(), Some("off" | "auto" | "forced")),
            Self::Language => value.as_str().is_some_and(valid_language),
        }
    }
    pub(in crate::app::ott_runtime) fn find(id: &str) -> Option<Self> { Self::ALL.into_iter().find(|field| field.id() == id) }
    pub(in crate::app::ott_runtime) fn index(self, value: &Value) -> Option<u64> {
        if !self.valid(value) { return None; }
        Some(match self {
            Self::Mode => ["off", "auto", "forced"].iter().position(|mode| Some(*mode) == value.as_str())? as u64,
            Self::Language => LANGUAGES.iter().position(|(code, _)| Some(*code) == value.as_str()).map_or(u64::MAX, |index| index as u64),
        })
    }
    pub(in crate::app::ott_runtime) fn choices(self) -> Vec<(u64, String)> {
        match self {
            Self::Mode => ["Off (manual selection)", "Automatic", "Forced only"].into_iter().enumerate().map(|(index, label)| (index as u64, label.into())).collect(),
            Self::Language => LANGUAGES.iter().enumerate().map(|(index, (_, label))| (index as u64, (*label).into())).collect(),
        }
    }
    pub(in crate::app::ott_runtime) fn choice_value(self, index: u64) -> Result<Value, &'static str> {
        let index = usize::try_from(index).map_err(|_| "Invalid subtitle choice")?;
        match self {
            Self::Mode => ["off", "auto", "forced"].get(index).map(|value| json!(value)),
            Self::Language => LANGUAGES.get(index).map(|(code, _)| json!(code)),
        }.ok_or("Invalid subtitle choice")
    }
    pub(in crate::app::ott_runtime) fn candidate(self, saved: &Saved, value: &Value) -> Result<Saved, &'static str> {
        if !self.valid(value) { return Err("Invalid native subtitle selection preference"); }
        read(saved)?;
        let previous = revision(saved)?;
        if saved.settings.get(self.id()) == Some(value) { return Ok(saved.clone()); }
        let next = previous.checked_add(1).ok_or("Native subtitle revision exhausted")?;
        let mut candidate = saved.clone();
        candidate.settings.insert(self.id().into(), value.clone());
        candidate.settings.insert(REVISION.into(), json!(next));
        Ok(candidate)
    }
    pub(in crate::app::ott_runtime) fn descriptor(self) -> Value {
        let mut descriptor = json!({"id":self.id(),"type":"string","scope":"application","sensitive":false,
            "access":{"read":true,"write":true},"default_value":self.default_value(),"apply":"immediate",
            "description":"Native HLS subtitle selection. Automatic selections update after saving. Manual track or Off choices take precedence until the next logical playback. Empty language follows manifest defaults. Plex retains its own preferences."});
        match self {
            Self::Mode => {
                descriptor["constraints"] = json!({"values":["off","auto","forced"]});
                descriptor["choices"] = json!([{"value":"off","label":"Off (manual selection)","aliases":[]},
                    {"value":"auto","label":"Automatic","aliases":[]}, {"value":"forced","label":"Forced only","aliases":[]}]);
            }
            Self::Language => { descriptor["constraints"] = json!({"max_utf8_bytes":35}); }
        }
        descriptor
    }
}

fn valid_language(language: &str) -> bool {
    if language.is_empty() { return true; }
    if language.len() > 35 { return false; }
    let mut parts = language.split('-');
    let first = parts.next().unwrap_or("");
    (2..=3).contains(&first.len()) && first.bytes().all(|b| b.is_ascii_alphabetic())
        && parts.all(|part| !part.is_empty() && part.len() <= 8 && part.bytes().all(|b| b.is_ascii_alphanumeric()))
}

pub(in crate::app::ott_runtime) fn read(saved: &Saved) -> Result<HlsSubtitlePolicy, &'static str> {
    let mode = saved.settings.get(MODE).cloned().unwrap_or_else(|| Field::Mode.default_value());
    let language = saved.settings.get(LANGUAGE).cloned().unwrap_or_else(|| Field::Language.default_value());
    if !Field::Mode.valid(&mode) || !Field::Language.valid(&language) { return Err("Invalid native subtitle selection preference"); }
    Ok(HlsSubtitlePolicy { mode: match mode.as_str().unwrap() {
        "auto" => HlsSubtitleMode::Auto, "forced" => HlsSubtitleMode::Forced, _ => HlsSubtitleMode::Off,
    }, language: language.as_str().unwrap().to_owned() })
}

#[cfg(test)]
#[path = "ott_subtitle_selection_tests.rs"]
mod tests;
