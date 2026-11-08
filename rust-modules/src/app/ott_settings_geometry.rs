//! Typed aliases for the existing native external-source subtitle preferences.
//! Plex retains its own profile geometry; no new settings store is introduced.
use super::super::super::subtitle_settings::Edit;
use plx_media::route::NativeSubtitlePreferences;
use plx_plex::plex::session::{SubtitlePosition, SubtitleSize, SubtitleTone};
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Field {
    Size,
    Position,
    Tone,
}

impl Field {
    pub(super) fn id(self) -> &'static str {
        match self {
            Self::Size => "nativeSubtitleSize",
            Self::Position => "nativeSubtitlePosition",
            Self::Tone => "nativeSubtitleTone",
        }
    }

    pub(super) fn default_value(self) -> Value {
        match self {
            Self::Size => json!(SubtitleSize::default()),
            Self::Position => json!(SubtitlePosition::default()),
            Self::Tone => json!(SubtitleTone::default()),
        }
    }

    pub(super) fn value(self, preferences: NativeSubtitlePreferences) -> Value {
        match self {
            Self::Size => json!(preferences.size),
            Self::Position => json!(preferences.position),
            Self::Tone => json!(preferences.tone),
        }
    }

    pub(super) fn edit(self, value: &Value) -> Result<Edit, ()> {
        match self {
            Self::Size => serde_json::from_value(value.clone()).map(Edit::Size),
            Self::Position => serde_json::from_value(value.clone()).map(Edit::Position),
            Self::Tone => serde_json::from_value(value.clone()).map(Edit::Tone),
        }.map_err(|_| ())
    }

    pub(super) fn descriptor(self) -> Value {
        let values: Vec<(Value, &str)> = match self {
            Self::Size => SubtitleSize::LADDER.into_iter()
                .zip(["Small", "Medium", "Large", "Extra large", "Huge"])
                .map(|(value, label)| (json!(value), label)).collect(),
            Self::Position => SubtitlePosition::LADDER.into_iter()
                .zip(["Low", "Middle", "High"])
                .map(|(value, label)| (json!(value), label)).collect(),
            Self::Tone => SubtitleTone::LADDER.into_iter()
                .map(|value| (json!(value), value.label())).collect(),
        };
        let choices: Vec<_> = values.into_iter().map(|(value, label)|
            json!({"value":value,"label":label,"aliases":[value]})).collect();
        let values: Vec<_> = choices.iter().map(|choice| choice["value"].clone()).collect();
        json!({"id":self.id(),"type":"string","scope":"application","sensitive":false,
            "access":{"read":true,"write":true},"default_value":self.default_value(),
            "constraints":{"values":values,"max_utf8_bytes":32},"choices":choices,"apply":"immediate",
            "description":"Native external-source subtitle preference. Missing values inherit playback preferences; effective_value is omitted without an external source. Plex retains its profile preferences. Saving does not select a track, restart playback or wake the player. Image and styled subtitles retain their renderer-specific geometry."})
    }
}
