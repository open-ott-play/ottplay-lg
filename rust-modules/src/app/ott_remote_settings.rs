//! The server editor saves a validated, credential-free discovery address through
//! the public preference transaction. It never changes authenticated control tokens.
use super::*;

const ADDRESS: &str = "nativeRemoteServerAddress";

pub(super) fn saved_address(saved: &Saved) -> Result<Option<&str>, &'static str> {
    let Some(value) = saved.settings.get(ADDRESS) else { return Ok(None) };
    let address = value.as_str().ok_or("Invalid saved remote server address")?;
    if Remote::normalize_address(address)? != address {
        return Err("Invalid saved remote server address");
    }
    Ok(Some(address))
}

pub(super) fn address_input(runtime: &Runtime) -> String {
    saved_address(&runtime.saved).ok().flatten()
        .unwrap_or_else(|| runtime.remote.address_input()).to_owned()
}

pub(super) fn candidate(runtime: &Runtime, value: &str) -> Result<Saved, &'static str> {
    let address = Remote::normalize_address(value)?;
    let mut candidate = runtime.saved.clone();
    candidate.settings.insert(ADDRESS.into(), Value::String(address));
    Ok(candidate)
}

/// The writer acknowledgement is the only authority to begin discovery. An
/// abandoned editor still saves its address, but cannot start a later pairing.
pub(super) fn after_saved(
    runtime: &mut Runtime,
    app: &App,
    session: &EditorSession,
    error: Option<String>,
) -> Option<String> {
    if error.is_some() || session.key != "remote.address"
        || runtime.edit_session.as_ref() != Some(session)
        || current_ui_owner(app) != Some(session.owner)
        || runtime.reply != Some(session.owner)
    {
        return error;
    }
    let result = saved_address(&runtime.saved)
        .and_then(|address| address.ok_or("Saved remote server address is missing"))
        .map(str::to_owned)
        .and_then(|address| runtime.remote.discover_and_pair(&address, runtime.now_ms()));
    if let Err(message) = result {
        runtime.status = format!("Address saved. {message}");
        return Some(runtime.status.clone());
    }
    None
}
