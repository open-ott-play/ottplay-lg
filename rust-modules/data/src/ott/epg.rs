//! Bounded XMLTV reader. No DTD, entity expansion, filesystem or network access.
use super::{
    CatalogError, MAX_CATALOG_BYTES, MAX_CATALOG_ENTRIES, MAX_CATALOG_LINE_BYTES,
    MAX_METADATA_BYTES,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Programme {
    pub channel: String,
    pub start: i64,
    pub end: i64,
    pub title: String,
    pub description: String,
}

/// XMLTV timestamps must carry an explicit offset; guessing the TV timezone shifts EPG.
pub fn timestamp(value: &str) -> Option<i64> {
    let parts: Vec<_> = value.split_whitespace().collect();
    if parts.len() != 2 || parts[0].len() != 14 || parts[1].len() != 5 {
        return None;
    }
    let d = parts[0];
    let z = parts[1];
    if !d.bytes().all(|b| b.is_ascii_digit()) || !z.as_bytes()[1..].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let n = |a, b| d.get(a..b)?.parse::<i64>().ok();
    let (y, m, day, h, min, s) = (
        n(0, 4)?,
        n(4, 6)?,
        n(6, 8)?,
        n(8, 10)?,
        n(10, 12)?,
        n(12, 14)?,
    );
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1970..=9999).contains(&y)
        || !(1..=12).contains(&m)
        || day < 1
        || day > days[(m - 1) as usize]
        || h > 23
        || min > 59
        || s > 59
    {
        return None;
    }
    let (zh, zm) = (z[1..3].parse::<i64>().ok()?, z[3..5].parse::<i64>().ok()?);
    if zh > 23 || zm > 59 {
        return None;
    }
    let sign = match &z[..1] {
        "+" => 1,
        "-" => -1,
        _ => return None,
    };
    let before = |v: i64| 365 * v + v / 4 - v / 100 + v / 400;
    let epoch_days =
        before(y - 1) - before(1969) + days[..(m - 1) as usize].iter().sum::<i64>() + day - 1;
    Some(epoch_days * 86400 + h * 3600 + min * 60 + s - sign * (zh * 3600 + zm * 60))
}

fn unescape(input: &str) -> Result<String, CatalogError> {
    let mut out = String::new();
    let mut rest = input;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i + 1..];
        let end = rest.find(';').ok_or(CatalogError::InvalidFormat)?;
        let entity = &rest[..end];
        let ch = match entity {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let code = if let Some(v) = entity.strip_prefix("#x") {
                    u32::from_str_radix(v, 16).ok()
                } else {
                    entity.strip_prefix('#').and_then(|v| v.parse().ok())
                };
                code.and_then(char::from_u32)
                    .filter(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t'))
                    .ok_or(CatalogError::InvalidFormat)?
            }
        };
        out.push(ch);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}
