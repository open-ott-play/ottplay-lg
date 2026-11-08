//! Typed application OSD settings. Parsing never publishes a draft or reads global state.
use super::Saved;
use plx_appkit::player_hud::model::{OsdOpacity, PermanentClock};
use serde_json::{json, Value};

#[derive(Clone, Copy)]
pub(super) enum Field { Opacity, Clock }

impl Field {
    pub(super) const ALL: [Self; 2] = [Self::Opacity, Self::Clock];
    pub(super) fn find(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|field| field.id() == id)
    }
    pub(super) fn id(self) -> &'static str {
        match self { Self::Opacity => "osdOpacity", Self::Clock => "permanentTime" }
    }
    pub(super) fn default_value(self) -> Value {
        self.value((OsdOpacity::default(), PermanentClock::default()))
    }
    pub(super) fn value(self, preferences: (OsdOpacity, PermanentClock)) -> Value {
        match self { Self::Opacity => json!(preferences.0.level()), Self::Clock => json!(preferences.1.index()) }
    }
    pub(super) fn valid(self, value: &Value) -> bool {
        let Some(index) = value.as_u64() else { return false; };
        match self {
            Self::Opacity => u8::try_from(index).ok().and_then(OsdOpacity::from_level).is_some(),
            Self::Clock => PermanentClock::from_index(index).is_some(),
        }
    }
    pub(super) fn parse_text(self, text: &str) -> Result<Value, &'static str> {
        let error = match self {
            Self::Opacity => "Enter an integer from 0 to 10",
            Self::Clock => "Enter an integer from 0 to 2",
        };
        let text = text.trim();
        if text.is_empty() || text.len() > 2 || !text.bytes().all(|b| b.is_ascii_digit()) { return Err(error); }
        let value = json!(text.parse::<u8>().map_err(|_| error)?);
        if self.valid(&value) { Ok(value) } else { Err(error) }
    }
    pub(super) fn descriptor(self) -> Value {
        let mut result = json!({"id":self.id(),"type":"integer","scope":"application","sensitive":false,
            "access":{"read":true,"write":true},"default_value":self.default_value(),
            "constraints":{"minimum":0,"maximum":match self { Self::Opacity=>10, Self::Clock=>2 }},
            "apply":"immediate"});
        match self {
            Self::Opacity => {
                result["description"] = json!("On-screen display background level. Level 7 preserves the native themed background; text and icons are unchanged. This is not a final-alpha percentage.");
            }
            Self::Clock => {
                result["choices"] = json!([{"value":0,"label":"Off","aliases":["off"]},
                    {"value":1,"label":"Background","aliases":["background"]},
                    {"value":2,"label":"Transparent","aliases":["transparent"]}]);
                result["description"] = json!("Persistent clock over active playback: off, with background, or transparent background.");
            }
        }
        result
    }
}

pub(super) fn preferences(saved: &Saved) -> (OsdOpacity, PermanentClock) {
    let opacity = saved.settings.get(Field::Opacity.id()).and_then(Value::as_u64)
        .and_then(|n| u8::try_from(n).ok()).and_then(OsdOpacity::from_level).unwrap_or_default();
    let clock = saved.settings.get(Field::Clock.id()).and_then(Value::as_u64)
        .and_then(PermanentClock::from_index).unwrap_or_default();
    (opacity, clock)
}
