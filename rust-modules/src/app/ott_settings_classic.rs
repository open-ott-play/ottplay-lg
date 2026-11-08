//! Typed Classic preferences share remote/local validation and accepted publication.
use super::Saved;
use plx_appkit::classic_settings::{ArrowFunction, ListPosition, PageSize, Preferences, Seek13Duration};
use serde_json::{json, Value};

#[derive(Clone, Copy)]
pub(super) enum Field { PageSize, ListPosition, Seek13Duration, ArrowFun, ShowNumber, ShowName, ShowProgram, ShowDescription, ShowProgress }
impl Field {
    pub(super) const ALL: [Self; 9] = [Self::PageSize, Self::ListPosition, Self::Seek13Duration, Self::ArrowFun,
        Self::ShowNumber, Self::ShowName, Self::ShowProgram, Self::ShowDescription, Self::ShowProgress];
    pub(super) fn id(self) -> &'static str {
        match self {
            Self::PageSize => "pageSize", Self::ListPosition => "listPosition",
            Self::Seek13Duration => "seek13Duration", Self::ArrowFun => "arrowFun",
            Self::ShowNumber => "showNumber", Self::ShowName => "showName",
            Self::ShowProgram => "showProgram", Self::ShowDescription => "showDescription",
            Self::ShowProgress => "showProgress",
        }
    }
    pub(super) fn provider_scoped(self) -> bool {
        matches!(self, Self::ShowNumber | Self::ShowName | Self::ShowProgram | Self::ShowDescription | Self::ShowProgress)
    }
    pub(super) fn storage_key(self, provider: &str) -> String {
        if self.provider_scoped() { format!("provider:{provider}:{}", self.id()) }
        else { self.id().into() }
    }
    pub(super) fn default_value(self) -> Value { self.value(Preferences::default()) }
    pub(super) fn value(self, preferences: Preferences) -> Value {
        match self {
            Self::PageSize => json!(preferences.page_size()),
            Self::ListPosition => json!(u8::from(preferences.list_left())),
            Self::Seek13Duration => json!(preferences.seek13_seconds()),
            Self::ArrowFun => json!(preferences.arrow_mode()),
            Self::ShowNumber => json!(u8::from(preferences.show_number())),
            Self::ShowName => json!(u8::from(preferences.show_name())),
            Self::ShowProgram => json!(u8::from(preferences.show_program())),
            Self::ShowDescription => json!(u8::from(preferences.show_description())),
            Self::ShowProgress => json!(u8::from(preferences.show_progress())),
        }
    }
    pub(super) fn valid(self, value: &Value) -> bool {
        let Some(number) = value.as_u64() else { return false; };
        match self {
            Self::PageSize => usize::try_from(number).ok().and_then(PageSize::from_rows).is_some(),
            Self::ListPosition => u8::try_from(number).ok().and_then(ListPosition::from_index).is_some(),
            Self::Seek13Duration => u32::try_from(number).ok().and_then(Seek13Duration::from_seconds).is_some(),
            Self::ArrowFun => u8::try_from(number).ok().and_then(ArrowFunction::from_index).is_some(),
            Self::ShowNumber | Self::ShowName | Self::ShowProgram | Self::ShowDescription | Self::ShowProgress => number <= 1,
        }
    }
    pub(super) fn parse_text(self, text: &str) -> Result<Value, &'static str> {
        let error = match self {
            Self::PageSize => "Enter an integer from 10 to 30",
            Self::ListPosition | Self::ShowNumber | Self::ShowName | Self::ShowProgram | Self::ShowDescription | Self::ShowProgress => "Enter an integer from 0 to 1",
            Self::Seek13Duration => "Select a supported numeric seek duration",
            Self::ArrowFun => "Enter an integer from 0 to 3",
        };
        let text = text.trim();
        // Four decimal digits cover the largest supported value, 3600 seconds.
        if text.is_empty() || text.len() > 4 || !text.bytes().all(|b| b.is_ascii_digit()) { return Err(error); }
        let value = json!(text.parse::<u64>().map_err(|_| error)?);
        if self.valid(&value) { Ok(value) } else { Err(error) }
    }
    pub(super) fn descriptor(self) -> Value {
        let mut result = json!({"id":self.id(),"type":"integer","scope":"application","sensitive":false,
            "access":{"read":true,"write":true},"default_value":self.default_value(),"apply":"immediate"});
        if self.provider_scoped() { result["scope"] = json!("provider"); }
        match self {
            Self::ShowNumber | Self::ShowName | Self::ShowProgram | Self::ShowDescription | Self::ShowProgress => {
                result["constraints"] = json!({"values":[0,1]});
                result["choices"] = json!([{"value":0,"label":"Off","aliases":[]},{"value":1,"label":"On","aliases":[]}]);
                result["description"] = json!(match self {
                    Self::ShowNumber => "Show the existing item number in Classic channel lists.",
                    Self::ShowName => "Show channel and file names in Classic channel lists.",
                    Self::ShowProgram => "Show available current-programme titles in Classic channel lists.",
                    Self::ShowProgress => "Show current-programme progress in Classic channel lists.",
                    _ => "Show synopsis text beside the Classic channel list; the full description remains available.",
                });
            }
            Self::PageSize => {
                result["constraints"] = json!({"minimum":10,"maximum":30});
                result["description"] = json!("Preferred Classic list rows per page, reduced when text does not fit.");
            }
            Self::ListPosition => {
                result["constraints"] = json!({"values":[0,1]});
                result["choices"] = json!([{"value":0,"label":"Right","aliases":[]},{"value":1,"label":"Left","aliases":[]}]);
                result["description"] = json!("Classic catalogue list position.");
            }
            Self::Seek13Duration => {
                result["constraints"] = json!({"values":Seek13Duration::CHOICES});
                result["unit"] = json!("seconds");
                result["description"] = json!("Numeric 1/3 seek step for eligible finite and archive playback; does not change arrow-key seeking.");
            }
            Self::ArrowFun => {
                result["constraints"] = json!({"values":[0,1,2,3]});
                result["choices"] = json!([{"value":0,"label":"Paging","aliases":["paging","neutrino"]},
                    {"value":1,"label":"Volume","aliases":[]},{"value":2,"label":"Dune/PHP","aliases":[]}]);
                result["description"] = json!("Classic list left/right behavior. Stored legacy value 3 is preserved as a Paging alias; fullscreen arrow behavior is unchanged.");
            }
        }
        result
    }
}

