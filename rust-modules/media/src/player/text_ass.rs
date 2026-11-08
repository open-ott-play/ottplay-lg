//! Text subtitles as ASS: the pure half of rendering every text track through libass (plan
//! `libass-text-subs` §1.3, §1.4, §1.6, §2.2).
//!
//! What lives here, all of it pure (no libass, no font, no clock, no I/O):
//!
//! - [`convert`]: one cue's text, in one of the [`TextDialect`]s, to the body of an ASS event plus
//!   the cue's numpad alignment. It ports the semantics of the `sub_text` the bespoke caption draw
//!   used (kept in `tests.rs` as the oracle) and then escapes the result for libass, which reads `\N`, `\n`, `\h`, `\{` and `\}` as
//!   escapes, so a literal backslash must never be allowed to form one by accident.
//! - [`header`]: the synthesized `[Script Info]`/`[V4+ Styles]` for the viewer's size and position
//!   ([`TextStyle`](super::text_style::TextStyle)), one style per numpad alignment so the position
//!   of a cue is a property of its Style field.
//! - [`script_from_srt`] / [`script_from_vtt`]: a whole sidecar file to ASS event lines, with the
//!   newest-wins clip applied ([`clip_newest_wins`]) and times written as `H:MM:SS.cc`.
//! - [`event_payload`]: the Matroska packet an embedded cue becomes (what `ass_source::push`
//!   takes).
//!
//! Both switches are wired: `ass_source::push_text` converts every embedded text cue here, and
//! `sidecar::decode` converts a sidecar file with [`script_from_srt`] / [`script_from_vtt`].

use super::text_style::{self, TextStyle};

/// The historic default family. The shipped Inter Bold answers to it (plan §3.2);
/// an explicit Regular pick names the bundled "Inter" face instead.
pub const STYLE_FONT: &str = "PlxSubs";

/// A cue's numpad alignment, 1-9 (2 is the default, bottom centre).
pub type Align = u8;

/// The alignment of a cue that asks for none.
pub const DEFAULT_ALIGN: Align = 2;

/// Where a cue sits vertically. The newest-wins clip only compares cues of one class, and the
/// screen lifts only the bottom class while the transport HUD is up (plan §2.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AlignClass {
    /// Numpad 1-3.
    Bottom,
    /// Numpad 4-6.
    Middle,
    /// Numpad 7-9.
    Top,
}

impl AlignClass {
    /// The class of a numpad alignment; anything outside 1-9 is the default, bottom.
    pub fn of_align(align: Align) -> Self {
        match align {
            4..=6 => Self::Middle,
            7..=9 => Self::Top,
            _ => Self::Bottom,
        }
    }

    /// The class of a Style field, as `Default`/`An1`..`An9` the header defines. Another name
    /// (an authored style) has no class.
    pub fn of_style(name: &str) -> Option<Self> {
        if name == "Default" {
            return Some(Self::Bottom);
        }
        let digit = name.strip_prefix("An")?;
        match digit.as_bytes() {
            [d @ b'1'..=b'9'] => Some(Self::of_align(d - b'0')),
            _ => None,
        }
    }
}

/// The name of the header style for a numpad alignment. `Default` is alignment 2.
pub fn style_name(align: Align) -> &'static str {
    match align {
        1 => "An1",
        3 => "An3",
        4 => "An4",
        5 => "An5",
        6 => "An6",
        7 => "An7",
        8 => "An8",
        9 => "An9",
        _ => "Default",
    }
}

/// The grammar a text track's cues are written in: what the cue text means beyond the common floor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextDialect {
    Srt,
    WebVtt,
    MovText,
    Sami,
    RealText,
    MicroDvd,
    Mpl2,
    /// Every other text codec: the common floor only.
    Plain,
}

impl TextDialect {
    /// The dialect of an FFmpeg subtitle codec name, or `None` for a codec that is not text
    /// (`ass`/`ssa` have a renderer of their own; PGS/VobSub/DVB are images).
    pub fn from_codec_name(name: &str) -> Option<Self> {
        Some(match name {
            "subrip" | "srt" => Self::Srt,
            "webvtt" => Self::WebVtt,
            "mov_text" => Self::MovText,
            "sami" => Self::Sami,
            "realtext" => Self::RealText,
            "microdvd" => Self::MicroDvd,
            "mpl2" => Self::Mpl2,
            "text" | "vplayer" | "pjs" | "jacosub" | "subviewer" | "subviewer1" | "stl" => Self::Plain,
            _ => return None,
        })
    }

    fn decodes_entities(self) -> bool {
        matches!(self, Self::WebVtt | Self::Sami | Self::RealText)
    }
}

/// A converted cue: the ASS event text and the numpad alignment it asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Converted {
    pub body: String,
    pub align: Align,
}

impl Converted {
    pub fn class(&self) -> AlignClass {
        AlignClass::of_align(self.align)
    }
}

/// Pure cooperative work accounting. The owner supplies cancellation/deadline policy.
struct ConversionWork<'a> {
    should_stop: &'a mut dyn FnMut() -> bool,
    since_check: usize,
}
impl<'a> ConversionWork<'a> {
    fn new(should_stop: &'a mut dyn FnMut() -> bool) -> Self {
        Self { should_stop, since_check: 255 }
    }
    fn step(&mut self, units: usize) -> Result<(), &'static str> {
        self.since_check = self.since_check.saturating_add(units);
        if self.since_check >= 256 {
            self.since_check = 0;
            if (self.should_stop)() { return Err("Subtitle conversion cancelled"); }
        }
        Ok(())
    }
    fn scan(&mut self, text: &str) -> Result<(), &'static str> {
        for chunk in text.as_bytes().chunks(256) { self.step(chunk.len())?; }
        Ok(())
    }
    fn check(&mut self) -> Result<(), &'static str> {
        if (self.should_stop)() { Err("Subtitle conversion cancelled") } else { Ok(()) }
    }
}