fn attrs(input: &str) -> Result<Vec<(String, String)>, CatalogError> {
    let mut rest = input;
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();
    while !rest.trim().is_empty() {
        rest = rest.trim_start();
        if rest == "/" {
            break;
        }
        let eq = rest.find('=').ok_or(CatalogError::InvalidFormat)?;
        let key = rest[..eq].trim();
        if key.is_empty() || key.chars().any(char::is_whitespace) {
            return Err(CatalogError::InvalidFormat);
        }
        rest = rest[eq + 1..].trim_start();
        let quote = rest.chars().next().ok_or(CatalogError::InvalidFormat)?;
        if !matches!(quote, '\'' | '"') {
            return Err(CatalogError::InvalidFormat);
        }
        rest = &rest[1..];
        let end = rest.find(quote).ok_or(CatalogError::InvalidFormat)?;
        if !seen.insert(key) {
            return Err(CatalogError::InvalidFormat);
        }
        result.push((key.to_string(), unescape(&rest[..end])?));
        rest = &rest[end + 1..];
    }
    Ok(result)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Channel {
    pub id: String,
    pub names: Vec<String>,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Document {
    pub channels: Vec<Channel>,
    pub programmes: Vec<Programme>,
    pub invalid_channels: BTreeMap<String, CatalogError>,
}

pub fn parse_xmltv(input: &str, channel: &str) -> Result<Vec<Programme>, CatalogError> {
    Ok(parse(input, Some(channel))?.programmes)
}

/// Discover identities and names before choosing which channel to parse strictly.
/// Programme-only IDs also count; unrelated programme timestamps are not interpreted.
pub fn discover_channels(input: &str) -> Result<Vec<Channel>, CatalogError> {
    Ok(parse(input, None)?.channels)
}

fn parse(input: &str, wanted: Option<&str>) -> Result<Document, CatalogError> {
    read(input, wanted, false, &mut || false)?.ok_or(CatalogError::InvalidFormat)
}

/// The same structural reader, retaining titles and intervals but no descriptions.
/// None means the supplied cancellation predicate fired; no partial result escapes.
pub(super) fn compact(
    input: &str,
    cancelled: &mut dyn FnMut() -> bool,
) -> Result<Option<Document>, CatalogError> {
    read(input, None, true, cancelled)
}

fn append_text(
    current: &mut Option<Programme>, field: Option<&str>, text: &str,
    compact: bool, metadata_bytes: &mut usize,
    invalid: &mut BTreeMap<String, CatalogError>,
) {
    let Some(row) = current.as_mut() else { return; };
    if !matches!(field, Some("title" | "desc")) { return; }
    if compact {
        *metadata_bytes = metadata_bytes.saturating_add(text.len());
        if *metadata_bytes > MAX_METADATA_BYTES {
            invalid.entry(row.channel.clone()).or_insert(CatalogError::LimitExceeded);
            *current = None;
            return;
        }
    }
    match field {
        Some("title") => row.title.push_str(text),
        Some("desc") if !compact => row.description.push_str(text),
        _ => {}
    }
}

fn read(
    input: &str, wanted: Option<&str>, compact: bool,
    cancelled: &mut dyn FnMut() -> bool,
) -> Result<Option<Document>, CatalogError> {
    if cancelled() { return Ok(None); }
    if input.len() > MAX_CATALOG_BYTES {
        return Err(CatalogError::LimitExceeded);
    }
    let mut rest = input.trim_start_matches('\u{feff}');
    let mut stack: Vec<String> = Vec::new();
    let mut current: Option<Programme> = None;
    let mut metadata_bytes = 0usize;
    let mut invalid_channels = BTreeMap::new();
    let mut rows = Vec::new();
    let mut channels = Vec::new();
    let mut programme_ids = BTreeSet::new();
    let mut channel: Option<Channel> = None;
    let mut display_name = String::new();
    let mut count = 0;
    let mut root = false;
    while !rest.is_empty() {
        if cancelled() { return Ok(None); }
        if current
            .as_ref()
            .is_some_and(|p| p.title.len() + p.description.len() > MAX_METADATA_BYTES)
        {
            return Err(CatalogError::LimitExceeded);
        }
        let i = rest.find('<').unwrap_or(rest.len());
        let text = unescape(&rest[..i])?;
        if stack.is_empty() && !text.trim().is_empty() {
            return Err(CatalogError::InvalidFormat);
        }
        if stack.last().is_some_and(|s| s == "display-name") && channel.is_some() {
            display_name.push_str(&text);
            if display_name.len() > MAX_METADATA_BYTES {
                return Err(CatalogError::LimitExceeded);
            }
        }
        append_text(&mut current, stack.last().map(String::as_str), &text,
            compact, &mut metadata_bytes, &mut invalid_channels);
        if current
            .as_ref()
            .is_some_and(|p| p.title.len() + p.description.len() > MAX_METADATA_BYTES)
        {
            return Err(CatalogError::LimitExceeded);
        }
        rest = &rest[i..];
        if rest.is_empty() {
            break;
        }
        if rest.starts_with("<!--") {
            let end = rest.find("-->").ok_or(CatalogError::Truncated)?;
            rest = &rest[end + 3..];
            continue;
        }
        if rest.starts_with("<![CDATA[") {
            let end = rest.find("]]>").ok_or(CatalogError::Truncated)?;
            if stack.is_empty() {
                return Err(CatalogError::InvalidFormat);
            }
            if stack.last().is_some_and(|s| s == "display-name") && channel.is_some() {
                display_name.push_str(&rest[9..end]);
                if display_name.len() > MAX_METADATA_BYTES {
                    return Err(CatalogError::LimitExceeded);
                }
            }
            append_text(&mut current, stack.last().map(String::as_str), &rest[9..end],
                compact, &mut metadata_bytes, &mut invalid_channels);
            rest = &rest[end + 3..];
            continue;
        }
        if rest.starts_with("<?") {
            let end = rest.find("?>").ok_or(CatalogError::Truncated)?;
            rest = &rest[end + 2..];
            continue;
        }
        if rest.starts_with("<!") {
            return Err(CatalogError::InvalidFormat);
        }
        let mut quote = None;
        let end = rest
            .char_indices()
            .skip(1)
            .find_map(|(i, c)| {
                if Some(c) == quote {
                    quote = None;
                } else if quote.is_none() && matches!(c, '\'' | '"') {
                    quote = Some(c);
                } else if quote.is_none() && c == '>' {
                    return Some(i);
                }
                None
            })
            .ok_or(CatalogError::Truncated)?;
        if end > MAX_CATALOG_LINE_BYTES {
            return Err(CatalogError::LimitExceeded);
        }
        let tag = rest[1..end].trim();
        rest = &rest[end + 1..];
        if let Some(close) = tag.strip_prefix('/') {
            if stack.pop().as_deref() != Some(close) {
                return Err(CatalogError::InvalidFormat);
            }
            if close == "display-name" {
                if let Some(channel) = &mut channel {
                    let name = display_name.trim();
                    if !name.is_empty() {
                        channel.names.push(name.to_owned());
                    }
                }
                display_name.clear();
            }
            if close == "channel" {
                if let Some(channel) = channel.take() {
                    channels.push(channel);
                }
            }
            if close == "programme" {
                if let Some(mut row) = current.take() {
                    row.title = row.title.trim().to_string();
                    row.description = row.description.trim().to_string();
                    rows.push(row);
                }
            }
            continue;
        }
        let name = tag
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .ok_or(CatalogError::InvalidFormat)?;
        let attributes = attrs(&tag[name.len()..])?;
        if stack.is_empty() {
            if root || name != "tv" {
                return Err(CatalogError::InvalidFormat);
            }
            root = true;
        } else if name == "tv" {
            return Err(CatalogError::InvalidFormat);
        }
        if stack.len() >= 32 {
            return Err(CatalogError::LimitExceeded);
        }
        if name == "channel" && wanted.is_none() {
            if stack.last().map(String::as_str) != Some("tv")
                || channels.len() >= MAX_CATALOG_ENTRIES
            {
                return Err(CatalogError::InvalidFormat);
            }
            channel = attributes
                .iter()
                .find(|(k, _)| k == "id")
                .map(|(_, v)| v.clone())
                .filter(|v| !v.is_empty())
                .map(|id| Channel {
                    id,
                    names: Vec::new(),
                });
        }
        if name == "display-name" {
            display_name.clear();
        }
        if name == "programme" {
            if stack.last().map(String::as_str) != Some("tv") {
                return Err(CatalogError::InvalidFormat);
            }
            count += 1;
            if count > MAX_CATALOG_ENTRIES {
                return Err(CatalogError::LimitExceeded);
            }
            let attr = |k| {
                attributes
                    .iter()
                    .find(|(key, _)| key == k)
                    .map(|(_, v)| v.as_str())
            };
            if wanted.is_none() {
                if let Some(id) = attr("channel").filter(|id| !id.is_empty()) {
                    programme_ids.insert(id.to_string());
                }
            }
            if (wanted.is_some() && attr("channel") == wanted)
                || (compact && attr("channel").is_some_and(|id| !id.is_empty()))
            {
                let id = attr("channel").unwrap_or("");
                let interval = attr("start").and_then(timestamp)
                    .zip(attr("stop").and_then(timestamp))
                    .filter(|(start, end)| end > start);
                if let Some((start, end)) = interval {
                    if !invalid_channels.contains_key(id) {
                        metadata_bytes = 0;
                        current = Some(Programme {
                            channel: id.into(), start, end,
                            title: String::new(), description: String::new(),
                        });
                    }
                } else if compact {
                    invalid_channels.entry(id.to_owned()).or_insert(CatalogError::InvalidFormat);
                } else {
                    return Err(CatalogError::InvalidFormat);
                }
            }
        }
        if !tag.ends_with('/') {
            stack.push(name.to_string());
        } else if name == "channel" {
            if let Some(channel) = channel.take() {
                channels.push(channel);
            }
        } else if name == "programme" {
            if let Some(row) = current.take() {
                rows.push(row);
            }
        }
    }
    if !root || !stack.is_empty() || current.is_some() {
        return Err(CatalogError::Truncated);
    }
    let mut known: BTreeSet<String> = channels.iter().map(|channel| channel.id.clone()).collect();
    for id in programme_ids {
        if known.insert(id.clone()) {
            if channels.len() >= MAX_CATALOG_ENTRIES {
                return Err(CatalogError::LimitExceeded);
            }
            channels.push(Channel {
                id,
                names: Vec::new(),
            });
        }
    }
    if !compact { rows.sort_by_key(|p| p.start); }
    if cancelled() { return Ok(None); }
    Ok(Some(Document {
        channels,
        programmes: rows,
        invalid_channels,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_non_xmltv_roots_and_text() {
        for input in [
            "plain text",
            "<html/>",
            "<![CDATA[text]]><tv/>",
            "<tv><tv/></tv>",
            "<tv/><tv/>",
        ] {
            assert!(parse_xmltv(input, "x").is_err(), "{input}");
        }
        assert!(parse_xmltv("<tv/>", "x").unwrap().is_empty());
    }
    #[test]
    fn metadata_and_start_tags_have_independent_bounds() {
        let xml=format!("<tv><programme channel='x' start='20260101000000 +0000' stop='20260101010000 +0000'><desc>{}</desc></programme></tv>", "x".repeat(MAX_METADATA_BYTES+1));
        assert_eq!(parse_xmltv(&xml, "x"), Err(CatalogError::LimitExceeded));
        let xml = format!("<tv huge='{}'/>", "x".repeat(MAX_CATALOG_LINE_BYTES));
        assert_eq!(parse_xmltv(&xml, "x"), Err(CatalogError::LimitExceeded));
        let xml="<tv><programme channel='x' start='20260101000000 +0000' stop='20260101010000 +0000'/></tv>";
        assert_eq!(parse_xmltv(xml, "x").unwrap().len(), 1);
    }
    #[test]
    fn offsets_and_leap_days() {
        assert_eq!(timestamp("19700101000000 +0000"), Some(0));
        assert_eq!(timestamp("19700101010000 +0100"), Some(0));
        assert!(timestamp("20230229000000 +0000").is_none());
        assert!(timestamp("20240229000000 +0000").is_some());
    }
    #[test]
    fn reads_descriptions_and_rejects_entities() {
        let xml="<tv><programme channel='x' start='20260101000000 +0000' stop='20260101010000 +0000'><title>A &amp; B</title><desc><![CDATA[Long <description>]]></desc></programme></tv>";
        let rows = parse_xmltv(xml, "x").unwrap();
        assert_eq!(rows[0].title, "A & B");
        assert_eq!(rows[0].description, "Long <description>");
        assert!(parse_xmltv("<!DOCTYPE tv SYSTEM 'file:///private'><tv/>", "x").is_err());
        assert!(parse_xmltv("<tv>", "x").is_err());
    }
}

#[cfg(test)]
mod discovery_tests {
    use super::*;
    #[test]
    fn discovery_finds_programme_only_ids_without_interpreting_bad_times() {
        let xml="<tv><channel id='good'><display-name>Guide name</display-name></channel><programme channel='good' start='20260101000000 +0000' stop='20260101010000 +0000'><title>Current</title></programme><programme channel='bad' start='invalid'><title>Broken</title></programme></tv>";
        let channels = discover_channels(xml).unwrap();
        assert!(channels
            .iter()
            .any(|c| c.id == "good" && c.names == ["Guide name"]));
        assert!(channels.iter().any(|c| c.id == "bad" && c.names.is_empty()));
        assert_eq!(parse_xmltv(xml, "good").unwrap()[0].title, "Current");
        assert!(parse_xmltv(xml, "bad").is_err());
    }
    #[test]
    fn discovery_retains_document_and_size_boundaries() {
        assert!(discover_channels("<!DOCTYPE tv SYSTEM 'file:///private'><tv/>").is_err());
        assert!(discover_channels("<tv><programme channel='x'/>").is_err());
        assert_eq!(
            discover_channels(&" ".repeat(MAX_CATALOG_BYTES + 1)),
            Err(CatalogError::LimitExceeded)
        );
    }
}
