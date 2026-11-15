//! Device-local media resolution. The private input receipt survives for the
//! whole decoder lifetime; no URL or filesystem authority crosses the wire.
use super::*;
use plx_media::ff::ExternalSource;

pub(in crate::app::ott_runtime) struct LocalResolved {
    pub input: AuthorizedInput,
    pub source: ExternalSource,
    // Drop signals the independent provider worker; never joins on the UI thread.
    pub lease: Option<crate::app::ott_vportal_lease::Lease>,
}
impl AuthorizedInput {
    pub fn resolve_local(self, worker: &plx_base::task::OffFrame) -> Result<LocalResolved, Error> {
        if !self.work.local || self.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let (source, lease) = if let Some(plex) = &self.plex {
            (plex.local_source().map_err(|_| Error::Stale)?, None)
        } else if matches!(self.entry.media, MediaLocator::Local { .. }) {
            (
                transient::open_video_source(
                    worker,
                    self.work.local_read.as_ref(),
                    &self.work.binding.profile,
                    &self.entry,
                    &self.work.cancel,
                )
                .map_err(|_| Error::Resolve)?,
                None,
            )
        } else {
            let resolved = ott_catalog::resolve_typed(
                worker,
                &self.work.binding.profile,
                &self.entry,
                &self.work.cancel,
            )
            .map_err(|_| Error::Resolve)?;
            let url = resolved.url;
            let hls = url
                .split(['?', '#'])
                .next()
                .is_some_and(|path| path.to_ascii_lowercase().ends_with(".m3u8"));
            (
                if hls {
                    ExternalSource::Hls(url)
                } else {
                    ExternalSource::Http(url)
                },
                resolved.lease,
            )
        };
        if self.is_cancelled() || !self.current_plex_authority() {
            return Err(Error::Cancelled);
        }
        Ok(LocalResolved {
            input: self,
            source,
            lease,
        })
    }
}

#[cfg(all(test, feature = "hostsim"))]
mod tests {
    use super::*;
    use plx_data::ott::multiview::{Layout, Scene, SceneId, SlotId};

