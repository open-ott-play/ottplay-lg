//! Outbound native OTT control. No listening socket, browser, or debug FIFO.
//! Models in protocol are pure. Command delivery, notification polling and image
//! loading each retain one bounded IO task, including canceled workers.
pub(crate) use protocol::favorites_rejection;
mod protocol;
mod discovery;
use discovery::{CatalogProfile, ControllerBase, DiscoveryTarget};
mod transport;
mod display;
pub(super) mod media_tracks;
pub(super) mod plex_servers;
mod ingress;
mod notifications;
pub(crate) use protocol::{Config, Invocation};
use protocol::{Delivery, PairResult, Pairing, Request, SendKind};
use serde_json::{json, Value};
use std::sync::mpsc::TryRecvError;
use transport::{Outcome, Task};

fn display_text(value: &Value, limit: usize) -> Value {
    value.as_str().and_then(|value| display::text(value, limit))
        .map_or(Value::Null, Value::String)
}

/// Closed, credential-free live playback shape. Retained presence further limits IDs to v1.
pub(crate) fn project_playback(value: &Value) -> Value {
    let mut playback = serde_json::Map::new();
    playback.insert("title".into(), display_text(&value["title"], 256));
    playback.insert("provider".into(), display_text(&value["provider"], 64));
    for (key, max) in [
        ("position", 9007199254740991.0),
        ("duration", 9007199254740991.0),
        ("volume", 100.0),
    ] {
        playback.insert(key.into(), value[key].as_f64()
            .filter(|n| n.is_finite() && *n >= 0.0 && *n <= max)
            .map_or(Value::Null, |n| json!(n)));
    }
    playback.insert("channel_number".into(), value["channel_number"].as_u64()
        .filter(|n| (1..=2147483647).contains(n))
        .map_or(Value::Null, |n| json!(n)));
    playback.insert("media_id".into(), value["media_id"].as_str()
        .and_then(display::media_id).map_or(Value::Null, |id| json!(id)));
    let state = value["state"].as_str().filter(|state| matches!(*state,
        "unknown" | "idle" | "loading" | "playing" | "paused" | "buffering"
        | "ended" | "error" | "standby")).unwrap_or("unknown");
    let kind = value["kind"].as_str().filter(|kind| matches!(*kind,
        "none" | "live" | "archive" | "vod" | "unknown")).unwrap_or("unknown");
    playback.insert("state".into(), json!(state));
    playback.insert("kind".into(), json!(kind));
    if matches!(state, "idle" | "ended" | "standby") {
        for key in ["title", "media_id", "channel_number"] {
            playback.insert(key.into(), Value::Null);
        }
    }
    Value::Object(playback)
}

#[derive(Clone)]
enum Kind {
    Load,
    Save(Config),
    Delivery(SendKind),
    Discovery,
    Catalog(CatalogProfile),
    LanDiscovery,
    Pairing,
    CancelPairing,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AfterReply {
    Exit,
    Standby(bool),
    Volume {
        level: u8,
        muted: bool,
        last_input: u32,
    },
    Input {
        key: plx_machine::machine::Key,
        sym: u32,
        wcode: u32,
        page: Option<plx_machine::machine::InstanceId>,
        surface: Option<&'static str>,
        last_input: u32,
    },
}
struct Flight {
    kind: Kind,
    generation: u64,
    task: Task,
}

pub(crate) struct Remote {
    notifications: Box<notifications::State>,
    config: Config,
    write_permit: plx_platform::storage::private::WritePermit,
    runtime: String,
    generation: u64,
    flight: Option<Flight>,
    delivery: Delivery,
    pairing: Option<Pairing>,
    cleanup: Option<Request>,
    pending_save: Option<Config>,
    loading: bool,
    confirm_loaded_config: bool,
    next_ms: u64,
    failures: u8,
    status: &'static str,
    message: &'static str,
    network_diagnostic: String,
    persisted: bool,
    epoch: String,
    presence_sequence: u64,
    presence_at: u64,
    presence_pending: Option<Value>,
    configuration_receipt: Option<Value>,
    discover_address: Option<DiscoveryTarget>,
    lan_pending: bool,
    lan_deadline: Option<u64>,
    catalog_profile: Result<Option<CatalogProfile>, &'static str>,
    find_state: &'static str,
    find_message: &'static str,
    find_diagnostic: String,
    // Last admitted editor input; distinct from the one-shot discovery queue and
    // the authenticated persisted endpoint. Unpaired drafts last for this run.
    address_draft: Option<String>,
    discovered: Vec<Value>,
    after_reply: Vec<(String, AfterReply, u64)>,
    effects: Vec<AfterReply>,
}
impl Remote {
    /// Public correlation generation only; configuration secrets never leave Remote.
    pub(crate) fn authority_generation(&self) -> u64 { self.generation }

    /// No initial/retry load remains pending. A terminal load failure also
    /// settles this boundary; this does not imply persistence or authorization.
    pub(crate) fn configuration_load_settled(&self) -> bool { !self.loading }

    /// Revoke disk authority before dropping any asynchronous control work.
    pub(crate) fn retire_for_erasure(&mut self) {
        self.notifications.suspend();
        self.write_permit.revoke();
        self.clear_discovery();
        self.flight.take();
        self.pending_save = None;
        self.pairing = None;
        self.cleanup = None;
        self.after_reply.clear();
        self.effects.clear();
    }

    /// Deferred work must lose its authority when the control configuration changes.

    pub fn from_install(device_id: &str, runtime: &str) -> Self {
        let config = Config::disabled(device_id.to_owned());
        Self {
            notifications: Box::default(),
            config,
            write_permit: plx_platform::storage::private::WritePermit::new(),
            runtime: runtime.into(),
            generation: 1,
            flight: None,
            delivery: Delivery::default(),
            pairing: None,
            cleanup: None,
            pending_save: None,
            loading: true,
            confirm_loaded_config: false,
            next_ms: 0,
            failures: 0,
            status: "loading",
            message: "Loading remote control",
            network_diagnostic: String::new(),
            persisted: false,
            epoch: String::new(),
            presence_sequence: 0,
            presence_at: 0,
            presence_pending: None,
            configuration_receipt: None,
            discover_address: None,
            lan_pending: false,
            lan_deadline: None,
            catalog_profile: CatalogProfile::configured(),
            find_state: "",
            find_message: "",
            find_diagnostic: String::new(),
            address_draft: None,
            discovered: Vec::new(),
            after_reply: Vec::new(),
            effects: Vec::new(),
        }
    }
    #[cfg(test)]
    pub(crate) fn disabled_for_test(device_id: &str, runtime: &str) -> Self {
        let mut remote = Self::from_install(device_id, runtime);
        remote.loading = false;
        remote.persisted = true;
        remote.status = "disabled";
        remote.message = "Remote control disabled";
        remote
    }
    /// Use the real envelope parser for test commands without starting transport.
    #[cfg(test)]
    pub(crate) fn request_for_test(&mut self, action: &str, params: Value) -> Invocation {
        let mut invocation = self.delivery.poll(&json!({"commands":[],"server_time":1000,"requests":[{
            "id":"0123456789abcdef0123456789abcdef","action":action,"params":params,"expires_at":1020
        }]}),100,0).unwrap().remove(0);
        invocation.generation = self.generation;
        invocation
    }

