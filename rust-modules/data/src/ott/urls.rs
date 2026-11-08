use plx_net::net::origin::Origin;
use plx_net::stream::redirect::{resolve_location, Target};

/// Uses the same strict RFC3986 resolver as the native media transport.
pub fn http_url(value: &str) -> Option<String> {
    http_target(value).map(|target| target.url())
}

fn http_target(value: &str) -> Option<Target> {
    if value.len() > 8192 || value.chars().any(|c| c.is_control() || c.is_whitespace() || c == '\\') { return None; }
    let origin = Origin::parse("https://catalog.invalid:443")?;
    let base = Target { origin, path: "/".into() };
    if !(value.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("http://")) || value.get(..8).is_some_and(|s| s.eq_ignore_ascii_case("https://"))) { return None; }
    resolve_location(&base, value)
}

pub fn resolve_http_url(base: &str, value: &str) -> Option<String> {
    if value.len() > 8192 || value.chars().any(|c| c.is_control() || c.is_whitespace() || c == '\\') { return None; }
    resolve_location(&http_target(base)?, value).map(|target| target.url())
}
