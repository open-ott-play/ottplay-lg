//! Device-bound system television requests using the existing paired player.
//! The server derives the physical TV from this token, never from caller input.
use super::{
    protocol::{Config, Request},
    transport::{self, Outcome, Task},
};
use serde_json::{json, Value};
use std::sync::mpsc::TryRecvError;

pub(in crate::app) struct Pending {
    task: Task,
    generation: u64,
    action: &'static str,
    device: String,
}
impl Pending {
    pub(in crate::app) fn poll(
        &self,
        remote: &super::Remote,
    ) -> Option<Result<Value, &'static str>> {
        if self.generation != remote.generation
            || !available_config(&remote.config, remote.persisted)
        {
            return Some(Err("System TV connection changed; inspect the TV before retrying"));
        }
        match self.task.poll() {
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(
                "System TV request stopped; inspect the TV before retrying",
            )),
            Ok(Outcome::Http {
                response: Ok(response),
                ..
            }) => Some(match response.status {
                200 => serde_json::from_slice::<Value>(&response.body)
                    .map_err(|_| "Invalid system TV response")
                    .and_then(|value| {
                        if value["version"] != 1
                            || value["device_id"].as_str() != Some(self.device.as_str())
                            || value["action"] != self.action
                        {
                            return Err("System TV response identity changed");
                        }
                        Ok(value)
                    }),
                403 => Err("System TV control is not enabled for this paired player on the server"),
                401 => Err("Pair this player with the command server again"),
                _ => Err("System TV request failed; inspect the TV before retrying"),
            }),
            _ => Some(Err(
                "System TV request failed; inspect the TV before retrying",
            )),
        }
    }
}
fn available_config(config: &Config, persisted: bool) -> bool {
    config.enabled && persisted && !config.token.is_empty() && !config.address.is_empty()
}
impl super::Remote {
    pub(in crate::app) fn television_generation(&self) -> u64 {
        self.generation
    }
    pub(in crate::app) fn television_available(&self) -> bool {
        available_config(&self.config, self.persisted)
    }
    pub(in crate::app) fn television_request(
        &self,
        action: &'static str,
        channel_id: Option<&str>,
    ) -> Result<Pending, &'static str> {
        if !self.television_available() {
            return Err("Pair this player with a command server first");
        }
        if !matches!(
            action,
            "channels" | "channel_current" | "channel_program" | "channel_select"
        ) {
            return Err("Unsupported system TV action");
        }
        let mut body = json!({"action":action,"timeout_ms":6000});
        if let Some(id) = channel_id {
            if id.trim().is_empty() || id.len() > 512 || id.chars().any(char::is_control) {
                return Err("Invalid system TV channel identity");
            }
            body["channel_id"] = json!(id);
        }
        let task = transport::send_bounded(
            Request::new(&self.config, "POST", "/api/television-channels", Some(body)),
            4 * 1024 * 1024,
        )
        .ok_or("Could not start system TV request")?;
        Ok(Pending {
            task,
            generation: self.generation,
            action,
            device: self.config.device_id.clone(),
        })
    }
}