pub(super) fn preferences(saved: &Saved) -> Preferences {
    let page = saved.settings.get(Field::PageSize.id()).and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok()).and_then(PageSize::from_rows).unwrap_or_default();
    let position = saved.settings.get(Field::ListPosition.id()).and_then(Value::as_u64)
        .and_then(|n| u8::try_from(n).ok()).and_then(ListPosition::from_index).unwrap_or_default();
    let seek = saved.settings.get(Field::Seek13Duration.id()).and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok()).and_then(Seek13Duration::from_seconds).unwrap_or_default();
    let arrows = saved.settings.get(Field::ArrowFun.id()).and_then(Value::as_u64)
        .and_then(|n| u8::try_from(n).ok()).and_then(ArrowFunction::from_index).unwrap_or_default();
    Preferences::from_values(page.rows(), position.index(), arrows.index(), seek.seconds())
        .expect("each Classic preference is checked above")
}

/// Provider preferences intentionally span profile slots, matching Classic M3U
/// and Stalker. Only accepted Saved data is passed here; editor drafts stay out.
pub(super) fn with_provider_display(preferences: Preferences, saved: &Saved, provider: &str) -> Preferences {
    let visible = |field: Field| saved.settings.get(&field.storage_key(provider))
        .and_then(Value::as_u64).filter(|value| *value <= 1).unwrap_or(1) as u8;
    preferences.with_channel_display(visible(Field::ShowNumber), visible(Field::ShowName),
        visible(Field::ShowProgram), visible(Field::ShowDescription))
        .and_then(|preferences| preferences.with_show_progress(visible(Field::ShowProgress)))
        .expect("each visibility preference is checked above")
}
