//! Edem guide JSON uses template guide IDs independently of stream channel numbers.
use super::{ott, CatalogEntry, ProviderHttp, ProviderProfile, Request};
pub(super) fn guide(
    profile: &ProviderProfile,
    entry: &CatalogEntry,
    request: &ott::guide::Request,
    http: &mut dyn ProviderHttp,
) -> Result<ott::guide::ResultSet, &'static str> {
    let config = ott::edem::Config::parse(&profile.source).map_err(|_| "Invalid Edem profile")?;
    let guide_id = entry
        .live
        .as_ref()
        .and_then(|live| live.guide_id.as_deref())
        .ok_or("Channel has no guide identifier")?;
    if guide_id.is_empty()
        || guide_id.len() > 256
        || !guide_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err("Invalid Edem guide identifier");
    }
    let namespace = if config.list == 1 {
        "iptv-e2-soveni"
    } else {
        "edem"
    };
    let response = http.request(&Request {
        url: format!("https://epg.drm-play.com/{namespace}/epg/{guide_id}.json"),
        headers: vec!["User-Agent: Mozilla/5.0".into()],
        body: None,
    })?;
    if http.cancelled() {
        return Err("Provider request cancelled");
    }
    if !(200..300).contains(&response.status) || response.body.len() > ott::MAX_CATALOG_BYTES {
        return Err("Edem guide request failed");
    }
    let data: serde_json::Value =
        serde_json::from_slice(&response.body).map_err(|_| "Invalid Edem guide")?;
    json_guide(&data["epg_data"], guide_id, entry, request)
}
pub(super) fn json_guide(
    data: &serde_json::Value,
    guide_id: &str,
    entry: &CatalogEntry,
    request: &ott::guide::Request,
) -> Result<ott::guide::ResultSet, &'static str> {
    let rows = data
        .as_array()
        .filter(|rows| rows.len() <= ott::MAX_CATALOG_ENTRIES)
        .ok_or("Invalid Edem guide")?;
    let timestamp = |value: &serde_json::Value| {
        value
            .as_i64()
            .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
    };
    let mut programmes = Vec::new();
    for row in rows {
        let (Some(start), Some(end), Some(title)) = (
            timestamp(&row["time"]),
            timestamp(&row["time_to"]),
            row["name"].as_str(),
        ) else {
            return Err("Invalid provider programme");
        };
        let description = row["descr"].as_str().unwrap_or("");
        if start < 0
            || end <= start
            || title.len() > ott::MAX_METADATA_BYTES
            || description.len() > ott::MAX_METADATA_BYTES
        {
            return Err("Invalid provider programme");
        }
        programmes.push(ott::epg::Programme {
            channel: guide_id.into(),
            start,
            end,
            title: title.into(),
            description: description.into(),
        });
    }
    let programmes = ott::guide::select(
        &programmes,
        guide_id,
        request.query,
        entry
            .live
            .as_ref()
            .map_or(0, |live| live.guide_shift_seconds),
    )
    .map_err(|_| "Invalid provider guide interval")?;
    Ok(ott::guide::ResultSet {
        context: request.context.clone(),
        query: request.query,
        coverage: ott::guide::Coverage::RequestedWindow,
        programmes,
    })
}