    fn fixture() -> (App, Runtime) {
        let mut app = crate::app::run::lifecycle_regression_tests::app();
        app.running = true;
        let mut runtime =
            Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
        runtime.remote = Remote::disabled_for_test("dev_local_sources", "fixture");
        runtime.loaded = true;
        runtime.state_load = StateLoad::Idle;
        runtime.entropy = Some([3; 16]);
        runtime.resume_source_work();
        let profile = ProviderProfile {
            id: "local-m3u".into(),
            name: "Source".into(),
            kind: ProviderKind::M3u,
            source: "https://source.invalid/list.m3u".into(),
            source_alias: "Source".into(),
            enabled: true,
        };
        runtime.saved.profiles.active_profile = Some(profile.id.clone());
        runtime.saved.profiles.profiles.push(profile);
        runtime.provider = ProviderKind::M3u;
        runtime.catalog.entries.push(CatalogEntry {
            id: "direct-channel".into(),
            title: "Direct channel".into(),
            category: String::new(),
            description: String::new(),
            poster: String::new(),
            synopsis: None,
            protection: Some(false),
            media: MediaLocator::Http {
                url: "https://source.invalid/live.m3u8".into(),
            },
            duration_ms: None,
            provider_id: None,
            provider_id_field: None,
            live: None,
        });
        (app, runtime)
    }
    fn resolve_ticket(handle: SourceHandle) -> ResolveTicket {
        let mut scene = Scene::new(SceneId::from_opaque(NonZeroU64::new(71).unwrap()));
        scene.set_layout(Layout::Grid4).unwrap();
        scene
            .bind_source(SlotId::SLOT_0, handle.source_ref())
            .unwrap();
        scene
            .begin_resolution(SlotId::SLOT_0, handle.source_ref(), handle.media_ref())
            .unwrap()
    }
    #[test]
    fn direct_catalogue_local_resolution_uses_real_authority_without_saved_favorite_or_fullscreen_ticket(
    ) {
        let _serial = plx_base::testlock::serial();
        for (original, canonical) in [
            (
                "https://source.invalid/live.m3u8",
                "https://source.invalid:443/live.m3u8",
            ),
            (
                "https://source.invalid:8443/live.m3u8?fixture=1",
                "https://source.invalid:8443/live.m3u8?fixture=1",
            ),
        ] {
            let (app, mut runtime) = fixture();
            runtime.catalog.entries[0].media = MediaLocator::Http {
                url: original.into(),
            };
            let entry_before = runtime.catalog.entries[0].clone();
            let profile_before = runtime.effective_profile().unwrap().clone();
            let before = serde_json::to_value(&runtime.saved).unwrap();
            let ticket_before = plx_media::route::play_request_ticket();
            assert_eq!(Snapshot::capture(&runtime).unwrap().rows().count(), 0);
            let snapshot = Snapshot::capture_local(&runtime).unwrap();
            let handle = snapshot.rows().next().unwrap().handle;
            let work = snapshot
                .capture_local_work(&mut runtime, &app, &handle, resolve_ticket(handle))
                .unwrap();
            let worker = plx_base::task::OffFrame::for_test();
            let metadata = work.prepare_local_metadata(&worker).unwrap();
            let input = metadata.authorize_completion(&mut runtime, &app).unwrap();
            let resolved = input.resolve_local(&worker).unwrap();
            // Ordinary typed resolution uses the strict transport's canonical
            // authority spelling, including the explicit HTTPS port.
            assert_eq!(resolved.source, ExternalSource::Hls(canonical.into()));
            assert_eq!(resolved.input.entry, entry_before);
            assert_eq!(runtime.catalog.entries[0], entry_before);
            assert_eq!(resolved.input.work.binding.profile, profile_before);
            resolved.input.reauthorize(&mut runtime, &app).unwrap();
            assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
            assert_eq!(plx_media::route::play_request_ticket(), ticket_before);
            assert!(
                runtime.play_target.is_none()
                    && runtime.resolver_next.is_none()
                    && runtime.playing.is_none()
            );
        }
    }
    #[test]
    fn local_capture_survives_unrelated_catalogue_generation_but_revokes_on_profile_epoch_or_snapshot_drop(
    ) {
        let _serial = plx_base::testlock::serial();
        let (app, mut runtime) = fixture();
        let snapshot = Snapshot::capture_local(&runtime).unwrap();
        let handle = snapshot.rows().next().unwrap().handle;
        let work = snapshot
            .capture_local_work(&mut runtime, &app, &handle, resolve_ticket(handle))
            .unwrap();
        runtime.generation += 1;
        runtime.catalog_revision += 1;
        runtime.catalog.entries.clear();
        runtime.profile_edit_epoch += 1;
        runtime.saved.profiles.active_profile = None;
        assert!(snapshot.current_for(&runtime));
        assert!(
            work.current_for(&runtime),
            "browsing cannot cancel the already captured provider input"
        );
        runtime.multiview_local_profile_epoch += 1;
        assert!(
            !work.current_for(&runtime),
            "definition ABA authority remains mandatory"
        );
        let cancel = work.cancellation();
        drop(snapshot);
        assert!(cancel.load(Ordering::Acquire));
    }

    #[test]
    fn local_refresh_appends_provider_preserves_active_handle_and_zero_epoch_denies() {
        let _serial = plx_base::testlock::serial();
        let (app, mut runtime) = fixture();
        let mut second = runtime.saved.profiles.profiles[0].clone();
        second.id = "second-m3u".into();
        second.name = "Second".into();
        second.source = "https://second.invalid/list.m3u".into();
        runtime.saved.profiles.profiles.push(second.clone());
        let mut snapshot = Snapshot::capture_local(&runtime).unwrap();
        let first = snapshot.rows().next().unwrap().handle;
        let work = snapshot
            .capture_local_work(&mut runtime, &app, &first, resolve_ticket(first))
            .unwrap();
        runtime.saved.profiles.active_profile = Some(second.id);
        runtime.profile_edit_epoch += 1;
        runtime.generation += 1;
        runtime.catalog.entries[0].id = "second-channel".into();
        runtime.catalog.entries[0].media = MediaLocator::Http {
            url: "https://second.invalid/live.m3u8".into(),
        };
        snapshot.refresh_local(&runtime).unwrap();
        assert_eq!(snapshot.rows().count(), 2);
        assert!(snapshot.contains(&first) && work.current_for(&runtime));
        assert!(snapshot.rows().nth(1).unwrap().handle != first);
        runtime.multiview_local_profile_epoch = 0;
        assert!(!snapshot.current_for(&runtime) && !work.current_for(&runtime));
        assert!(Snapshot::capture_local(&runtime).is_err());
    }
}
