// Included from navigation.rs to reuse its actual Bridge/worker-inbox fixture.
#[test]
fn terminal_catalog_return_rejects_a_replaced_profile_or_source() {
    let _serial = plx_base::testlock::serial();
    for completed in [false, true] {
        for replacement in ["profile", "source", "generation"] {
            let mut app = m3u_navigation_fixture();
            mount_ott(&mut app, "channels", false);
            let runtime = app.ott.as_ref().unwrap();
            runtime.catalog_tx.send(CatalogEvent::Finished(runtime.generation,
                if completed { Ok(()) } else { Err("Original source failed") })).unwrap();
            runtime_tick(&mut app);
            press_back(&mut app);
            press_back(&mut app);
            let runtime = app.ott.as_mut().unwrap();
            assert_eq!(runtime.page, "providers");
            match replacement {
                "profile" => runtime.saved.profiles.active_profile = Some("second".into()),
                "source" => runtime.saved.profiles.profiles[0].source = "https://media.invalid/replaced.m3u".into(),
                _ => runtime.generation = runtime.generation.wrapping_add(1),
            }
            // A stale marker must neither overwrite a newer outcome nor queue
            // the old source. This is a source-fence probe, not a profile save.
            runtime.status = "New source outcome".into();
            let generation = runtime.generation;
            let saved = serde_json::to_value(&runtime.saved).unwrap();
            show_channel_list(&mut app);
            let runtime = app.ott.as_ref().unwrap();
            assert_eq!(runtime.status, "New source outcome", "{completed}/{replacement}");
            assert_eq!(runtime.generation, generation);
            assert!(!runtime.catalog_complete && !runtime.catalog_loading);
            assert!(runtime.catalog_return.is_none() && runtime.catalog_next.is_none());
            assert!(!runtime.saving && runtime.play_target.is_none() && runtime.resolver_next.is_none());
            assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        }
    }
}
