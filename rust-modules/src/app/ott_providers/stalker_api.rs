//! JSON-RPC adapter, run only by the existing provider worker.
use super::{ProviderHttp, Request};
use ott::stalker::{Config, Dialect};
use ott::stalker_api::{self as codec, Operation};
use plx_data::ott::{self, CatalogEntry, MediaLocator, ProviderProfile};
use serde_json::Value;

struct Session<'a> {
    config: Config,
    http: &'a mut dyn ProviderHttp,
    renewed: bool,
}
impl<'a> Session<'a> {
    fn connect(
        http: &'a mut dyn ProviderHttp,
        profile: &ProviderProfile,
    ) -> Result<Self, &'static str> {
        profile.validate().map_err(|_| "Invalid Stalker profile")?;
        if profile.kind != ott::ProviderKind::Stalker {
            return Err("Invalid Stalker profile");
        }
        let config = Config::parse(&profile.source).map_err(|_| "Invalid Stalker profile")?;
        if config.dialect() != Dialect::JsonRpc {
            return Err("Stalker dialect mismatch");
        }
        let mut session = Self {
            config,
            http,
            renewed: false,
        };
        let value = session.raw(Operation::Handshake)?;
        codec::handshake(&value)?;
        Ok(session)
    }
    fn raw(&mut self, operation: Operation<'_>) -> Result<Value, &'static str> {
        if self.http.cancelled() {
            return Err("Provider request cancelled");
        }
        let body = serde_json::to_vec(&codec::request(&self.config, operation)?)
            .map_err(|_| "Invalid Stalker request")?;
        let response = self.http.request(&Request {
            url: self.config.portal.clone(),
            headers: vec![
                "Content-Type: application/json".into(),
                "Accept: application/json".into(),
            ],
            body: Some(body),
        })?;
        if self.http.cancelled() {
            return Err("Provider request cancelled");
        }
        if matches!(response.status, 401 | 403) {
            return Err("Stalker authorization expired");
        }
        if !(200..300).contains(&response.status) {
            return Err("Stalker request failed");
        }
        if response.body.len() > ott::MAX_CATALOG_BYTES {
            return Err("Stalker response exceeds limit");
        }
        let response: Value =
            serde_json::from_slice(&response.body).map_err(|_| "Invalid Stalker JSON response")?;
        Ok(codec::result(&response)?.clone())
    }
    fn call(&mut self, operation: Operation<'_>) -> Result<Value, &'static str> {
        match self.raw(operation) {
            Err("Stalker authorization expired") if !self.renewed => {
                self.renewed = true;
                let value = self.raw(Operation::Handshake)?;
                codec::handshake(&value)?;
                self.raw(operation)
            }
            result => result,
        }
    }
}

pub(super) fn visit(
    profile: &ProviderProfile,
    http: &mut dyn ProviderHttp,
    emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    let mut session = Session::connect(http, profile)?;
    let data = session.call(Operation::Channels)?;
    let catalog = codec::catalog(profile, &session.config, &data)?;
    let mut batch = Vec::new();
    for entry in catalog.entries {
        if session.http.cancelled() {
            return Err("Provider request cancelled");
        }
        batch.push(entry);
        if batch.len() == 128 {
            emit(std::mem::take(&mut batch))?;
        }
    }
    if session.http.cancelled() {
        return Err("Provider request cancelled");
    }
    if !batch.is_empty() {
        emit(batch)?;
    }
    if session.http.cancelled() {
        return Err("Provider request cancelled");
    }
    Ok(())
}

pub(super) fn resolve(
    profile: &ProviderProfile,
    entry: &CatalogEntry,
    http: &mut dyn ProviderHttp,
) -> Result<String, &'static str> {
    let MediaLocator::StalkerApi { channel_id } = &entry.media else {
        return Err("Invalid Stalker locator");
    };
    codec::valid_id(channel_id)?; // malformed locator never reaches I/O
    let mut session = Session::connect(http, profile)?;
    let data = session.call(Operation::Channels)?;
    codec::resolve(&session.config, &data, channel_id)
}

pub(super) fn guide(
    profile: &ProviderProfile,
    entry: &CatalogEntry,
    request: &ott::guide::Request,
    http: &mut dyn ProviderHttp,
) -> Result<ott::guide::ResultSet, &'static str> {
    if request.context.profile_id != profile.id || request.context.entry_id != entry.id {
        return Err("Guide context mismatch");
    }
    let query = request.query;
    query.validate().map_err(|_| "Invalid guide interval")?;
    let MediaLocator::StalkerApi {
        channel_id: channel,
    } = &entry.media
    else {
        return Err("Invalid Stalker locator");
    };
    codec::valid_id(channel)?;
    let (from, to) = match query {
        ott::guide::Query::CurrentNext { now } => (
            now.saturating_sub(86400).max(0),
            now.checked_add(86400).ok_or("Invalid guide time")?,
        ),
        ott::guide::Query::Day { start, end } => (start, end),
        ott::guide::Query::Records { .. } => return Err("Records require an explicit M3U catch-up source"),
    };
    let mut session = Session::connect(http, profile)?;
    let data = session.call(Operation::Guide { channel, from, to })?;
    let rows = codec::guide(channel, &data)?;
    let programmes =
        ott::guide::select(&rows, channel, query, 0).map_err(|_| "Invalid Stalker guide")?;
    Ok(ott::guide::ResultSet {
        context: request.context.clone(),
        query,
        coverage: ott::guide::Coverage::RequestedWindow,
        programmes,
    })
}

#[cfg(test)]
#[path = "stalker_api_tests.rs"]
mod tests;