/// Convert one cue's text. `None` when nothing visible is left, so the caller drops the cue as the
/// bespoke draw skipped an empty one.
pub fn convert(text: &str, dialect: TextDialect) -> Option<Converted> {
    convert_aligned(text, dialect, None)
}

/// [`convert`] with the alignment a cue gets when its own text names none (WebVTT's `line:0`).
pub fn convert_aligned(text: &str, dialect: TextDialect, default_align: Option<Align>) -> Option<Converted> {
    let mut never_stop = || false;
    convert_checked(text, dialect, default_align, usize::MAX, &mut ConversionWork::new(&mut never_stop)).ok().flatten()
}
fn convert_checked(text: &str, dialect: TextDialect, default_align: Option<Align>,
    output_limit: usize, work: &mut ConversionWork<'_>) -> Result<Option<Converted>, &'static str>
{
    let parsed = Parser::run(text, dialect, work)?;
    let Some(pieces) = normalize(parsed.pieces, work)? else { return Ok(None) };
    let align = parsed.align.or(default_align).filter(|a| (1..=9).contains(a)).unwrap_or(DEFAULT_ALIGN);
    Ok(Some(Converted { body: render(&pieces, output_limit, work)?, align }))
}

/// The Matroska packet an embedded cue becomes: `ReadOrder,Layer,Style,Name,MarginL,MarginR,
/// MarginV,Effect,Text`. Every event margin is 0, so the style's own margins apply.
pub fn event_payload(read_order: u32, align: Align, body: &str) -> String {
    format!("{read_order},0,{},,0,0,0,,{body}", style_name(align))
}

/// How many characters of text a converted body shows: override blocks (`{...}`) count nothing, an
/// escape (`\N`, `\h`, `\{`, `\}`) counts as the one thing it stands for, and the ZWSP that keeps
/// a literal backslash apart from the next letter (see [`convert`]) counts nothing. This is the
/// `len=` of the `sub cue` log line: a length, never the dialogue.
pub fn visible_chars(body: &str) -> usize {
    let mut n = 0;
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => {
                for c in chars.by_ref() {
                    if c == '}' {
                        break;
                    }
                }
            }
            '\\' if matches!(chars.peek(), Some('N' | 'n' | 'h' | '{' | '}')) => {
                chars.next();
                n += 1;
            }
            '\u{200B}' => {}
            _ => n += 1,
        }
    }
    n
}

// ---- parsing ----------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Piece {
    Ch(char),
    /// A line break (`\N`).
    Break,
    /// A non-breaking space (`\h`).
    Nbsp,
    /// The inside of an override block, without braces: `\i1`.
    Tag(String),
}

struct Parsed {
    pieces: Vec<Piece>,
    /// The first `\anN` of the cue.
    align: Option<Align>,
}

struct Parser {
    dialect: TextDialect,
    out: Parsed,
    /// Inside a WebVTT `<rt>…</rt>`: ruby text is dropped.
    in_rt: bool,
    /// Nothing but a break or the start of the cue precedes the cursor on its line.
    line_start: bool,
    /// Italic until the end of this line: an MPL2 line that began with `/`, or MicroDVD `{y:i}`
    /// (uppercase `{Y:i}` covers the cue).
    line_italic: bool,
}