    /// Cross-crate application fixtures use the actual raw transport completion.
    /// This does not start a request, worker, socket, or persistence operation.
    #[cfg(all(test, feature = "hostsim"))]
    pub(crate) fn raw_poll_for_test(&mut self, body: &str, now_ms: u64) -> Vec<Invocation> {
        let mut actions=self.finish(
            Kind::Delivery(SendKind::Poll),
            Outcome::Http {
                response: Ok(protocol::Response { status:200, body:body.as_bytes().to_vec() }),
                elapsed_ms:0,
            },
            now_ms,
        );
        for action in &mut actions { action.generation=self.generation; }
        actions
    }

    /// Complete the real initial-load task through tick without opening storage.
    #[cfg(all(test, feature = "hostsim"))]
    pub(crate) fn queue_load_for_test(&mut self, result: Result<(Config, bool, String), &'static str>) {
        assert!(self.loading && self.flight.is_none());
        let (task, sender) = transport::Task::pending_for_test();
        sender.send(Outcome::Loaded(result)).unwrap_or_else(|_| panic!("fixture load receiver disappeared"));
        self.flight = Some(Flight { kind: Kind::Load, generation: self.generation, task });
    }

    /// Stage a hash-bound public catalog through real Find admission/completion.
    /// Only test bytes are used; no transport or protected writer is launched.
    #[cfg(all(test, feature = "hostsim"))]
    pub(crate) fn find_catalog_for_test(&mut self, body: &[u8], now_ms: u64) -> Result<(), &'static str> {
        let digest = plx_base::sha256::sha256(body).iter()
            .map(|byte| format!("{byte:02x}")).collect::<String>();
        self.catalog_profile = CatalogProfile::from_config(
            Some("https://catalog.invalid/discovery.json"), Some(&digest));
        self.queue_find(now_ms, false)?;
        let Some(DiscoveryTarget::CatalogUrl(profile)) = self.discover_address.take() else {
            return Err("Fixture catalog was not queued");
        };
        self.finish(Kind::Catalog(profile), Outcome::Http {
            response: Ok(protocol::Response { status: 200, body: body.to_vec() }), elapsed_ms: 0,
        }, now_ms);
        Ok(())
    }

    /// Complete manual discovery through real admission and response handling, without IO.
    #[cfg(all(test, feature = "hostsim"))]
    pub(crate) fn find_manual_for_test(&mut self, address: &str, body: &[u8], now_ms: u64)
        -> Result<(), &'static str>
    {
        self.discover_and_pair(address, now_ms)?;
        let Some(DiscoveryTarget::ControllerBase(_)) = self.discover_address.take() else {
            return Err("Fixture manual discovery was not queued");
        };
        self.finish(Kind::Discovery, Outcome::Http {
            response: Ok(protocol::Response { status: 200, body: body.to_vec() }), elapsed_ms: 0,
        }, now_ms);
        Ok(())
    }

    /// Inspect the actual delivery result, without starting transport.
    #[cfg(all(test, feature = "hostsim"))]
    pub(crate) fn response_for_id_for_test(&self, id: &str) -> Option<Value> {
        self.delivery.response_for_id_for_test(id)
    }

    /// Stable receiver identity is unavailable until its configuration is confirmed.
    pub(crate) fn persisted_device_id(&self) -> Option<&str> {
        (!self.loading && self.persisted && self.pending_save.is_none())
            .then_some(self.config.device_id.as_str())
    }

