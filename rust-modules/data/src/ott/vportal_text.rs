//! Tolerant, bounded provider prose to plain text. This is not an HTML renderer:
//! only known formatting tags are consumed; unknown or malformed angle prose stays literal.
use super::{CatalogError, MAX_METADATA_BYTES};

const TAG_BYTES: usize = 512;
const ENTITY_BYTES: usize = 32;

pub(super) fn plain_text(input: &str) -> Result<String, CatalogError> {
    if input.len() > MAX_METADATA_BYTES {
        return Err(CatalogError::LimitExceeded);
    }
    // One entity pass also accepts source-escaped formatting such as &lt;br&gt;.
    // The later tag pass never decodes entities: &amp;lt;b&amp;gt; stays escaped.
    let mut decoded = String::with_capacity(input.len());
    let mut at = 0;
    while at < input.len() {
        let rest = &input[at..];
        if rest.starts_with('&') {
            if let Some((used, ch)) = entity(rest) {
                decoded.push(ch);
                at += used;
                continue;
            }
        }
        let ch = rest.chars().next().expect("nonempty suffix");
        if ch == '\r' {
            decoded.push('\n');
            at += if rest.starts_with("\r\n") { 2 } else { 1 };
        } else {
            decoded.push(if ch.is_control() && ch != '\n' && ch != '\t' {
                ' '
            } else {
                ch
            });
            at += ch.len_utf8();
        }
    }
    let mut output = String::with_capacity(decoded.len());
    at = 0;
    while at < decoded.len() {
        let rest = &decoded[at..];
        if rest.starts_with('<') {
            if let Some((used, breaks)) = formatting_tag(rest) {
                if !output.is_empty() {
                    if breaks == 1 {
                        output.push('\n');
                    } else if breaks == 2 {
                        let existing = output
                            .bytes()
                            .rev()
                            .take(2)
                            .take_while(|b| *b == b'\n')
                            .count();
                        for _ in existing..2 {
                            output.push('\n');
                        }
                    }
                }
                at += used;
                continue;
            }
        }
        let ch = rest.chars().next().expect("nonempty suffix");
        output.push(ch);
        at += ch.len_utf8();
    }
    // Every replacement is no larger than its input spelling. Keep the output
    // admission explicit so future entities/tags cannot silently truncate text.
    if output.len() > MAX_METADATA_BYTES {
        return Err(CatalogError::LimitExceeded);
    }
    Ok(output.trim().to_owned())
}

fn formatting_tag(input: &str) -> Option<(usize, u8)> {
    let bytes = input.as_bytes();
    let mut at = 1;
    if bytes.get(at) == Some(&b'/') {
        at += 1;
    }
    let start = at;
    while bytes.get(at).is_some_and(u8::is_ascii_alphanumeric) {
        at += 1;
    }
    if at == start || at - start > 10 {
        return None;
    }
    let name = input[start..at].to_ascii_lowercase();
    let breaks = match name.as_str() {
        "br" => 1,
        "p" | "div" | "ul" | "ol" | "li" | "h1" | "h2" | "h3" | "blockquote" => 2,
        "b" | "strong" | "i" | "em" | "u" | "s" | "del" | "small" | "span" | "font" | "a"
        | "sub" | "sup" => 0,
        _ => return None,
    };
    if !bytes
        .get(at)
        .is_some_and(|b| b.is_ascii_whitespace() || matches!(*b, b'/' | b'>'))
    {
        return None;
    }
    let mut quote = None;
    while at < bytes.len().min(TAG_BYTES) {
        let byte = bytes[at];
        if let Some(end) = quote {
            if byte == end {
                quote = None;
            }
        } else {
            match byte {
                b'\'' | b'"' => quote = Some(byte),
                b'>' => return Some((at + 1, breaks)),
                b'<' => return None,
                _ => {}
            }
        }
        at += 1;
    }
    None
}

fn entity(input: &str) -> Option<(usize, char)> {
    let end = input
        .as_bytes()
        .iter()
        .take(ENTITY_BYTES)
        .position(|b| *b == b';')?;
    let name = &input[1..end];
    let ch = match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "ndash" => '–',
        "mdash" => '—',
        "hellip" => '…',
        "laquo" => '«',
        "raquo" => '»',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "bull" => '•',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "euro" => '€',
        "deg" => '°',
        _ => {
            let (digits, radix) =
                if let Some(hex) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
                    (hex, 16)
                } else {
                    (name.strip_prefix('#')?, 10)
                };
            if digits.is_empty()
                || !digits.bytes().all(|b| {
                    if radix == 16 {
                        b.is_ascii_hexdigit()
                    } else {
                        b.is_ascii_digit()
                    }
                })
            {
                return None;
            }
            char::from_u32(u32::from_str_radix(digits, radix).ok()?)?
        }
    };
    if ch.is_control() && !matches!(ch, '\n' | '\t') {
        return None;
    }
    Some((end + 1, ch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting_and_entities_become_plain_paragraphs_once() {
        assert_eq!(plain_text("<p><b>Описание</b> &amp; &#x442;&#1077;&#1082;&#1089;&#1090;.</p><p>Вторая<br />строка&nbsp;&laquo;да&raquo;.</p>").unwrap(),
            "Описание & текст.\n\nВторая\nстрока\u{a0}«да».");
        assert_eq!(plain_text("A<br><BR/>B\r\nC").unwrap(), "A\n\nB\nC");
        assert_eq!(
            plain_text("&lt;b&gt;quoted&lt;/b&gt; &amp;lt;end&amp;gt;").unwrap(),
            "quoted &lt;end&gt;"
        );
        assert_eq!(
            plain_text("<b>Название</b><br>Текст").unwrap(),
            plain_text("&lt;b&gt;Название&lt;/b&gt;&lt;br&gt;Текст").unwrap()
        );
        assert_eq!(
            plain_text("&lt;phase&gt;literal&lt;/phase&gt;").unwrap(),
            "<phase>literal</phase>"
        );
        assert_eq!(
            plain_text("&amp;lt;b&amp;gt;text&amp;lt;/b&amp;gt;").unwrap(),
            "&lt;b&gt;text&lt;/b&gt;"
        );
    }

    #[test]
    fn unknown_or_malformed_text_is_not_guessed_as_markup() {
        for prose in [
            "2 < 3 and 5 > 4",
            "<phase> remains </phase>",
            "<b not closed",
            "<bold>literal</bold>",
            "&unknown; &#0; &#xD800; &#x110000; &unfinished",
            "<script>alert(1)</script>",
        ] {
            assert_eq!(plain_text(prose).unwrap(), prose);
        }
        assert_eq!(
            plain_text("<span title='a > b'>visible</span>").unwrap(),
            "visible"
        );
    }

    #[test]
    fn bounds_preserve_complete_multibyte_prose_or_reject_it() {
        let full = format!(
            "{}КОНЕЦ",
            "я".repeat((MAX_METADATA_BYTES - "КОНЕЦ".len()) / 2)
        );
        assert_eq!(plain_text(&full).unwrap(), full);
        assert_eq!(
            plain_text(&"я".repeat(MAX_METADATA_BYTES / 2 + 1)),
            Err(CatalogError::LimitExceeded)
        );
        let malformed = "<".repeat(MAX_METADATA_BYTES);
        assert_eq!(plain_text(&malformed).unwrap(), malformed);
    }
}
