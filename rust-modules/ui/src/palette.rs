//! App-scoped interface colors. Parsing, editing and publication share one wire format.
//! An empty document inherits the existing theme; media/caption colors are separate.
use std::cell::Cell;

pub const KEY: &str = "uiColorConfig";
pub const MAX_BYTES: usize = 2048;
const INVALID: &str = "Use a canonical interface color configuration";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Role { Background, Surface, Text, Muted, Accent, Selection, SelectionText, Border, EpgTime }
impl Role {
    pub const ALL: [Self; 9] = [Self::Background, Self::Surface, Self::Text, Self::Muted,
        Self::Accent, Self::Selection, Self::SelectionText, Self::Border, Self::EpgTime];
    pub fn id(self) -> &'static str { ["background", "surface", "text", "muted", "accent",
        "selection", "selectionText", "border", "epgTime"][self as usize] }
    pub fn title(self) -> &'static str { ["Background", "Panel background", "Text", "Secondary text",
        "Accent", "Selected item", "Selected item text", "Borders", "EPG date and time"][self as usize] }
    pub fn find(id: &str) -> Option<Self> { Self::ALL.into_iter().find(|role| role.id() == id) }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Config([Option<u32>; 9]);
impl Config {
    pub fn decode(text: &str) -> Result<Self, &'static str> {
        if text.is_empty() { return Ok(Self::default()); }
        if text.len() > MAX_BYTES { return Err(INVALID); }
        let value: serde_json::Value = serde_json::from_str(text).map_err(|_| INVALID)?;
        let object = value.as_object().ok_or(INVALID)?;
        if object.len() != 2 || value["version"].as_f64() != Some(1.0) { return Err(INVALID); }
        let colors = value["colors"].as_object().ok_or(INVALID)?;
        let mut config = Self::default();
        for (id, value) in colors {
            let role = Role::find(id).ok_or(INVALID)?;
            config.0[role as usize] = Some(parse_hex(&value.as_str().ok_or(INVALID)?.to_ascii_uppercase())?);
        }
        Ok(config)
    }
    /// Canonical form is required for remote writes; reads never rewrite stored bytes.
    pub fn parse(text: &str) -> Result<Self, &'static str> {
        let config = Self::decode(text)?;
        // Reject duplicate keys, extra whitespace, key reordering, lowercase hex,
        // alternate numbers and an empty colors object. The UI always emits this form.
        if config.encode() != text { return Err(INVALID); }
        Ok(config)
    }
    pub fn encode(self) -> String {
        let entries: Vec<_> = Role::ALL.into_iter().filter_map(|role|
            self.hex(role).map(|hex| format!("\"{}\":\"{hex}\"", role.id()))).collect();
        if entries.is_empty() { String::new() }
        else { format!("{{\"version\":1,\"colors\":{{{}}}}}", entries.join(",")) }
    }
    pub fn contains(self, role: Role) -> bool { self.0[role as usize].is_some() }
    pub fn hex(self, role: Role) -> Option<String> { self.0[role as usize].map(|rgb| format!("#{rgb:06X}")) }
    pub fn with(mut self, role: Role, hex: Option<&str>) -> Result<Self, &'static str> {
        self.0[role as usize] = hex.map(parse_hex).transpose()?;
        Ok(self)
    }
    pub fn resolve(self, role: Role, fallback: [f32; 4]) -> [f32; 4] {
        self.0[role as usize].map_or(fallback, |rgb|
            [((rgb >> 16) & 255) as f32 / 255.0, ((rgb >> 8) & 255) as f32 / 255.0,
                (rgb & 255) as f32 / 255.0, fallback[3]])
    }
    pub fn preset(id: &str) -> Option<Self> {
        let colors = match id {
            "inherit" => return Some(Self::default()),
            "midnight" => [0x080C12,0x111B2B,0xF1F5F9,0x94A3B8,0x67E8F9,0x164E63,0xFFFFFF,0x334155,0x67E8F9],
            "ocean" => [0x061826,0x0C293D,0xEFFAFF,0xA3C4D5,0x5EEAD4,0x155E75,0xFFFFFF,0x3B6C83,0x5EEAD4],
            "warm-graphite" => [0x171412,0x29231F,0xFFF7ED,0xC9B9A7,0xFDBA74,0x78350F,0xFFFFFF,0x685649,0xFDE68A],
            "high-contrast" => [0x000000,0x000000,0xFFFFFF,0xE5E5E5,0xFFFF00,0xFFFF00,0x000000,0xFFFFFF,0xFFFF00],
            "light" => [0xF5F7FA,0xFFFFFF,0x111827,0x475569,0x1D4ED8,0xDBEAFE,0x102A56,0x64748B,0x1D4ED8],
            _ => return None,
        };
        Some(Self(colors.map(Some)))
    }
}
fn parse_hex(text: &str) -> Result<u32, &'static str> {
    if text.len() != 7 || !text.starts_with('#')
        || !text.as_bytes()[1..].iter().all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(b)) {
        return Err(INVALID);
    }
    u32::from_str_radix(&text[1..],16).map_err(|_| INVALID)
}