impl Parser {
    fn run(text: &str, dialect: TextDialect, work: &mut ConversionWork<'_>) -> Result<Parsed, &'static str> {
        let mut p = Parser {
            dialect,
            out: Parsed { pieces: Vec::new(), align: None },
            in_rt: false,
            line_start: true,
            line_italic: false,
        };
        let mut chars = Vec::new();
        for ch in text.chars() { work.step(1)?; chars.push(ch); }
        let mut no_closing_brace = false;
        let mut i = 0;
        while i < chars.len() {
            work.step(1)?;
            let c = chars[i];
            match c {
                '\r' => i += 1,
                '\n' => {
                    p.line_break();
                    i += 1;
                }
                '\\' => match chars.get(i + 1) {
                    Some('N') | Some('n') => {
                        p.line_break();
                        i += 2;
                    }
                    Some('h') => {
                        p.push(Piece::Nbsp);
                        i += 2;
                    }
                    _ => {
                        p.push(Piece::Ch('\\'));
                        i += 1;
                    }
                },
                '{' => {
                    let close = if no_closing_brace { None } else { find_closing(&chars, i + 1, '}', work)? };
                    match close {
                        Some(end) => {
                            let inner = collect_chars(&chars[i + 1..end], work)?;
                            p.block(&inner, work)?;
                            i = end + 1;
                        }
                        None => {
                            // No later opening brace can have a closing brace either.
                            no_closing_brace = true;
                            p.push(Piece::Ch('{'));
                            i += 1;
                        }
                    }
                }
                '<' => match find_closing(&chars, i + 1, '>', work)? {
                    Some(end) => {
                        let inner = collect_chars(&chars[i + 1..end], work)?;
                        p.tag(&inner, work)?;
                        i = end + 1;
                    }
                    // Unmatched: hides the rest of the cue, as the bespoke draw did in every dialect.
                    None => i = chars.len(),
                },
                '&' if dialect.decodes_entities() => {
                    let (piece, used) = entity(&chars[i..]);
                    p.push(piece);
                    i += used;
                }
                '|' if matches!(dialect, TextDialect::MicroDvd | TextDialect::Mpl2) => {
                    p.line_break();
                    i += 1;
                }
                '/' if dialect == TextDialect::Mpl2 && p.line_start => {
                    p.line_italic = true;
                    p.push(Piece::Tag("\\i1".into()));
                    i += 1;
                }
                _ => {
                    p.push(Piece::Ch(c));
                    i += 1;
                }
            }
        }
        Ok(p.out)
    }

    fn push(&mut self, piece: Piece) {
        if self.in_rt {
            return;
        }
        if !matches!(piece, Piece::Tag(_)) {
            self.line_start = false;
        }
        self.out.pieces.push(piece);
    }

    fn line_break(&mut self) {
        if self.line_italic {
            self.line_italic = false;
            self.push(Piece::Tag("\\i0".into()));
        }
        self.push(Piece::Break);
        if !self.in_rt {
            self.line_start = true;
        }
    }

    /// A matched `{…}` block: an override block, or (MicroDVD) a control code. Only the whitelisted
    /// tags survive, so a text track can never ask for animation or arbitrary placement.
    fn block(&mut self, inner: &str, work: &mut ConversionWork<'_>) -> Result<(), &'static str> {
        work.scan(inner)?;
        if self.dialect == TextDialect::MicroDvd {
            if let Some(rest) = inner.strip_prefix("y:").or_else(|| inner.strip_prefix("Y:")) {
                if rest.contains(['i', 'I']) {
                    self.line_italic = inner.starts_with('y');
                    self.push(Piece::Tag("\\i1".into()));
                }
                return Ok(());
            }
        }
        for tag in override_tags(inner, work)? {
            work.step(1)?;
            if let Some(digit) = tag.strip_prefix("an") {
                if let [d @ b'1'..=b'9'] = digit.as_bytes() {
                    self.out.align.get_or_insert(d - b'0');
                }
                continue;
            }
            if matches!(tag, "i0" | "i1" | "b0" | "b1" | "u0" | "u1" | "s0" | "s1") {
                self.push(Piece::Tag(format!("\\{tag}")));
            }
        }
        Ok(())
    }

    /// A matched `<…>`.
    fn tag(&mut self, inner: &str, work: &mut ConversionWork<'_>) -> Result<(), &'static str> {
        work.scan(inner)?;
        let inner = inner.trim();
        let closing = inner.starts_with('/');
        let body = inner.trim_start_matches('/').trim();
        let mut name = String::new();
        for ch in body.chars().take_while(|c| !c.is_whitespace() && *c != '/') {
            work.step(1)?;
            name.push(ch.to_ascii_lowercase());
        }
        match self.dialect {
            TextDialect::Srt | TextDialect::WebVtt => match name.as_str() {
                "i" | "b" | "u" => self.style_tag(&name, closing),
                "s" if self.dialect == TextDialect::Srt => self.style_tag(&name, closing),
                "font" if self.dialect == TextDialect::Srt => {
                    if closing {
                        self.push(Piece::Tag("\\c".into()));
                    } else if let Some(bgr) = font_color(body) {
                        self.push(Piece::Tag(format!("\\c&H{bgr}&")));
                    }
                }
                "rt" if self.dialect == TextDialect::WebVtt => self.in_rt = !closing,
                _ => {}
            },
            TextDialect::Sami | TextDialect::RealText => {
                if name == "br" {
                    self.line_break();
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn style_tag(&mut self, name: &str, closing: bool) {
        self.push(Piece::Tag(format!("\\{name}{}", if closing { 0 } else { 1 })));
    }
}

fn find_closing(chars: &[char], start: usize, end: char, work: &mut ConversionWork<'_>)
    -> Result<Option<usize>, &'static str>
{
    for (offset, ch) in chars[start..].iter().enumerate() {
        work.step(1)?;
        if *ch == end { return Ok(Some(start + offset)); }
    }
    Ok(None)
}
fn collect_chars(chars: &[char], work: &mut ConversionWork<'_>) -> Result<String, &'static str> {
    let mut out = String::new();
    for ch in chars { work.step(1)?; out.push(*ch); }
    Ok(out)
}

/// The tags of an override block, each without its leading backslash. A backslash inside
/// parentheses (`\t(\i1)`) belongs to the enclosing tag.
fn override_tags<'a>(inner: &'a str, work: &mut ConversionWork<'_>) -> Result<Vec<&'a str>, &'static str> {
    let mut tags = Vec::new();
    let mut depth = 0i32;
    let mut start: Option<usize> = None;
    for (at, c) in inner.char_indices() {
        work.step(1)?;
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            '\\' if depth <= 0 => {
                if let Some(s) = start {
                    tags.push(&inner[s..at]);
                }
                start = Some(at + 1);
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        tags.push(&inner[s..]);
    }
    let mut trimmed = Vec::with_capacity(tags.len());
    for tag in tags { work.step(1)?; trimmed.push(tag.trim()); }
    Ok(trimmed)
}

/// One `&name;` entity at the start of `chars`: the piece and how many characters it used. An
/// unknown or unterminated one is a literal `&`.
fn entity(chars: &[char]) -> (Piece, usize) {
    let end = chars.iter().take(8).position(|&c| c == ';');
    if let Some(end) = end {
        let name: String = chars[1..end].iter().collect();
        let piece = match name.as_str() {
            "amp" => Some(Piece::Ch('&')),
            "lt" => Some(Piece::Ch('<')),
            "gt" => Some(Piece::Ch('>')),
            "quot" => Some(Piece::Ch('"')),
            "apos" => Some(Piece::Ch('\'')),
            "nbsp" => Some(Piece::Nbsp),
            _ => None,
        };
        if let Some(piece) = piece {
            return (piece, end + 1);
        }
    }
    (Piece::Ch('&'), 1)
}

/// The `BBGGRR` of a `<font color=…>` attribute, if it names a colour.
fn font_color(body: &str) -> Option<String> {
    let lower = body.to_ascii_lowercase();
    let after = lower[lower.find("color")? + "color".len()..].trim_start();
    let after = after.strip_prefix('=')?.trim_start();
    let value = match after.chars().next()? {
        q @ ('"' | '\'') => after[1..].split(q).next()?,
        _ => after.split(|c: char| c.is_whitespace() || c == '>').next()?,
    };
    let value = value.trim();
    let hex = value.strip_prefix('#').unwrap_or(value);
    let rgb: u32 = if hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        u32::from_str_radix(hex, 16).ok()?
    } else if hex.len() == 3 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        let n = u32::from_str_radix(hex, 16).ok()?;
        let (r, g, b) = ((n >> 8) & 0xF, (n >> 4) & 0xF, n & 0xF);
        (r * 17) << 16 | (g * 17) << 8 | b * 17
    } else {
        named_color(value)?
    };
    Some(format!("{:02X}{:02X}{:02X}", rgb & 0xFF, (rgb >> 8) & 0xFF, rgb >> 16))
}

/// The HTML colour keywords SubRip files use, as `0xRRGGBB`.
fn named_color(name: &str) -> Option<u32> {
    Some(match name {
        "white" => 0xFFFFFF,
        "black" => 0x000000,
        "red" => 0xFF0000,
        "lime" | "green" => 0x00FF00,
        "blue" => 0x0000FF,
        "yellow" => 0xFFFF00,
        "cyan" | "aqua" => 0x00FFFF,
        "magenta" | "fuchsia" => 0xFF00FF,
        "gray" | "grey" => 0x808080,
        "silver" => 0xC0C0C0,
        "maroon" => 0x800000,
        "navy" => 0x000080,
        "olive" => 0x808000,
        "purple" => 0x800080,
        "teal" => 0x008080,
        "orange" => 0xFFA500,
        _ => return None,
    })
}

// ---- normalisation and rendering --------------------------------------------------------------

/// Trim every line, drop empty ones, and collapse runs of breaks, as `caption_wrap` did when it
/// skipped empty segments. Tags are never lost to the trimming: they wait and are emitted before
/// the next visible character. `None` when no visible character remains.
fn normalize(pieces: Vec<Piece>, work: &mut ConversionWork<'_>) -> Result<Option<Vec<Piece>>, &'static str> {
    let mut out: Vec<Piece> = Vec::with_capacity(pieces.len());
    let mut deferred: Vec<Piece> = Vec::new();
    let mut pending_break = false;
    let mut line_visible = false;
    let mut any_visible = false;
    let mut any_nbsp = false;
    for piece in pieces {
        work.step(1)?;
        match piece {
            Piece::Tag(_) => deferred.push(piece),
            Piece::Ch(c) if c.is_whitespace() => deferred.push(piece),
            Piece::Nbsp => {
                any_nbsp = true;
                deferred.push(piece);
            }
            Piece::Break => {
                if line_visible {
                    // The tags that came before the break belong to the line it ends.
                    for d in deferred.drain(..) {
                        work.step(1)?;
                        if matches!(d, Piece::Tag(_)) { out.push(d); }
                    }
                    pending_break = true;
                    line_visible = false;
                }
                let mut tags = Vec::new();
                for d in deferred.drain(..) {
                    work.step(1)?;
                    if matches!(d, Piece::Tag(_)) { tags.push(d); }
                }
                deferred = tags;
            }
            visible => {
                if pending_break {
                    out.push(Piece::Break);
                    pending_break = false;
                }
                for d in deferred.drain(..) {
                    work.step(1)?;
                    if matches!(d, Piece::Tag(_)) || line_visible {
                        out.push(d);
                    }
                }
                out.push(visible);
                line_visible = true;
                any_visible = true;
            }
        }
    }
    Ok(if any_visible {
        Some(out)
    } else {
        // A cue that is only hard spaces is still an event (SAMI's `&nbsp;` clear packets).
        any_nbsp.then(|| vec![Piece::Nbsp])
    })
}

/// Pieces to ASS text. A literal backslash that would form an escape with what follows it (`N`,
/// `n`, `h`, or the `{` of an override block) is followed by U+200B, which is zero-width, present
/// in Inter Bold, and only a break opportunity under libunibreak.
fn append_bounded(out: &mut String, text: &str, limit: usize) -> Result<(), &'static str> {
    if out.len().checked_add(text.len()).is_none_or(|len| len > limit) {
        return Err("Subtitle conversion exceeds output limit");
    }
    out.push_str(text);
    Ok(())
}
fn render(pieces: &[Piece], output_limit: usize, work: &mut ConversionWork<'_>) -> Result<String, &'static str> {
    let mut s = String::new();
    let mut i = 0;
    while i < pieces.len() {
        work.step(1)?;
        match &pieces[i] {
            Piece::Ch('{') => append_bounded(&mut s, "\\{", output_limit)?,
            Piece::Ch('}') => append_bounded(&mut s, "\\}", output_limit)?,
            Piece::Ch('\\') => {
                append_bounded(&mut s, "\\", output_limit)?;
                if matches!(pieces.get(i + 1), Some(Piece::Ch('N' | 'n' | 'h')) | Some(Piece::Tag(_))) {
                    append_bounded(&mut s, "\u{200B}", output_limit)?;
                }
            }
            Piece::Ch(c) => { let mut buf = [0; 4]; append_bounded(&mut s, c.encode_utf8(&mut buf), output_limit)?; }
            Piece::Break => append_bounded(&mut s, "\\N", output_limit)?,
            Piece::Nbsp => append_bounded(&mut s, "\\h", output_limit)?,
            Piece::Tag(_) => {
                append_bounded(&mut s, "{", output_limit)?;
                while let Some(Piece::Tag(t)) = pieces.get(i) {
                    work.step(1)?;
                    append_bounded(&mut s, t, output_limit)?;
                    i += 1;
                }
                append_bounded(&mut s, "}", output_limit)?;
                continue;
            }
        }
        i += 1;
    }
    Ok(s)
}

// ---- header -----------------------------------------------------------------------------------

/// The numpad alignments the header defines a style for, in order, with the style `MarginV`.
fn style_margins(style: &TextStyle) -> [(Align, u32); 9] {
    let bottom = text_style::bottom_margin_v(style.position);
    let top = text_style::TOP_MARGIN_V.round() as u32;
    [(2, bottom), (1, bottom), (3, bottom), (4, 0), (5, 0), (6, 0), (7, top), (8, top), (9, top)]
}

/// `51.49` or `40`: at most two decimals, no trailing zeros.
fn number(x: f64) -> String {
    let s = format!("{x:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The synthesized header of a text-origin script for the viewer's picks (plan §2.2): PlayRes
/// 1920x1080, `WrapStyle 1`, one style per numpad alignment, ending after the `[Events]` format
/// line so events can be appended.
pub fn header(style: &TextStyle) -> String {
    let scale = style.scale();
    let fs = number(text_style::font_size(scale));
    let margin = text_style::side_margin(scale);
    let appearance = style.appearance;
    let font = appearance.font.family();
    let color = appearance.color.ass_bgr();
    let outline = appearance.outline.pixels();
    let background_alpha = appearance.background.ass_alpha();
    // Pinned libass 0.17.5, ass_render.c::add_background / ass_render_event:
    // BorderStyle 4 draws BackColour behind the event while preserving the normal
    // glyph outline. BorderStyle 3 would replace that outline with an opaque box.
    // Transparent keeps the historical style byte-for-byte, including Shadow=0.
    let border_style = if appearance.background == text_style::TextBackground::Transparent { 1 } else { 4 };
    let mut h = String::from(
        "[Script Info]\nScriptType: v4.00+\nPlayResX: 1920\nPlayResY: 1080\nWrapStyle: 1\n\
         ScaledBorderAndShadow: yes\nYCbCr Matrix: None\n\n[V4+ Styles]\n\
         Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, \
         BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, \
         BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n",
    );
    for (align, margin_v) in style_margins(style) {
        h.push_str(&format!(
            "Style: {name},{font},{fs},&H00{color},&H00{color},&H26000000,&H{background_alpha:02X}000000,\
             0,0,0,0,100,100,0,0,{border_style},{outline},0,{align},{margin},{margin},{margin_v},-1\n",
            name = style_name(align),
        ));
    }
    h.push_str("\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n");
    h
}

// ---- newest wins ------------------------------------------------------------------------------

/// One cue as the clip sees it: a span in any one unit (ms for embedded cues, ns for a sidecar)
/// and the class it sits in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipCue {
    pub start: i64,
    pub end: i64,
    pub class: AlignClass,
    /// Arrival order (a Matroska ReadOrder, a sidecar's file index). Cues with one start are
    /// ordered by it, never by their place in the slice, so a caller may build the slice from a
    /// map ordered by end time and still get the newest cue.
    pub seq: u32,
}

/// The newest-wins view of a set of cues (D10): each cue's end is clipped to the start of the next
/// cue of the same class, so a 1-frame overlap, or an end defaulted to start + 4 s, no longer
/// leaves the old text up beside the new. Cues of different classes do not clip each other, so a
/// top sign over bottom dialogue both show.
///
/// The result has one entry per input cue, in input order: the clipped `(start, end)`, or `None`
/// for a cue that is dropped, never emitted, because its clipped duration is not positive (two
/// cues with one start: the one with the higher `seq` is the newer and wins) or it had none to begin
/// with. `cues` may be in any order; equal starts are ordered by `seq`.
pub fn clip_newest_wins(cues: &[ClipCue]) -> Vec<Option<(i64, i64)>> {
    let mut order: Vec<usize> = (0..cues.len()).filter(|&i| cues[i].end > cues[i].start).collect();
    order.sort_by_key(|&i| (cues[i].start, cues[i].seq, i));
    let mut next_start: [Option<i64>; 3] = [None; 3];
    let slot = |c: AlignClass| match c {
        AlignClass::Bottom => 0,
        AlignClass::Middle => 1,
        AlignClass::Top => 2,
    };
    let mut out = vec![None; cues.len()];
    for &i in order.iter().rev() {
        let cue = cues[i];
        let end = next_start[slot(cue.class)].map_or(cue.end, |s| cue.end.min(s));
        if end > cue.start {
            out[i] = Some((cue.start, end));
        }
        next_start[slot(cue.class)] = Some(cue.start);
    }
    out
}

// ---- sidecar files ----------------------------------------------------------------------------

/// `00:01:02,500 --> 00:01:05,000` (SubRip), `01:02.500 --> 01:05.000 line:0` (WebVTT).
pub fn timing_line(line: &str) -> Option<(i64, i64)> {
    timing_line_settings(line).map(|(start, end, _)| (start, end))
}

/// [`timing_line`] and the cue settings that follow the end time (empty for SubRip).
pub fn timing_line_settings(line: &str) -> Option<(i64, i64, &str)> {
    let (a, b) = line.split_once("-->")?;
    let start = timestamp(a.trim())?;
    let b = b.trim_start();
    let end_text = b.split_whitespace().next()?;
    let end = timestamp(end_text)?;
    Some((start, end, b[end_text.len()..].trim()))
}

/// `[H+:]MM:SS[,.]fff` → ns. The fraction is read as written (`.5` is half a second, `.50` too).
pub fn timestamp(s: &str) -> Option<i64> {
    let (clock, frac) = match s.rsplit_once([',', '.']) {
        Some((c, f)) => (c, f),
        None => (s, ""),
    };
    let mut secs: i64 = 0;
    let mut fields = 0;
    for part in clock.split(':') {
        let v: i64 = part.trim().parse().ok().filter(|v| *v >= 0)?;
        secs = secs.checked_mul(60)?.checked_add(v)?;
        fields += 1;
    }
    if !(2..=3).contains(&fields) {
        return None;
    }
    let mut frac_ns: i64 = 0;
    let mut scale: i64 = 100_000_000;
    for ch in frac.chars().take(9) {
        frac_ns += ch.to_digit(10)? as i64 * scale;
        scale /= 10;
    }
    secs.checked_mul(1_000_000_000)?.checked_add(frac_ns)
}

/// One cue of a sidecar file before conversion.
pub(crate) struct FileCue {
    pub(crate) id: Option<String>,
    pub(crate) start_ns: i64,
    pub(crate) end_ns: i64,
    pub(crate) settings: String,
    pub(crate) text: String,
}

/// Read the cues of a SubRip or WebVTT file: a timing line (one containing `-->`) opens a cue, its
/// text is every line up to the next blank line or timing line, and anything else outside a cue
/// (the counter line, `WEBVTT`, a cue identifier) is not text. A WebVTT NOTE, STYLE or REGION
/// block is skipped whole.
pub(crate) fn read_cues(text: &str) -> Vec<FileCue> {
    let mut never_stop = || false;
    read_cues_checked(text, &mut ConversionWork::new(&mut never_stop)).unwrap_or_default()
}
fn read_cues_checked(text: &str, work: &mut ConversionWork<'_>) -> Result<Vec<FileCue>, &'static str> {
    let text = text.trim_start_matches('\u{feff}');
    let mut out = Vec::new();
    let mut cur: Option<FileCue> = None;
    let mut pending_id = None;
    let mut skipping = false;
    for raw in text.split('\n') {
        work.scan(raw)?;
        work.step(1)?;
        let line = raw.trim_end_matches('\r');
        let blank = line.trim().is_empty();
        if blank {
            skipping = false;
            pending_id = None;
            out.extend(cur.take().filter(|c| !c.text.is_empty()));
            continue;
        }
        if skipping {
            continue;
        }
        if let Some((start_ns, end_ns, settings)) = timing_line_settings(line) {
            out.extend(cur.take().filter(|c| !c.text.is_empty()));
            cur = Some(FileCue { id: pending_id.take(), start_ns, end_ns, settings: settings.to_string(), text: String::new() });
        } else if let Some(c) = cur.as_mut() {
            if !c.text.is_empty() {
                c.text.push('\n');
            }
            c.text.push_str(line);
        } else if ["NOTE", "STYLE", "REGION"].iter().any(|k| line == *k || line.starts_with(&format!("{k} ")) || line.starts_with(&format!("{k}\t"))) {
            skipping = true;
            pending_id = None;
        } else {
            pending_id = Some(line.to_string());
        }
    }
    out.extend(cur.take().filter(|c| !c.text.is_empty()));
    Ok(out)
}

/// Whether WebVTT cue settings ask for the top of the frame: `line:0`, or a `line:` of 10 % or
/// less. A negative line counts from the bottom and an `align:` is not a placement, so neither is.
fn vtt_line_is_top(settings: &str) -> bool {
    let Some(value) = settings.split_whitespace().find_map(|t| t.strip_prefix("line:")) else {
        return false;
    };
    let value = value.split(',').next().unwrap_or("");
    match value.strip_suffix('%') {
        Some(percent) => percent.parse::<f64>().is_ok_and(|p| (0.0..=10.0).contains(&p)),
        None => value.parse::<f64>().is_ok_and(|n| n == 0.0),
    }
}

/// A sidecar file as ASS: the `Dialogue:` lines, without a header, so a change of the viewer's
/// size or position re-assembles the script without converting the file again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextScript {
    /// Zero or more `Dialogue:` lines, each ending in a newline, in start order.
    pub events: String,
    /// How many cues survived (empty, zero-length and clipped-away cues do not).
    pub cues: usize,
}

impl TextScript {
    /// The complete script for the viewer's picks.
    pub fn assemble(&self, style: &TextStyle) -> String {
        let mut script = header(style);
        script.push_str(&self.events);
        script
    }
}

/// A SubRip file as ASS events (already decoded to UTF-8).
pub fn script_from_srt(text: &str) -> TextScript {
    script_from(text, TextDialect::Srt)
}

/// A WebVTT file as ASS events (already decoded to UTF-8). `line:0` places a cue at the top.
pub fn script_from_vtt(text: &str) -> TextScript {
    script_from(text, TextDialect::WebVtt)
}

/// A validated, timeline-mapped HLS WebVTT track. Authored overlapping cues retain
/// their full intervals. The output budget includes every Dialogue header, escaped
/// body and newline; the caller reserves the separate ASS script header's bytes.
/// Cancellation/deadline policy is supplied by the owner, never read globally here.
pub fn script_from_vtt_timeline_bounded(text: &str, output_limit: usize,
    should_stop: &mut dyn FnMut() -> bool) -> Result<TextScript, &'static str>
{
    script_from_checked(text, TextDialect::WebVtt, false, output_limit, &mut ConversionWork::new(should_stop))
}

fn script_from(text: &str, dialect: TextDialect) -> TextScript {
    let mut never_stop = || false;
    script_from_checked(text, dialect, true, usize::MAX, &mut ConversionWork::new(&mut never_stop))
        .unwrap_or_else(|_| TextScript { events: String::new(), cues: 0 })
}
fn event_prefix(start_cs: i64, end_cs: i64, align: Align) -> String {
    format!("Dialogue: 0,{},{},{},,0,0,0,,", centis(start_cs), centis(end_cs), style_name(align))
}
fn script_from_checked(text: &str, dialect: TextDialect, clip: bool, output_limit: usize,
    work: &mut ConversionWork<'_>) -> Result<TextScript, &'static str>
{
    work.check()?;
    let mut cues = Vec::new();
    let mut converted = Vec::new();
    let mut reserved = 0usize;
    for cue in read_cues_checked(text, work)? {
        work.step(1)?;
        if cue.end_ns <= cue.start_ns { continue; }
        if !clip && cue.end_ns / 10_000_000 <= cue.start_ns / 10_000_000 { continue; }
        let top = dialect == TextDialect::WebVtt && vtt_line_is_top(&cue.settings);
        let remaining = output_limit.checked_sub(reserved).ok_or("Subtitle conversion exceeds output limit")?;
        let Some(c) = convert_checked(&cue.text, dialect, top.then_some(8), remaining, work)? else { continue };
        let prefix = event_prefix(cue.start_ns / 10_000_000, cue.end_ns / 10_000_000, c.align);
        reserved = reserved.checked_add(prefix.len()).and_then(|n| n.checked_add(c.body.len()))
            .and_then(|n| n.checked_add(1)).filter(|n| *n <= output_limit)
            .ok_or("Subtitle conversion exceeds output limit")?;
        cues.push(ClipCue { start: cue.start_ns, end: cue.end_ns, class: c.class(), seq: cues.len() as u32 });
        converted.push(c);
    }
    let clipped = if clip { clip_newest_wins(&cues) } else {
        let mut spans = Vec::with_capacity(cues.len());
        for cue in &cues { work.step(1)?; spans.push(Some((cue.start, cue.end))); }
        spans
    };
    let mut lines: Vec<(i64, usize, String)> = Vec::new();
    let mut used = 0usize;
    for (i, span) in clipped.iter().enumerate() {
        work.step(1)?;
        let Some((start, end)) = *span else { continue };
        // Floor to the centisecond (under a frame), then drop what rounding emptied.
        let (start_cs, end_cs) = (start / 10_000_000, end / 10_000_000);
        if end_cs <= start_cs { continue; }
        let remaining = output_limit.checked_sub(used).ok_or("Subtitle conversion exceeds output limit")?;
        let mut line = String::new();
        append_bounded(&mut line, &event_prefix(start_cs, end_cs, converted[i].align), remaining)?;
        work.scan(&converted[i].body)?;
        append_bounded(&mut line, &converted[i].body, remaining)?;
        append_bounded(&mut line, "\n", remaining)?;
        used += line.len();
        lines.push((start_cs, i, line));
    }
    work.check()?;
    lines.sort_by_key(|l| (l.0, l.1));
    let count = lines.len();
    let mut events = String::new();
    for (_, _, line) in lines {
        work.scan(&line)?;
        append_bounded(&mut events, &line, output_limit)?;
    }
    work.check()?;
    Ok(TextScript { events, cues: count })
}

/// `H:MM:SS.cc` from centiseconds.
fn centis(cs: i64) -> String {
    format!("{}:{:02}:{:02}.{:02}", cs / 360_000, cs / 6_000 % 60, cs / 100 % 60, cs % 100)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod appearance_tests {
    use super::*;
    use super::text_style::{TextAppearance, TextBackground, TextColor, TextFont, TextOutline};

    fn fields(style: &TextStyle) -> Vec<String> {
        header(style).lines().find(|line| line.starts_with("Style: Default,"))
            .unwrap().trim_start_matches("Style: ").split(',').map(str::to_owned).collect()
    }

    #[test]
    fn default_appearance_preserves_the_existing_header_and_geometry() {
        let row = fields(&TextStyle::default());
        assert_eq!(row.join(","), "Default,PlxSubs,51.49,&H00FFFFFF,&H00FFFFFF,&H26000000,&HFF000000,0,0,0,0,100,100,0,0,1,2,0,2,580,580,97,-1");
        assert_eq!(TextFont::default().family(), STYLE_FONT);
    }

    #[test]
    fn real_header_uses_each_selected_font_and_text_color_without_changing_geometry() {
        let baseline = fields(&TextStyle::default());
        for font in TextFont::LADDER { for color in TextColor::LADDER {
            let style = TextStyle { appearance: TextAppearance { font, color, ..Default::default() }, ..Default::default() };
            let row = fields(&style);
            assert_eq!(row[1], font.family());
            assert_eq!(row[3], format!("&H00{}", color.ass_bgr()));
            assert_eq!(row[4], row[3]);
            assert_eq!(&row[18..], &baseline[18..], "font/colour changed alignment or margins");
            assert_eq!(header(&style).lines().filter(|line| line.starts_with("Style: ")).count(), 9);
        }}
    }

    #[test]
    fn background_opacity_and_outline_remain_independent_in_the_real_header() {
        for background in TextBackground::LADDER { for outline in TextOutline::LADDER {
            let style = TextStyle { appearance: TextAppearance { background, outline, ..Default::default() }, ..Default::default() };
            let row = fields(&style);
            assert_eq!(row[3], "&H00FFFFFF", "background must not make the glyph transparent");
            assert_eq!(row[5], "&H26000000", "background must not change outline opacity");
            assert_eq!(row[6], format!("&H{:02X}000000", background.ass_alpha()));
            assert_eq!(row[15], if background == TextBackground::Transparent { "1" } else { "4" });
            assert_eq!(row[16], outline.pixels().to_string());
            assert_eq!(row[17], "0");
        }}
    }
}

#[cfg(test)]
mod conversion_budget_tests {
    use super::*;

    #[test]
    fn unmatched_braces_are_literal_with_linear_checked_work() {
        let count = 900_000;
        let text = "{".repeat(count);
        let mut checks = 0usize;
        let parsed = {
            let mut stop = || { checks += 1; false };
            Parser::run(&text, TextDialect::WebVtt, &mut ConversionWork::new(&mut stop)).unwrap()
        };
        assert_eq!(parsed.pieces.len(), count);
        assert!(parsed.pieces.iter().all(|piece| *piece == Piece::Ch('{')));
        // Collection, one failed closing-brace scan, and one parser pass.
        assert!(checks <= 3 * count / 256 + 4, "unexpected repeated suffix scans: {checks}");
        assert_eq!(convert("{{unclosed", TextDialect::WebVtt).unwrap().body, "\\{\\{unclosed");
    }

    #[test]
    fn conversion_can_cancel_inside_parser_and_consecutive_render_tags() {
        let mut checks = 0;
        let parsed = {
            let mut stop = || { checks += 1; checks == 2 };
            Parser::run(&"{".repeat(10_000), TextDialect::WebVtt, &mut ConversionWork::new(&mut stop))
        };
        assert_eq!(parsed.err(), Some("Subtitle conversion cancelled"));
        assert_eq!(checks, 2);
        let pieces = vec![Piece::Tag("\\i1".into()); 10_000];
        let mut checks = 0;
        let rendered = {
            let mut stop = || { checks += 1; checks == 2 };
            render(&pieces, usize::MAX, &mut ConversionWork::new(&mut stop))
        };
        assert_eq!(rendered.err(), Some("Subtitle conversion cancelled"));
        assert_eq!(checks, 2);
    }

    #[test]
    fn conversion_can_cancel_inside_deferred_normalization_scan() {
        let mut pieces = vec![Piece::Tag("\\i1".into()); 1024];
        pieces.push(Piece::Break);
        let mut checks = 0;
        let normalized = {
            let mut stop = || { checks += 1; checks == 6 };
            normalize(pieces, &mut ConversionWork::new(&mut stop))
        };
        assert_eq!(normalized.err(), Some("Subtitle conversion cancelled"));
        assert_eq!(checks, 6, "cancellation occurs while revisiting deferred tags");
    }

    #[test]
    fn bounded_timeline_counts_escaped_body_and_complete_event_header() {
        let text = format!("WEBVTT\n\n00:01.000 --> 00:04.000\n{}\n\n", "}".repeat(4096));
        let full = script_from_vtt_timeline_bounded(&text, 16_384, &mut || false).unwrap();
        assert_eq!(full.cues, 1);
        assert!(full.events.len() > 8192, "escaped body plus Dialogue prefix and newline");
        assert_eq!(script_from_vtt_timeline_bounded(&text, full.events.len(), &mut || false).unwrap(), full);
        assert_eq!(script_from_vtt_timeline_bounded(&text, full.events.len() - 1, &mut || false).err(),
            Some("Subtitle conversion exceeds output limit"));
        assert_eq!(script_from_vtt_timeline_bounded(&text, 5000, &mut || false).err(),
            Some("Subtitle conversion exceeds output limit"));
    }
}
