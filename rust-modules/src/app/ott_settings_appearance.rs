//! Pure typed projection of the existing installation-wide text appearance.
//! Native tokens have their own IDs; Classic integer indexes keep their meaning.
use plx_media::player::text_style::{
    TextAppearance, TextBackground, TextColor, TextFont, TextOutline,
};
use serde_json::{json, Value};

pub(super) const STORAGE_KEY: &str = "nativeSubtitleAppearance";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Field {
    Font,
    Color,
    Outline,
    Background,
}

impl Field {
    pub(super) const ALL: [Self; 4] = [Self::Font, Self::Color, Self::Outline, Self::Background];

    pub(super) fn id(self) -> &'static str {
        match self {
            Self::Font => "nativeSubtitleFont",
            Self::Color => "nativeSubtitleColor",
            Self::Outline => "nativeSubtitleOutline",
            Self::Background => "nativeSubtitleBackground",
        }
    }

    fn member(self) -> &'static str {
        match self {
            Self::Font => "font",
            Self::Color => "color",
            Self::Outline => "outline",
            Self::Background => "background",
        }
    }

    pub(super) fn value(self, appearance: TextAppearance) -> Value {
        match self {
            Self::Font => json!(appearance.font),
            Self::Color => json!(appearance.color),
            Self::Outline => json!(appearance.outline),
            Self::Background => json!(appearance.background),
        }
    }

    pub(super) fn default_value(self) -> Value {
        self.value(TextAppearance::default())
    }

    /// Return a new typed value; no settings map or player is mutated here.
    pub(super) fn merge(
        self,
        mut current: TextAppearance,
        value: &Value,
    ) -> Result<TextAppearance, ()> {
        match self {
            Self::Font => current.font = serde_json::from_value(value.clone()).map_err(|_| ())?,
            Self::Color => current.color = serde_json::from_value(value.clone()).map_err(|_| ())?,
            Self::Outline => {
                current.outline = serde_json::from_value(value.clone()).map_err(|_| ())?
            }
            Self::Background => {
                current.background = serde_json::from_value(value.clone()).map_err(|_| ())?
            }
        }
        Ok(current)
    }

    fn choices(self) -> Vec<Value> {
        let values: Vec<(Value, &str)> = match self {
            Self::Font => TextFont::LADDER
                .into_iter()
                .zip(["Inter Bold", "Inter Regular"])
                .map(|(value, label)| (json!(value), label))
                .collect(),
            Self::Color => TextColor::LADDER
                .into_iter()
                .zip(["White", "Yellow", "Green", "Cyan", "Blue", "Magenta", "Red"])
                .map(|(value, label)| (json!(value), label))
                .collect(),
            Self::Outline => TextOutline::LADDER
                .into_iter()
                .zip(["None", "Thin", "Normal", "Thick", "Extra thick"])
                .map(|(value, label)| (json!(value), label))
                .collect(),
            Self::Background => TextBackground::LADDER
                .into_iter()
                .zip([
                    "Transparent",
                    "25% opacity",
                    "50% opacity",
                    "75% opacity",
                    "Opaque",
                ])
                .map(|(value, label)| (json!(value), label))
                .collect(),
        };
        values
            .into_iter()
            .map(|(value, label)| {
                json!({
                    "value":value,"label":label,"aliases":[value]
                })
            })
            .collect()
    }

    pub(super) fn descriptor(self) -> Value {
        let choices = self.choices();
        let values: Vec<Value> = choices
            .iter()
            .map(|choice| choice["value"].clone())
            .collect();
        json!({"id":self.id(),"type":"string","scope":"application","sensitive":false,
            "access":{"read":true,"write":true},"default_value":self.default_value(),
            "constraints":{"values":values,"max_utf8_bytes":32},"choices":choices,"apply":"immediate",
            "description":"Installation-wide native text subtitle appearance. Saving does not select a track, restart playback or wake the player. Image subtitles keep their original artwork."})
    }

    pub(super) fn snapshot(self, raw: Option<&Value>) -> Value {
        let parsed = raw.map(|value| serde_json::from_value::<TextAppearance>(value.clone()));
        let value = parsed
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .map(|appearance| self.value(*appearance))
            .unwrap_or_else(|| self.default_value());
        let invalid = parsed.as_ref().is_some_and(Result::is_err);
        let present = !invalid && raw.is_some_and(|value| value.get(self.member()).is_some());
        let mut result = json!({"id":self.id(),"value":value,
            "storage":if invalid {"invalid"} else if present {"stored"} else {"default"}});
        if present {
            result["persisted_value"] = value;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptors_match_the_real_cli_conformance_fixture() {
        let fixture: Value =
            serde_json::from_str(include_str!("fixtures/ott-settings-appearance.json")).unwrap();
        let descriptors: Vec<_> = Field::ALL.into_iter().map(Field::descriptor).collect();
        assert_eq!(json!(descriptors), fixture["settings"]);
    }

    #[test]
    fn every_advertised_token_merges_only_its_own_typed_field() {
        let original = TextAppearance {
            font: TextFont::InterRegular,
            color: TextColor::Cyan,
            outline: TextOutline::Thick,
            background: TextBackground::Half,
        };
        for field in Field::ALL {
            let descriptor = field.descriptor();
            assert_eq!(descriptor["type"], "string");
            assert_eq!(descriptor["access"]["write"], true);
            for choice in descriptor["choices"].as_array().unwrap() {
                let merged = field.merge(original, &choice["value"]).unwrap();
                assert_eq!(field.value(merged), choice["value"]);
                assert_eq!(choice["aliases"], json!([choice["value"]]));
                for other in Field::ALL {
                    if other != field {
                        assert_eq!(other.value(merged), other.value(original));
                    }
                }
                let roundtrip: TextAppearance = serde_json::from_value(json!(merged)).unwrap();
                assert_eq!(roundtrip, merged);
            }
        }
    }

    #[test]
    fn defaults_missing_members_and_explicit_defaults_are_distinct() {
        for field in Field::ALL {
            let absent = field.snapshot(None);
            assert_eq!(absent["storage"], "default");
            assert_eq!(absent["value"], field.default_value());
            assert!(absent.get("persisted_value").is_none());
            let stored = field.snapshot(Some(&json!(TextAppearance::default())));
            assert_eq!(stored["storage"], "stored");
            assert_eq!(stored["persisted_value"], field.default_value());
        }
        let partial = json!({"font":"inter_regular"});
        assert_eq!(Field::Color.snapshot(Some(&partial))["storage"], "default");
        assert_eq!(
            Field::Font.snapshot(Some(&partial))["persisted_value"],
            "inter_regular"
        );
    }

    #[test]
    fn invalid_values_never_mutate_current_or_escape_as_stored_metadata() {
        let current = TextAppearance::default();
        for field in Field::ALL {
            for invalid in [
                json!(null),
                json!(0),
                json!(true),
                json!([]),
                json!({}),
                json!("unknown"),
                json!("White"),
                json!("white\n[Events]"),
            ] {
                assert_eq!(field.merge(current, &invalid), Err(()));
                assert_eq!(current, TextAppearance::default());
            }
            let snapshot = field.snapshot(Some(&json!({"unexpected":"private fixture"})));
            assert_eq!(snapshot["storage"], "invalid");
            assert!(snapshot.get("persisted_value").is_none());
            assert!(!snapshot.to_string().contains("private fixture"));
        }
    }

    #[test]
    fn classic_numeric_indexes_are_not_native_enum_aliases() {
        for field in Field::ALL {
            assert!(field.merge(TextAppearance::default(), &json!(1)).is_err());
            assert!(field.merge(TextAppearance::default(), &json!("1")).is_err());
        }
        assert!(Field::Font
            .merge(TextAppearance::default(), &json!("Arial"))
            .is_err());
        assert_eq!(Field::Outline.default_value(), "normal");
        assert_eq!(Field::Background.default_value(), "transparent");
    }
}