// Rendering and accepted publication run on the UI thread. A complete value is
// replaced once, so a frame cannot observe a mixture of old and new roles.
thread_local! { static PROJECTED: Cell<Config> = const { Cell::new(Config([None; 9])) }; }
pub fn current() -> Config { PROJECTED.with(Cell::get) }
pub fn project(config: Config) -> bool {
    let changed = PROJECTED.with(|value| value.replace(config) != config);
    if changed { plx_machine::idle::invalidate(); }
    changed
}
pub fn color(role: Role, fallback: [f32; 4]) -> [f32; 4] { current().resolve(role, fallback) }

pub const SWATCHES: [&str; 32] = ["#FFFFFF", "#E2E8F0", "#94A3B8", "#475569", "#1E293B", "#0F172A", "#080C12", "#000000", "#FEE2E2", "#FCA5A5", "#EF4444", "#FB923C", "#FBBF24", "#FDE047", "#BEF264", "#22C55E", "#A7F3D0", "#2DD4BF", "#22D3EE", "#38BDF8", "#60A5FA", "#818CF8", "#A78BFA", "#C084FC", "#F9A8D4", "#F472B6", "#E879F9", "#7C3AED", "#1D4ED8", "#0E7490", "#166534", "#78350F"];

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_roundtrip_and_partial_inheritance() {
        let input = r##"{"version":1,"colors":{"background":"#010203","epgTime":"#ABCDEF"}}"##;
        let config = Config::parse(input).unwrap();
        assert_eq!(config.encode(), input);
        assert_eq!(config.hex(Role::Text), None);
        assert_eq!(config.resolve(Role::Text, [0.2,0.3,0.4,0.5]), [0.2,0.3,0.4,0.5]);
        assert_eq!(config.resolve(Role::Background, [0.0,0.0,0.0,0.25]), [1.0/255.0,2.0/255.0,3.0/255.0,0.25]);
        assert_eq!(Config::default().encode(), "");
        for id in ["inherit","midnight","ocean","warm-graphite","high-contrast","light"] {
            let preset = Config::preset(id).unwrap();
            assert_eq!(Config::parse(&preset.encode()), Ok(preset));
        }
    }
    #[test]
    fn malformed_or_noncanonical_documents_are_rejected() {
        for input in [r##"{"version":1,"colors":{}}"##, r##"{"colors":{"text":"#FFFFFF"},"version":1}"##,
            r##"{"version":1,"colors":{"text":"#ffffff"}}"##,
            r##"{"version":1,"colors":{"text":"#FFFFFF","text":"#000000"}}"##,
            r##"{"version":1,"colors":{"unknown":"#FFFFFF"}}"##,
            r##"{"version":1,"colors":{"text":"#FFFFFFFF"}}"##,
            r##"{"version":1,"colors":{"text":"red"}}"##,
            r##"{"version":1,"colors":{"text":null}}"##,
            r##"{"version":2,"colors":{"text":"#FFFFFF"}}"##,
            r##"{"version":1,"colors":{"epgTime":"#FFFFFF","text":"#000000"}}"##,
            " ", "null"] { assert!(Config::parse(input).is_err(), "{input}"); }
        assert!(Config::parse(&" ".repeat(MAX_BYTES+1)).is_err());
    }
    #[test]
    fn read_normalizes_safe_legacy_bytes_but_write_requires_canonical_form() {
        let raw = r##"{ "colors": {"epgTime":"#abcdef", "text":"#123456"}, "version":1.0 }"##;
        let decoded = Config::decode(raw).unwrap();
        assert_eq!(decoded.encode(), r##"{"version":1,"colors":{"text":"#123456","epgTime":"#ABCDEF"}}"##);
        assert!(Config::parse(raw).is_err());
        assert_eq!(Config::decode(r#"{"version":1,"colors":{}}"#).unwrap().encode(), "");
        for invalid in [r#"{"version":1,"colors":{},"extra":1}"#, r#"{"version":1,"colors":{"text":"url(x)"}}"#,
            r##"{"version":1,"colors":{"other":"#123456"}}"##, r#"{"version":true,"colors":{}}"#] {
            assert!(Config::decode(invalid).is_err());
        }
    }
    #[test]
    fn publication_is_atomic_and_reset_preserves_the_original_look() {
        let previous = current();
        let config = Config::default().with(Role::Accent,Some("#123456")).unwrap();
        project(config);
        assert_eq!(current(),config);
        assert!(!project(config));
        assert!(Config::parse("bad").is_err());
        assert_eq!(current(),config);
        project(Config::default());
        assert_eq!(color(Role::Accent,[0.1,0.2,0.3,0.4]),[0.1,0.2,0.3,0.4]);
        project(previous);
    }
}