    pub fn device_id(&self) -> &str {
        &self.config.device_id
    }
    pub fn runtime(&self) -> &str {
        &self.runtime
    }
    /// Validate the discovery input without changing the active pairing or IO.
    pub(crate) fn normalize_address(address: &str) -> Result<String, &'static str> {
        protocol::normalize_base(address, false)
    }
    /// Hold the existing transport slot in a regression without opening IO.
    #[cfg(all(test, feature = "hostsim"))]
    pub(crate) fn hold_transport_for_test(&mut self) -> impl Send {
        let (task, sender) = transport::Task::pending_for_test();
        self.flight = Some(Flight { kind: Kind::Discovery, generation: self.generation, task });
        sender
    }
    /// Prefill only the addressed settings editor; do not publish the endpoint
    /// in status snapshots. A new process falls back to its paired configuration.
    pub fn address_input(&self) -> &str {
        self.address_draft.as_deref().unwrap_or(&self.config.address)
    }
    /// Nonsecret state for native settings and status; never serializes endpoint tokens.
    pub fn snapshot(&self) -> Value {
        let message = if self.network_diagnostic.is_empty() { self.message.to_owned() }
            else { format!("{} · {}", self.message, self.network_diagnostic) };
        json!({"version":1,"enabled":self.config.enabled,"device_id":self.config.device_id,
            "state":self.status,"message":message,"persisted":self.persisted,
            "find_configured": self.persisted && !self.loading && self.write_permit.is_valid(),
            "find_unavailable": match &self.catalog_profile {
                Ok(Some(_)) => "", Ok(None) => discovery::UNCONFIGURED, Err(_) => discovery::INVALID_PROFILE,
            },
            "find_state":self.find_state,"find_message":self.find_message,"find_diagnostic":self.find_diagnostic,
            "pairing_server":self.pairing.as_ref().map(|p|p.base.as_str()).unwrap_or(""),
            "servers":self.discovered,"pairing_code":self.pairing.as_ref().map(|p|p.code.as_str()).unwrap_or(""),
            "saving":self.pending_save.is_some() || matches!(self.flight.as_ref().map(|f|&f.kind),Some(Kind::Save(_)))})
    }
    /// A failed initial load may be retried only after its single worker has left.
    pub fn can_retry_load(&self) -> bool {
        self.write_permit.is_valid()
            && !self.persisted
            && !self.loading
            && self.status == "error"
            && self.flight.is_none()
            && self.pending_save.is_none()
    }
    /// Admit one new protected-state load; tick owns the existing IO task.
    pub fn retry_load(&mut self) -> Result<(), &'static str> {
        if !self.can_retry_load() {
            return Err("Control configuration cannot be retried now");
        }
        self.invalidate();
        self.loading = true;
        self.confirm_loaded_config = true;
        self.status = "loading";
        self.message = "Loading remote control";
        Ok(())
    }
    /// Durable settings change. A return of Ok is admission, not a durability receipt.
    pub fn disable(&mut self) -> Result<(), &'static str> {
        if self.loading || !self.persisted {
            return Err("Control configuration is not ready");
        }
        let mut config = self.config.clone();
        config.enabled = false;
        self.configure(serde_json::to_value(config).map_err(|_| "Invalid control configuration")?)
    }
    pub fn configure(&mut self, value: Value) -> Result<(), &'static str> {
        let config = Config::parse(value)?;
        if config.device_id != self.config.device_id {
            return Err("The installation device ID cannot be changed");
        }
        if self.pending_save.is_some()
            || matches!(self.flight.as_ref().map(|f| &f.kind), Some(Kind::Save(_)))
        {
            return Err("A control settings save is in progress");
        }
        self.invalidate();
        if let Some(pairing) = self.pairing.take() {
            if !pairing.id.is_empty() {
                self.cleanup = Some(pairing.request(true));
            }
        }
        self.loading = false;
        self.pending_save = Some(config);
        self.status = "saving";
        self.message = "Saving protected control configuration";
        Ok(())
    }
    /// Look up the server's published identity; no guessed DNS instance IDs.
    pub fn discover_and_pair(&mut self, address: &str, now_ms: u64) -> Result<(), &'static str> {
        if self.loading
            || !self.persisted
            || self.pending_save.is_some()
            || matches!(self.flight.as_ref().map(|f| &f.kind), Some(Kind::Save(_)))
        {
            return Err("Control configuration is not ready");
        }
        let base = ControllerBase::new(address)?;
        // normalize_base already bounds this trimmed input to 2048 bytes and
        // rejects credentials/query data. Keep it even if discovery later fails.
        self.address_draft = Some(address.trim().trim_end_matches('/').to_owned());
        self.cancel_pairing();
        self.discovered.clear();
        self.discover_address = Some(DiscoveryTarget::ControllerBase(base));
        self.status = "discovering";
        self.message = "Finding command server";
        self.next_ms = now_ms;
        Ok(())
    }
    /// DNS-SD, like a public catalog, never pairs until the user selects a row.
    pub fn find_servers(&mut self, now_ms: u64) -> Result<(), &'static str> {
        self.queue_find(now_ms, plx_net::control_discovery::supported())
    }
    fn queue_find(&mut self, now_ms: u64, lan: bool) -> Result<(), &'static str> {
        if self.loading || !self.persisted || !self.write_permit.is_valid()
            || self.pending_save.is_some()
            || matches!(self.flight.as_ref().map(|f| &f.kind), Some(Kind::Save(_)))
        { return Err("Control configuration is not ready"); }
        // A timed-out blocking resolver still owns the one transport slot.
        if matches!(self.flight.as_ref().map(|f| &f.kind), Some(Kind::LanDiscovery)) {
            return Err("Discovery is still finishing. Try again shortly.");
        }
        let catalog = if lan { None } else {
            Some(self.catalog_profile.clone()?.ok_or(discovery::UNCONFIGURED)?)
        };
        self.cancel_pairing();
        self.discover_address = catalog.map(DiscoveryTarget::CatalogUrl);
        self.lan_pending = lan;
        self.lan_deadline = lan.then(|| now_ms.saturating_add(plx_net::control_discovery::WINDOW_MS));
        self.find_state = "discovering";
        self.find_message = discovery::FINDING;
        self.next_ms = now_ms;
        Ok(())
    }
    /// The opaque token refers only to this generation's immutable validated table.
    pub fn select_discovered_choice(&mut self, choice: &str, now_ms: u64) -> Result<(), &'static str> {
        let index = (choice.len() <= 32 && !choice.is_empty()).then(||
            self.discovered.iter().position(|row| row["choice"].as_str() == Some(choice))).flatten();
        if let Some(index) = index { return self.select_discovered(index, now_ms); }
        // Surface this explicit input failure above background control health.
        // Do not disturb a newer pairing after Find has already relinquished ownership.
        if !self.find_message.is_empty() {
            self.fail_find();
            self.find_message = discovery::EXPIRED_CHOICE;
        }
        Err(discovery::EXPIRED_CHOICE)
    }
    fn select_discovered(&mut self, index: usize, now_ms: u64) -> Result<(), &'static str> {
        let selected = self
            .discovered
            .get(index)
            .cloned()
            .ok_or("Select a discovered server")?;
        self.begin_pairing(
            selected["address"].as_str().unwrap_or(""),
            selected["id"].as_str().unwrap_or(""),
            now_ms,
        )
    }
    pub fn begin_pairing(
        &mut self,
        address: &str,
        server_id: &str,
        now_ms: u64,
    ) -> Result<(), &'static str> {
        if self.loading
            || !self.persisted
            || self.pending_save.is_some()
            || matches!(self.flight.as_ref().map(|f| &f.kind), Some(Kind::Save(_)))
        {
            return Err("Control configuration is not ready");
        }
        let pairing = Pairing::new(address, server_id, &self.config.device_id, now_ms)?;
        self.cancel_pairing();
        self.pairing = Some(pairing);
        self.status = "pairing";
        self.message = "Requesting pairing approval";
        self.next_ms = 0;
        Ok(())
    }
    pub fn cancel_pairing(&mut self) {
        // There is no pairing authority before bootstrap completes. In
        // particular, Cancel must not turn a storage failure into "disabled".
        if !self.persisted {
            return;
        }
        if self.pending_save.is_some()
            || matches!(self.flight.as_ref().map(|f| &f.kind), Some(Kind::Save(_)))
        {
            return;
        }
        self.discover_address = None;
        self.discovered.clear();
        if let Some(pairing) = self.pairing.take() {
            if !pairing.id.is_empty() {
                self.cleanup = Some(pairing.request(true));
            }
        }
        self.invalidate();
        self.status = if self.config.enabled {
            "connecting"
        } else {
            "disabled"
        };
        self.message = "Pairing canceled";
    }
    pub fn take_configuration_receipt(&mut self) -> Option<Value> {
        self.configuration_receipt.take()
    }
    fn clear_discovery(&mut self) {
        self.discover_address = None;
        self.lan_pending = false;
        self.lan_deadline = None;
        self.discovered.clear();
        self.find_state = "";
        self.find_message = "";
        self.find_diagnostic.clear();
    }
    fn publish_candidates(&mut self, candidates: Vec<discovery::Candidate>) {
        self.discovered = candidates.into_iter().enumerate().map(|(index, row)| {
            // Labels are display metadata; identity and the pairing endpoint remain exact.
            let domain = display::text(&row.domain, 253).unwrap_or_default();
            json!({"id":row.id,"domain":domain,"address":row.address,
                "choice":format!("{:016x}:{:02x}",self.generation,index)})
        }).collect();
    }
    fn fail_find(&mut self) {
        self.discovered.clear();
        self.find_state = "error";
        self.find_message = discovery::FAILED;
        self.find_diagnostic.clear();
    }
    fn invalidate(&mut self) {
        self.notifications.suspend();
        self.clear_discovery();
        self.network_diagnostic.clear();
        self.generation = self.generation.wrapping_add(1);
        if let Some(flight) = &self.flight {
            flight.task.cancel();
        }
        // Keep the task slot until its worker exits: repeated settings edits cannot
        // start unbounded concurrent blocking requests.
        self.epoch.clear();
        self.presence_pending = None;
        self.after_reply.clear();
        self.effects.clear();
        self.next_ms = 0;
        self.failures = 0;
    }
    fn apply(&mut self, config: Config, persisted: bool) {
        self.notifications.suspend();
        self.clear_discovery();
        self.network_diagnostic.clear();
        self.config = config;
        self.persisted = persisted;
        self.delivery = Delivery::default();
        self.pairing = None;
        self.generation = self.generation.wrapping_add(1);
        self.epoch.clear();
        self.presence_pending = None;
        self.presence_sequence = 0;
        self.presence_at = 0;
        self.next_ms = 0;
        self.failures = 0;
        self.status = if self.config.enabled {
            "connecting"
        } else {
            "disabled"
        };
        self.message = if self.config.enabled {
            "Connecting to command server"
        } else {
            "Remote control disabled"
        };
    }
    fn fail(&mut self, now: u64, message: &'static str) {
        self.network_diagnostic.clear();
        self.failures = self.failures.saturating_add(1);
        self.next_ms = now.saturating_add((1000u64 << self.failures.min(5)).min(30000));
        self.status = "error";
        self.message = message;
    }
    fn launch(&mut self, kind: Kind, task: Option<Task>, now: u64) {
        if let Some(task) = task {
            self.flight = Some(Flight {
                kind,
                generation: self.generation,
                task,
            });
        } else {
            if matches!(kind, Kind::Load) {
                self.loading = false;
            }
            if matches!(kind, Kind::Catalog(_) | Kind::LanDiscovery) { self.fail_find(); }
            else { self.fail(now, "Remote control worker unavailable"); }
        }
    }
    /// Called on the frame thread with a monotonic clock. Returned actions are executed
    /// on that same thread through the native player dispatcher, then complete() is called.
    pub fn tick(&mut self, now_ms: u64, player_state: Value) -> Vec<Invocation> {
        if !self.write_permit.is_valid() { return Vec::new(); }
        let mut actions = Vec::new();
        if self.lan_deadline.is_some_and(|deadline| now_ms >= deadline) {
            self.lan_deadline = None;
            self.lan_pending = false;
            self.fail_find();
            self.find_diagnostic = "Local discovery timed out".into();
            // Do not drop/cancel the flight: libc cannot interrupt a DNS call.
            // Its result is retired below; a repeated Find cannot spawn a worker.
        }
        self.after_reply
            .retain(|(_, _, deadline)| *deadline > now_ms);
        if let Some(flight) = self.flight.take() {
            match flight.task.poll() {
                Err(TryRecvError::Empty) => {
                    self.flight = Some(flight);
                    return actions;
                }
                result if flight.generation != self.generation
                    || (matches!(&flight.kind, Kind::LanDiscovery) && self.lan_deadline.is_none()) => {
                    drop(result);
                }
                Err(TryRecvError::Disconnected) => {
                    if matches!(&flight.kind, Kind::Load) {
                        self.loading = false;
                    }
                    if matches!(&flight.kind, Kind::Catalog(_) | Kind::LanDiscovery) { self.fail_find(); }
                    else { self.fail(now_ms, "Remote control request failed"); }
                }
                Ok(outcome) => actions = self.finish(flight.kind, outcome, now_ms),
            }
        }
        // Dispatch before another request is launched, so its result can be queued first.
        if !actions.is_empty() {
            for action in &mut actions {
                action.generation = self.generation;
            }
            return actions;
        }
        if now_ms < self.next_ms {
            return actions;
        }
        if self.loading {
            self.launch(
                Kind::Load,
                transport::load(self.config.device_id.clone(), self.runtime.clone(),
                    self.write_permit.clone(), self.confirm_loaded_config),
                now_ms,
            );
        } else if let Some(config) = self.pending_save.take() {
            let task = transport::save(config.clone(), self.write_permit.clone());
            if task.is_none() {
                self.pending_save = Some(config.clone());
            }
            self.launch(Kind::Save(config), task, now_ms);
        } else if let Some(request) = self.cleanup.take() {
            self.launch(Kind::CancelPairing, transport::send(request), now_ms);
        } else if self.lan_pending {
            self.lan_pending = false;
            self.launch(Kind::LanDiscovery, transport::discover(), now_ms);
        } else if let Some(target) = self.discover_address.take() {
            let request = target.request();
            let limit = target.body_limit();
            let kind = match target {
                DiscoveryTarget::ControllerBase(_) => Kind::Discovery,
                DiscoveryTarget::CatalogUrl(profile) => Kind::Catalog(profile),
            };
            self.launch(kind, transport::send_bounded(request, limit), now_ms);
        } else if let Some(pairing) = &self.pairing {
            if now_ms >= pairing.expires_ms {
                self.cancel_pairing();
                self.status = "error";
                self.message = "Pairing expired";
            } else if now_ms >= pairing.next_ms {
                self.launch(
                    Kind::Pairing,
                    transport::send(pairing.request(false)),
                    now_ms,
                );
            }
        } else if self.config.enabled {
            let (kind, request) =
                if self.delivery.idle() && !self.epoch.is_empty() && now_ms >= self.presence_at {
                    if self.presence_pending.is_none() {
                        let body = self.presence(&player_state);
                        if body.to_string().len() <= 4096 {
                            self.presence_pending = Some(body);
                        } else {
                            self.presence_at = now_ms.saturating_add(15000);
                        }
                    }
                    if let Some(body) = &self.presence_pending {
                        (
                            SendKind::Presence,
                            Request::new(
                                &self.config,
                                "POST",
                                "/api/player-state",
                                Some(body.clone()),
                            ),
                        )
                    } else {
                        self.delivery.next(&self.config)
                    }
                } else {
                    self.delivery.next(&self.config)
                };
            self.launch(Kind::Delivery(kind), transport::send(request), now_ms);
        }
        actions
    }
    pub(crate) fn tick_notifications(&mut self, now: u64, enabled: bool, active: bool) {
        let config = (self.persisted && !self.loading && self.pending_save.is_none()
            && !matches!(self.flight.as_ref().map(|flight| &flight.kind), Some(Kind::Save(_)))
            && self.write_permit.is_valid() && self.config.enabled)
            .then_some(&self.config);
        self.notifications.tick(config, self.generation, &self.runtime, enabled, active, now);
    }
    pub(crate) fn suspend_notifications(&mut self) { self.notifications.suspend(); }
    pub(crate) fn notifications_need_frame(&self, now: u64) -> bool { self.notifications.needs_frame(now) }
    pub(crate) fn draw_notifications(&mut self, now: u64, opacity: f32) {
        self.notifications.draw(now, opacity, false, &plx_ui::widgets::LegacyMeasure);
    }
    #[cfg(all(test, feature = "hostsim"))]
    pub(crate) fn show_notification_for_test(&mut self, now: u64) { self.notifications.show_for_test(now); }
    pub fn invocation_is_current(&self, invocation: &Invocation) -> bool {
        invocation.generation == self.generation
    }
    pub fn complete(&mut self, invocation: Invocation, result: Value) {
        if invocation.generation != self.generation {
            return;
        }
        self.delivery.complete(invocation, result);
        self.next_ms = 0;
    }
    pub fn complete_after_reply(
        &mut self,
        invocation: Invocation,
        result: Value,
        effect: AfterReply,
    ) {
        if invocation.generation != self.generation {
            return;
        }
        let id = invocation.id.clone();
        let deadline = invocation.deadline_ms;
        if self.delivery.complete(invocation, result) && self.after_reply.len() < 50 {
            self.after_reply.push((id, effect, deadline));
        }
        self.next_ms = 0;
    }
    pub fn take_effects(&mut self) -> Vec<AfterReply> {
        std::mem::take(&mut self.effects)
    }
    fn presence(&mut self, state: &Value) -> Value {
        self.presence_sequence = self.presence_sequence.saturating_add(1);
        // Projection prevents a rich local status snapshot from leaking provider URLs,
        // credentials, or unsupported metadata into the server's closed presence schema.
        let p = &state["player"];
        let mut playback = project_playback(&state["playback"]);
        // Live status permits generated catalog IDs; retained player-state v1
        // still requires simple IDs. Omit an unsupported ID, not the whole report.
        playback["media_id"] = playback["media_id"]
            .as_str()
            .and_then(display::retained_media_id)
            .map_or(Value::Null, |id| json!(id));
        let platform = display_text(&p["platform"], 32);
        let platform = if platform.is_null() {
            json!("webos-native")
        } else {
            platform
        };
        let version = display_text(&p["version"], 64);
        let version = if version.is_null() {
            json!(env!("CARGO_PKG_VERSION"))
        } else {
            version
        };
        let standby = playback["state"] == "standby";
        json!({"version":1,"controller_epoch":self.epoch,"runtime":self.runtime,"sequence":self.presence_sequence,
            "player":{"platform":platform,"version":version,"standby":standby},"playback":playback})
    }
    fn advertisement(&mut self, value: &Value) {
        let a = &value["player_state"];
        let epoch = a["controller_epoch"].as_str().unwrap_or("");
        let valid = a["version"] == 1
            && a["report_path"] == "/api/player-state"
            && a["heartbeat_ms"] == 15000
            && a["stale_after_ms"] == 45000
            && a["max_body_bytes"] == 4096
            && epoch.len() == 32
            && epoch
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !valid {
            self.epoch.clear();
            self.presence_pending = None;
            return;
        }
        if epoch != self.epoch {
            self.epoch = epoch.into();
            self.presence_sequence = 0;
            self.presence_at = 0;
            self.presence_pending = None;
        }
    }
    fn finish(&mut self, kind: Kind, outcome: Outcome, now: u64) -> Vec<Invocation> {
        let catalog = matches!(&kind, Kind::Catalog(_));
        let diagnostic = match (&kind, &outcome) {
            (Kind::CancelPairing, _) => None,
            (_, Outcome::Http { response, .. }) => Some(match response {
                Err(failure) => transport::failure_message(failure),
                Ok(response) if response.status != 200
                    && !(matches!(&kind, Kind::Pairing) && matches!(response.status, 201 | 202)) =>
                {
                    format!("HTTP {}", response.status)
                }
                Ok(_) => String::new(),
            }),
            _ => None,
        };
        let actions = self.finish_outcome(kind, outcome, now);
        // Pairing errors can invalidate the flow. Retain the safe evidence from
        // this result after that transition; cleanup responses cannot erase it.
        if let Some(diagnostic) = diagnostic {
            if catalog { self.find_diagnostic = diagnostic; }
            else { self.network_diagnostic = diagnostic; }
        }
        actions
    }
    fn finish_outcome(&mut self, kind: Kind, outcome: Outcome, now: u64) -> Vec<Invocation> {
        let mut actions = Vec::new();
        match (kind, outcome) {
            (Kind::Load, Outcome::Loaded(result)) => {
                self.loading = false;
                self.confirm_loaded_config = false;
                match result {
                    Ok((config, saved, runtime)) => {
                        self.runtime = runtime;
                        self.apply(config, saved);
                        // A persisted public receiver ID supports first-time registration.
                        // Never log provisional identity, control addresses or credentials.
                        if let Some(id) = self.persisted_device_id() {
                            plx_base::eventlog::log(&format!("ott-control: device_id={id}"));
                        }
                    }
                    Err(message) => {
                        self.status = "error";
                        self.message = message;
                    }
                }
            }
            (Kind::Save(config), Outcome::Saved(result)) => match result {
                Ok(()) => {
                    self.apply(config, true);
                    self.configuration_receipt =
                        Some(json!({"status":"ok","data":{"persisted":true}}));
                }
                Err(message) => {
                    self.pairing = None;
                    self.status = "error";
                    self.message = message;
                    self.configuration_receipt =
                        Some(json!({"status":"rejected","data":{"error":message}}));
                }
            },
            (Kind::CancelPairing, _) => {}
            (Kind::LanDiscovery, Outcome::LanDiscovery(result)) => {
                if self.lan_deadline.take().is_none() { return actions; }
                match result {
                    Ok(candidates) if candidates.is_empty() => {
                        // Only a successful empty DNS result can use the optional
                        // pinned catalog; malformed/timeout/unsupported never can.
                        match self.catalog_profile.clone() {
                            Ok(Some(profile)) => {
                                self.discover_address = Some(DiscoveryTarget::CatalogUrl(profile));
                                self.next_ms = now;
                            }
                            Ok(None) => {
                                self.find_state = "empty";
                                self.find_message = discovery::EMPTY;
                            }
                            Err(_) => self.fail_find(),
                        }
                    }
                    Ok(candidates) => {
                        self.publish_candidates(candidates.into_iter().map(|row| discovery::Candidate {
                            id:row.id, domain:row.domain, address:row.address,
                        }).collect());
                        self.find_state = "choose";
                        self.find_message = discovery::CHOOSE;
                    }
                    Err(_) => self.fail_find(),
                }
            }
            (Kind::Catalog(profile), Outcome::Http { response, .. }) => {
                let parsed = response.ok().filter(|r| r.status == 200)
                    .ok_or("Discovery request failed")
                    .and_then(|r| profile.parse(&r.body));
                match parsed {
                    Ok(candidates) => {
                        self.publish_candidates(candidates);
                        let empty = self.discovered.is_empty();
                        self.find_state = if empty { "empty" } else { "choose" };
                        self.find_message = if empty { discovery::EMPTY } else { discovery::CHOOSE };
                    }
                    Err(_) => self.fail_find(),
                }
            }
            (Kind::Discovery, Outcome::Http { response, .. }) => {
                let parsed = response.ok().filter(|r| r.status == 200)
                    .ok_or("No command server was discovered")
                    .and_then(|r| discovery::parse_controller(&r.body));
                match parsed {
                    Ok(candidates) if !candidates.is_empty() => {
                        self.publish_candidates(candidates);
                        self.status = "choose";
                        self.message = "Select command server";
                        if self.discovered.len() == 1 {
                            if let Err(message) = self.select_discovered(0, now) {
                                self.status = "error";
                                self.message = message;
                            }
                        }
                    }
                    Ok(_) => {
                        self.discovered.clear();
                        self.status = "error";
                        self.message = "No command server was discovered";
                    }
                    Err(message) => {
                        self.discovered.clear();
                        self.status = "error";
                        self.message = message;
                    }
                }
            }
            (Kind::Pairing, Outcome::Http { response, .. }) => {
                let Some(pairing) = &mut self.pairing else {
                    return actions;
                };
                let Ok(response) = response else {
                    pairing.failures = pairing.failures.saturating_add(1);
                    if pairing.failures >= 6 {
                        self.cancel_pairing();
                        self.status = "error";
                        self.message = "Pairing server is unavailable";
                    } else {
                        pairing.next_ms = now.saturating_add(u64::from(pairing.failures) * 2000);
                    }
                    return actions;
                };
                let value = serde_json::from_slice(&response.body).unwrap_or(Value::Null);
                match pairing.accept(response.status, &value, now) {
                    Ok(PairResult::Pending) => {
                        self.status = "waiting";
                        self.message = "Approve the pairing code with ott";
                    }
                    Ok(PairResult::Approved(config)) => {
                        self.cleanup = Some(pairing.request(true));
                        self.pairing = None;
                        self.pending_save = Some(config);
                        self.status = "saving";
                        self.message = "Saving protected pairing";
                    }
                    Err(message) => {
                        self.cancel_pairing();
                        self.status = "error";
                        self.message = message;
                    }
                }
            }
            (
                Kind::Delivery(kind),
                Outcome::Http {
                    response,
                    elapsed_ms,
                },
            ) => {
                let Ok(response) = response else {
                    self.fail(now, "Command server unavailable; retrying");
                    return actions;
                };
                if matches!(kind, SendKind::Response) && response.status == 404 {
                    if let Some(id) = self.delivery.expired_response() {
                        self.after_reply.retain(|(pending, _, _)| pending != &id);
                    }
                    self.next_ms = now.saturating_add(1000);
                    return actions;
                }
                if response.status != 200 {
                    if matches!(kind, SendKind::Presence) {
                        self.presence_pending = None;
                        self.epoch.clear();
                        self.next_ms = now.saturating_add(1000);
                    } else {
                        self.fail(
                            now,
                            if response.status == 401 || response.status == 403 {
                                "Command server access denied"
                            } else {
                                "Command server unavailable; retrying"
                            },
                        );
                    }
                    return actions;
                }
                let parsed = match &kind {
                    SendKind::Poll => ingress::decode_poll(&response.body)
                        .map(|poll| (poll.value, poll.media_tracks_raw, poll.plex_servers_raw, poll.provider_settings_raw)),
                    _ => serde_json::from_slice::<Value>(&response.body)
                        .map(|value| (value, None, None, None)),
                };
                let Ok((value, media_tracks_raw, plex_servers_raw, provider_settings_raw)) = parsed else {
                    self.fail(now, "Invalid command server response");
                    return actions;
                };
                let valid = match &kind {
                    SendKind::Poll => match self.delivery.poll(&value, now, elapsed_ms) {
                        Ok(mut incoming) => {
                            // Delivery admits at most the first request. Its exact
                            // projection was validated before any ID was remembered.
                            for invocation in &mut incoming {
                                if invocation.expects_response() && invocation.action == "media_tracks" {
                                    invocation.media_tracks_raw = media_tracks_raw.clone();
                                }
                                if invocation.expects_response() && invocation.action == "plex_servers" {
                                    invocation.plex_servers_raw = plex_servers_raw.clone();
                                }
                                if invocation.expects_response() && matches!(invocation.action.as_str(), "settings_compare" | "provider_settings") {
                                    invocation.provider_settings_raw = provider_settings_raw.clone();
                                }
                            }
                            actions = incoming;
                            self.advertisement(&value);
                            true
                        }
                        Err(_) => false,
                    },
                    SendKind::Presence => {
                        let matches = self.presence_pending.as_ref().is_some_and(|p| {
                            value["version"] == 1
                                && value["controller_epoch"] == p["controller_epoch"]
                                && value["runtime"] == p["runtime"]
                                && value["sequence"] == p["sequence"]
                        });
                        self.presence_pending = None;
                        self.presence_at = now.saturating_add(15000);
                        matches
                    }
                    _ => match self.delivery.accepted(&kind, &value) {
                        Ok(completed) => {
                            if let Some(id) = completed {
                                if let Some(index) = self
                                    .after_reply
                                    .iter()
                                    .position(|(pending, _, _)| *pending == id)
                                {
                                    let (_, effect, deadline) = self.after_reply.remove(index);
                                    if now < deadline {
                                        self.effects.push(effect);
                                    }
                                }
                            }
                            true
                        }
                        Err(_) => false,
                    },
                };
                if !valid {
                    self.fail(now, "Invalid command server acknowledgement");
                } else {
                    self.failures = 0;
                    self.status = "connected";
                    self.message = "Remote control connected";
                    self.next_ms = if self.delivery.idle() {
                        now.saturating_add(1000)
                    } else {
                        now
                    };
                }
            }
            (Kind::Catalog(_) | Kind::LanDiscovery, _) => self.fail_find(),
            _ => self.fail(now, "Invalid control worker response"),
        }
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_address_input_falls_back_to_persisted_paired_config() {
        let mut remote = Remote::disabled_for_test("dev_test", "runtime");
        assert_eq!(remote.address_input(), "");
        let config = Config::parse(json!({
            "enabled": true,
            "address": "https://paired.invalid/control/",
            "token": "A".repeat(32),
            "device_id": "dev_test"
        }))
        .unwrap();
        let expected = config.address.clone();
        remote.apply(config, true);
        assert_eq!(remote.address_input(), expected);
        let snapshot = remote.snapshot().to_string();
        assert!(!snapshot.contains("paired.invalid"));
        assert!(!snapshot.contains(&"A".repeat(32)));
    }

    #[test]
    fn remote_address_draft_survives_discovery_and_pairing_failure_without_changing_active_config() {
        let mut remote = Remote::disabled_for_test("dev_test", "runtime");
        let config = Config::parse(json!({
            "enabled": true,
            "address": "https://paired.invalid/control",
            "token": "A".repeat(32),
            "device_id": "dev_test"
        }))
        .unwrap();
        let before = serde_json::to_value(&config).unwrap();
        remote.apply(config, true);
        let input = "https://new-server.invalid/long/control/path";
        remote.discover_and_pair(input, 100).unwrap();
        assert_eq!(remote.address_input(), input);
        // This is the queue consumption performed by tick, without launching IO.
        assert!(remote.discover_address.take().is_some());
        remote.finish(Kind::Discovery, response(503, Value::Null), 200);
        assert_eq!(remote.snapshot()["state"], "error");
        assert_eq!(remote.address_input(), input);
        remote.begin_pairing(input, "fixture-server", 300).unwrap();
        remote.finish(Kind::Pairing, response(403, Value::Null), 400);
        assert_eq!(remote.snapshot()["state"], "error");
        assert_eq!(remote.address_input(), input);
        assert_eq!(serde_json::to_value(&remote.config).unwrap(), before);
        assert!(remote.pending_save.is_none());
        assert!(remote.flight.is_none());
        assert!(!remote.snapshot().to_string().contains("new-server.invalid"));
    }

    #[test]
    fn invalid_remote_address_never_replaces_the_last_accepted_editor_input() {
        let mut remote = Remote::disabled_for_test("dev_test", "runtime");
        let accepted = "https://accepted.invalid/control";
        remote.discover_and_pair(accepted, 100).unwrap();
        let queued = remote.discover_address.clone();
        for invalid in [
            "http://public.invalid/control".to_owned(),
            "https://accepted.invalid/?token=private".to_owned(),
            "https://user:password@accepted.invalid/".to_owned(),
            format!("https://accepted.invalid/{}", "x".repeat(2049)),
        ] {
            assert!(remote.discover_and_pair(&invalid, 200).is_err());
            assert_eq!(remote.address_input(), accepted);
            assert_eq!(remote.discover_address, queued);
            assert!(!remote.config.enabled);
            assert!(remote.config.token.is_empty());
        }
        remote.cancel_pairing();
        assert_eq!(remote.address_input(), accepted);
    }

    fn request(remote: &mut Remote) -> Invocation {
        remote.request_for_test("lifecycle", json!({"operation":"restart_app"}))
    }
    fn response(status: u16, value: Value) -> Outcome {
        Outcome::Http {
            response: Ok(protocol::Response {
                status,
                body: serde_json::to_vec(&value).unwrap(),
            }),
            elapsed_ms: 10,
        }
    }
    #[test]
    fn destructive_effect_waits_for_server_ack_and_runs_once() {
        let mut remote = Remote::from_install("dev_test", "runtime");
        let call = request(&mut remote);
        remote.complete_after_reply(
            call,
            json!({"status":"ok","data":{"accepted":true}}),
            AfterReply::Exit,
        );
        assert!(remote.take_effects().is_empty());
        remote.finish(
            Kind::Delivery(SendKind::Response),
            response(500, Value::Null),
            200,
        );
        assert!(remote.take_effects().is_empty());
        remote.finish(
            Kind::Delivery(SendKind::Response),
            response(200, json!({"status":"ok"})),
            300,
        );
        assert_eq!(remote.take_effects(), vec![AfterReply::Exit]);
        remote.finish(
            Kind::Delivery(SendKind::Response),
            response(200, json!({"status":"ok"})),
            400,
        );
        assert!(remote.take_effects().is_empty());
    }
    #[test]
    fn expired_server_receipt_cannot_execute_after_replayed_response_is_accepted() {
        let mut remote = Remote::from_install("dev_test", "runtime");
        let call = request(&mut remote);
        let id = call.id.clone();
        remote.complete_after_reply(
            call,
            json!({"status":"ok","data":{"accepted":true}}),
            AfterReply::Exit,
        );
        remote.finish(
            Kind::Delivery(SendKind::Response),
            response(404, json!({"status":"error"})),
            200,
        );
        assert!(remote.after_reply.is_empty());
        assert!(remote.take_effects().is_empty());
        let actions = remote
            .delivery
            .poll(
                &json!({"commands":[],"server_time":1000,"requests":[{
                    "id":id,"action":"lifecycle","params":{"operation":"exit_app"},"expires_at":1020
                }]}),
                300,
                0,
            )
            .unwrap();
        assert!(actions.is_empty());
        remote.finish(
            Kind::Delivery(SendKind::Response),
            response(200, json!({"status":"ok"})),
            400,
        );
        assert!(remote.take_effects().is_empty());
    }
    #[test]
    fn invalid_or_expired_receipts_never_trigger_effects() {
        for (result, now) in [
            (json!({"status":"ok"}), 200),
            (json!({"status":"ok","data":{}}), 30000),
        ] {
            let mut remote = Remote::from_install("dev_test", "runtime");
            let call = request(&mut remote);
            remote.complete_after_reply(call, result, AfterReply::Exit);
            remote.finish(
                Kind::Delivery(SendKind::Response),
                response(200, json!({"status":"ok"})),
                now,
            );
            assert!(remote.take_effects().is_empty());
        }
    }
    #[test]
    fn status_projection_and_presence_reject_the_same_private_metadata() {
        let mut remote = Remote::from_install("dev_test", "runtime");
        for title in [
            "https://example.invalid/movie?token=sentinel", "file:/private/movie",
            "Authorization: Bearer sentinel", "token=sentinel", "api_key:sentinel",
            "\u{200b}file:sentinel", "to\u{2060}ken=sentinel", "\u{e000}/private/movie",
        ] {
            let input = json!({"state":"playing","kind":"vod","title":title,
                "provider":"offline","media_id":"file:/private/movie","position":12.5,
                "duration":120,"volume":null,"channel_number":1,"secret":"sentinel"});
            let playback = project_playback(&input);
            let presence = remote.presence(&json!({"player":{},"playback":playback}));
            assert_eq!(presence["playback"], playback);
            assert_eq!(project_playback(&playback), playback);
            assert_eq!(playback.as_object().unwrap().len(), 9);
            assert!(playback["title"].is_null());
            assert!(playback["media_id"].is_null());
            assert!(!presence.to_string().contains("sentinel"));
        }
    }

    #[test]
    fn status_projection_and_presence_preserve_valid_metadata_and_inactive_rules() {
        let mut remote = Remote::from_install("dev_test", "runtime");
        for phase in ["loading", "playing", "paused", "buffering", "error", "idle", "ended", "standby"] {
            let input = json!({"state":phase,"kind":"vod","title":"Том и Джерри",
                "provider":"offline","media_id":"local_42-a.b","position":12.5,
                "duration":120.0,"volume":null,"channel_number":1});
            let playback = project_playback(&input);
            let presence = remote.presence(&json!({"player":{},"playback":playback}));
            assert_eq!(presence["playback"], playback);
            assert_eq!(playback["kind"], input["kind"]);
            assert_eq!(playback["position"], input["position"]);
            assert_eq!(playback["provider"], input["provider"]);
            if matches!(phase, "idle" | "ended" | "standby") {
                for field in ["title", "media_id", "channel_number"] {
                    assert!(playback[field].is_null());
                }
            } else {
                assert_eq!(playback, input);
            }
        }
    }

    #[test]
    fn live_status_preserves_vportal_ids_while_presence_keeps_retained_v1_ids() {
        for profile_id in ["native:vportal:home".to_string(), "p".repeat(128)] {
            let profile = plx_data::ott::ProviderProfile {
                id: profile_id, name: "Home".into(),
                kind: plx_data::ott::ProviderKind::VPortal,
                source: "portal::[key:synthetic-secret]https://portal.invalid/api".into(),
                source_alias: String::new(), enabled: true,
            };
            let page = plx_data::ott::parse_vportal_response(
                r#"{"type":"stream","title":"Fixture movie","id":42,"request":{"id":42}}"#,
                &profile, "Movies",
            ).expect("synthetic VPortal catalog");
            let entry = &page.catalog.entries[0];
            assert!(entry.id.starts_with(&format!("{}:", profile.id)));
            let mut remote = Remote::from_install("dev_test", "runtime");
            for phase in ["loading", "playing", "paused", "buffering", "error", "idle", "ended", "standby"] {
                let input = json!({"state":phase,"kind":"vod","title":entry.title,
                    "provider":"vportal","media_id":entry.id,"position":12.5,
                    "duration":120.0,"volume":null,"channel_number":null});
                let playback = project_playback(&input);
                let presence = remote.presence(&json!({"player":{},"playback":playback}));
                let mut retained = playback.clone();
                retained["media_id"] = Value::Null;
                assert_eq!(presence["playback"], retained);
                assert_eq!(project_playback(&playback), playback);
                if matches!(phase, "idle" | "ended" | "standby") {
                    assert!(playback["media_id"].is_null());
                } else {
                    assert_eq!(playback["media_id"].as_str(), Some(entry.id.as_str()), "{phase}");
                }
                assert!(!presence.to_string().contains("synthetic-secret"));
                assert!(!presence.to_string().contains("portal.invalid"));
            }
        }
    }

    #[test]
    fn retained_presence_keeps_legacy_media_id_bounds() {
        let mut remote = Remote::from_install("dev_test", "runtime");
        for (id, accepted) in [
            ("local_42-a.b".to_string(), true),
            ("p".repeat(128), true),
            ("p".repeat(129), false),
            ("p".repeat(145), false),
        ] {
            let input = json!({"state":"playing","kind":"vod","media_id":id});
            let playback = project_playback(&input);
            let presence = remote.presence(&json!({"player":{},"playback":input}));
            assert_eq!(presence["playback"], playback);
            assert_eq!(presence["version"], 1);
            assert_eq!(presence["playback"].as_object().unwrap().len(), 9);
            assert_eq!(presence["playback"]["media_id"].as_str(), accepted.then_some(id.as_str()));
        }
    }

    #[test]
    fn presence_projects_closed_wire_shape_and_omits_secret_metadata() {
        let mut remote = Remote::from_install("dev_test", "runtime");
        let value=remote.presence(&json!({"player":{"version":"0.2","secret":"private"},"playback":{"state":"idle","kind":"none","title":"old title","token":"private"}}));
        assert_eq!(value["playback"].as_object().unwrap().len(), 9);
        assert_eq!(value["playback"]["title"], Value::Null);
        assert!(!value.to_string().contains("private"));
        assert_eq!(value["player"]["platform"], "webos-native");
    }
    #[test]
    fn previous_control_generation_cannot_receive_async_results() {
        let mut remote = Remote::from_install("dev_test", "runtime");
        let invocation = request(&mut remote);
        assert!(remote.invocation_is_current(&invocation));
        remote.invalidate();
        assert!(!remote.invocation_is_current(&invocation));
        remote.complete(
            invocation,
            json!({"status":"ok","data":{"private":"old result"}}),
        );
        assert!(remote.delivery.idle());
    }
}

#[cfg(test)]
#[path = "diagnostics_tests.rs"]
mod diagnostics_tests;

#[cfg(test)]
#[path = "ingress_tests.rs"]
mod ingress_tests;

#[cfg(test)]
#[path = "media_tracks_ingress_tests.rs"]
mod media_tracks_ingress_tests;

#[cfg(all(test, feature = "hostsim"))]
mod settings_test_support;

#[cfg(test)]
mod discovery_tests;

#[cfg(test)]
mod discovery_compat_tests;

#[cfg(test)]
mod plex_servers_ingress_tests;

#[cfg(all(test, feature = "hostsim"))]
#[path = "lan_discovery_tests.rs"]
mod lan_discovery_tests;
