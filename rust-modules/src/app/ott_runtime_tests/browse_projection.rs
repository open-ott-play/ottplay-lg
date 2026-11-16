//! Provider-neutral Modern metadata is borrowed from the current admitted
//! catalogue. Merely looking at a container never selects or plays it.
use super::*;
use plx_machine::machine::UiMode;

fn fixture() -> Box<App> {
    let mut app = Box::new(ready_app_fixture());
    let runtime = app.ott.as_mut().unwrap();
    let profile = ProviderProfile { id:"m3u-ui".into(), name:"Living room".into(),
        kind:ProviderKind::M3u, source:"https://fixture.invalid/list.m3u".into(),
        source_alias:String::new(), enabled:true };
    runtime.provider = ProviderKind::M3u;
    runtime.saved.profiles.profiles = vec![profile.clone()];
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    runtime.saved.settings.insert("interfaceMode".into(), json!(1));
    runtime.catalog = plx_data::ott::parse_m3u(
        "#EXTM3U\n#EXTINF:-1 group-title=\"News\" tvg-logo=\"https://fixture.invalid/logo.png\",Station\nhttps://fixture.invalid/live\n#EXTINF:60 group-title=\"Films\" tvg-logo=\"https://fixture.invalid/film.jpg\",Film\nhttps://fixture.invalid/film.mp4\n", &profile).unwrap();
    runtime.page = "categories";
    runtime.catalog_loading = false;
    runtime.catalog_complete = true;
    runtime.sync_saved_context();
    runtime.changed();
    app
}

#[test]
fn modern_category_cards_keep_canonical_actions_and_use_real_member_art() {
    let _lock = plx_base::testlock::serial();
    let app = fixture();
    let runtime = app.ott.as_ref().unwrap();
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    let generation = runtime.generation;
    let view = runtime.view();
    assert!(view.preview_context.is_some());
    let all = view.rows.iter().find(|row| row.id == "all").unwrap();
    assert!(matches!(&all.action, OttAction::SelectCategoryAt { category, .. } if category.is_empty()));
    assert!(all.media.is_none());
    assert!(all.detail.contains('2'));
    for row in view.rows.iter().filter(|row| matches!(row.action, OttAction::SelectCategoryAt { .. })) {
        assert!(row.content_title.is_some());
        assert!(row.description.is_empty(), "a member's plot is not its folder's synopsis");
        assert!(row.poster.is_some());
        assert!(row.media.is_none(), "a container is never fabricated playable media");
    }
    assert_eq!(runtime.generation, generation);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert!(runtime.playing.is_none() && runtime.resolver_next.is_none());
}

#[test]
fn provider_and_profile_cards_never_borrow_other_profiles_catalogue_art() {
    let _lock = plx_base::testlock::serial();
    let mut app = fixture();
    let runtime = app.ott.as_mut().unwrap();
    runtime.page = "providers";
    let view = runtime.view();
    let provider = view.rows.iter().find(|row| row.id == "m3u").unwrap();
    assert!(provider.content_title.is_some() && provider.poster.is_none());
    assert!(provider.detail.ends_with('1'));
    runtime.page = "profiles";
    let view = runtime.view();
    assert!(view.rows.iter().filter(|row| matches!(row.action, OttAction::SelectProfile(_)))
        .all(|row| row.content_title.is_some() && row.poster.is_none() && row.media.is_none()));
    assert!(runtime.playing.is_none() && runtime.resolver_next.is_none());
}

#[test]
fn modern_container_context_is_stable_for_appends_and_retires_on_source_change() {
    let _lock = plx_base::testlock::serial();
    let mut app = fixture();
    let runtime = app.ott.as_mut().unwrap();
    let context = browse_projection::context(runtime).unwrap();
    runtime.catalog_revision += 1;
    runtime.revision += 1;
    assert_eq!(browse_projection::context(runtime).as_deref(), Some(context.as_str()));
    runtime.generation += 1;
    assert_ne!(browse_projection::context(runtime).as_deref(), Some(context.as_str()));
    let current = browse_projection::context(runtime).unwrap();
    runtime.saved.profiles.profiles[0].source = "https://other.invalid/list.m3u".into();
    assert_ne!(browse_projection::context(runtime).as_deref(), Some(current.as_str()));
    // A presentation digest does not grant the media preview endpoint authority.
    assert!(preview::context(runtime).is_none());
}

#[test]
fn classic_category_projection_and_actions_stay_unchanged() {
    let _lock = plx_base::testlock::serial();
    let mut app = fixture();
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.settings.insert("interfaceMode".into(), json!(0));
    assert_eq!(runtime.interface_mode(), UiMode::Classic);
    let view = runtime.view();
    assert!(view.preview_context.is_none());
    assert!(view.rows.iter().filter(|row| matches!(row.action, OttAction::SelectCategoryAt { .. }))
        .all(|row| row.poster.is_none() && row.content_title.is_none() && row.detail.is_empty()));
}

#[test]
fn representative_artwork_visits_and_admitted_payload_are_bounded_without_hiding_categories() {
    let _lock = plx_base::testlock::serial();
    let mut app = fixture();
    let runtime = app.ott.as_mut().unwrap();
    let mut playlist = String::from("#EXTM3U\n");
    for index in 0..300 {
        playlist.push_str(&format!("#EXTINF:-1 group-title=\"Group {index}\" tvg-logo=\"https://fixture.invalid/{}.png\",Station {index}\nhttps://fixture.invalid/live/{index}\n", "a".repeat(2000)));
    }
    runtime.catalog = plx_data::ott::parse_m3u(&playlist, runtime.active().unwrap()).unwrap();
    runtime.catalog_revision += 1;
    let view = runtime.view();
    assert_eq!(view.rows.iter().filter(|row| matches!(row.action,
        OttAction::SelectCategoryAt { .. })).count(), 301);
    let urls: Vec<_> = view.rows.iter().filter_map(|row| match &row.poster {
        Some(plx_screens::ott::OttPoster::Http { url }) => Some(url), _ => None,
    }).collect();
    assert!(!urls.is_empty() && urls.len() <= 256);
    assert!(urls.iter().map(|url| url.len()).sum::<usize>() <= 256 * 1024);
    assert!(runtime.resolver_next.is_none() && runtime.playing.is_none());
}
