//! Explicit hosted EPG configuration; existing metadata epochs own all work.
use super::*;
pub(super) const KEY: &str = "nativeEpgApiV1";
pub(super) fn parse(
    value: &Value,
) -> Result<Option<plx_data::ott::hosted_epg::Config>, &'static str> {
    let text = value
        .as_str()
        .filter(|text| text.len() <= 4096)
        .ok_or("Enter a hosted EPG configuration string")?;
    if text.is_empty() {
        return Ok(None);
    }
    let value = serde_json::from_str(text).map_err(|_| "Invalid hosted EPG configuration")?;
    plx_data::ott::hosted_epg::Config::parse(&value)
        .map(Some)
        .map_err(|_| "Invalid hosted EPG configuration")
}
impl Runtime {
    pub(super) fn sync_hosted_epg_config(&mut self) -> Result<(), &'static str> {
        let next = parse(
            self.saved
                .settings
                .get(KEY)
                .unwrap_or(&Value::String(String::new())),
        )?;
        if next == self.hosted_epg_config {
            return Ok(());
        }
        self.hosted_epg_config = next;
        guide_ui::close(self);
        self.metadata_cancel.store(true, Ordering::Release);
        self.metadata_cancel = Arc::new(AtomicBool::new(false));
        self.metadata_generation = self.metadata_generation.wrapping_add(1);
        self.metadata_target = None;
        self.metadata_query = None;
        self.metadata_next = None;
        self.metadata_guide = None;
        self.row_guide = None;
        self.row_guide_attempt = None;
        self.row_guide_coverage = None;
        self.favorite_preview = None;
        self.metadata_refresh_ms = 0;
        self.live_hud = Default::default();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_epg_configuration_cancels_old_metadata_epoch_without_touching_catalog() {
        let mut runtime = Box::new(Runtime::unloaded(
            crate::app::ott_companion::Supervisor::inert_for_test(),
        ));
        let old_cancel = runtime.metadata_cancel.clone();
        let generation = runtime.metadata_generation;
        let catalog_generation = runtime.generation;
        let text = r#"{"version":1,"url":"https://guide.invalid/epg/v1","source":"epg-one"}"#;
        runtime.saved.settings.insert(KEY.into(), json!(text));
        runtime.sync_hosted_epg_config().unwrap();
        assert!(old_cancel.load(Ordering::Acquire));
        assert!(!runtime.metadata_cancel.load(Ordering::Acquire));
        assert_ne!(runtime.metadata_generation, generation);
        assert_eq!(runtime.generation, catalog_generation);
        assert_eq!(
            runtime.saved.settings[KEY], text,
            "storage preserves caller JSON text"
        );
        let accepted = runtime.metadata_generation;
        runtime
            .saved
            .settings
            .insert(KEY.into(), json!(text.replace(",", ", ")));
        runtime.sync_hosted_epg_config().unwrap();
        assert_eq!(
            runtime.metadata_generation, accepted,
            "formatting is not a different guide source"
        );
        runtime.saved.settings.insert(KEY.into(), json!(""));
        runtime.sync_hosted_epg_config().unwrap();
        assert!(runtime.hosted_epg_config.is_none());
        assert_ne!(runtime.metadata_generation, accepted);
    }
}
