use super::*;
use crate::ott::navigation;
use plx_machine::machine::{FontScale, TypographyMeasure, UiMode};

fn modern() -> OttScreen {
    let mut screen=OttScreen::new(EntryId(1),InstanceId(1));
    let mut view=(*snapshot(1,&["one","two"])).clone();
    view.status="Loaded 2 / 2".into();
    screen.install(Arc::new(view));
    screen.classic_visible=true;
    screen.refresh_presentation(UiMode::Modern,&plx_ui::fixture::FixtureMeasure);
    screen.refresh_viewport(&plx_ui::fixture::FixtureMeasure);
    screen
}

#[test]
fn modern_preview_is_measured_before_source_or_ack_without_reserving_the_narrative() {
    let screen=modern();
    let target=screen.viewport_target().expect("eligible geometry must not wait for Source or ACK");
    let plx_media::player::viewport::Target::Inline{rect,..}=target else {panic!("inline target")};
    let route=screen.route();
    assert_eq!(rect.x as f32,route.narrative.x);
    assert_eq!(rect.y as f32,route.narrative.y);
    assert!((rect.height as f32-rect.width as f32*9.0/16.0).abs()<1.0);
    assert!(rect.x+rect.width <= route.content.x as i32);
    assert!(screen.viewport_aperture().is_none(),"geometry is not a submitted lease");
    let row=&screen.snapshot.rows[0];
    let unchanged=metadata::flow(row,&screen.status,&plx_ui::fixture::FixtureMeasure,route,screen.viewport_aperture(),None);
    assert!(unchanged.crumb.is_some());
    assert_eq!(unchanged.title.y,route.narrative.y+plx_ui::route_screen::CRUMB_BAND+theme::space::LG);
}

#[test]
fn modern_preview_and_text_share_non_overlapping_geometry_and_leave_cards_unchanged() {
    let screen=modern();
    let route=screen.route();
    let original=navigation::route();
    assert_eq!((route.content.x,route.content.y,route.content.w,route.content.h),
        (original.content.x,original.content.y,original.content.w,original.content.h));
    let row=&screen.snapshot.rows[0];
    let rect=metadata::preview_rect(row,&screen.status,&plx_ui::fixture::FixtureMeasure,route,None).unwrap();
    let flow=metadata::flow(row,&screen.status,&plx_ui::fixture::FixtureMeasure,route,Some(rect),None);
    assert!(flow.crumb.is_none());
    assert!(flow.title.y>=rect.y+rect.h+theme::space::MD);
    assert!(flow.detail.y>=flow.title.y+flow.title.h);
    assert!(flow.body.y>=flow.detail.y+flow.detail.h);
    let status=flow.status.unwrap();
    assert!(flow.body.y+flow.body.h <= status.y-theme::space::MD);
    assert!(status.y+status.h < route.action.y);
    assert!(screen.has_description(),"full reading path remains reachable");
}

#[test]
fn modern_preview_yields_to_large_type_short_bounds_and_loading_or_browse_pages() {
    let mut screen=modern();
    let row=&screen.snapshot.rows[0];
    let mut short=screen.route();short.action.y=short.narrative.y+100.0;
    assert!(metadata::preview_rect(row,&screen.status,&plx_ui::fixture::FixtureMeasure,short,None).is_none());
    let mut measure=TypographyMeasure::new(plx_ui::fixture::FixtureMeasure);
    measure.set_font_scale(FontScale::from_percent(200).unwrap());
    let mut long=row.clone();long.title="A deliberately long programme title with several translated words ".repeat(4);
    assert!(metadata::preview_rect(&long,&screen.status,&measure,screen.route(),None).is_none());
    let normal=screen.viewport_target().unwrap();
    screen.browse=true;screen.refresh_viewport(&plx_ui::fixture::FixtureMeasure);
    assert!(screen.viewport_target().is_none());
    screen.browse=false;screen.refresh_viewport(&plx_ui::fixture::FixtureMeasure);
    assert_eq!(screen.viewport_target(),Some(normal));
    screen.reading=true;assert!(screen.viewport_target().is_none());
    screen.reading=false;screen.classic_visible=false;assert!(screen.viewport_target().is_none());
    screen.classic_visible=true;
    let mut loading=(*screen.snapshot).clone();loading.page_id="loading".into();
    screen.install(Arc::new(loading));screen.refresh_viewport(&plx_ui::fixture::FixtureMeasure);
    assert!(screen.viewport_target().is_none());
}

#[test]
fn modern_preview_geometry_does_not_select_or_start_the_focused_media() {
    let mut screen=modern();
    let initial=screen.viewport_target();
    let key=screen.keys["item:two"];
    let out=deliver_recorded(&mut screen,ScreenEvent::FocusMoved{from:None,
        to:FocusKey{entry:EntryId(1),elem:key},by:plx_ui::screen::By::Dir});
    assert_eq!(screen.viewport_target(),initial);
    assert!(out.iter().all(|effect| !matches!(&effect.fx,Fx::App(AppFx::Player(_))
        | Fx::App(AppFx::Ott(OttOut::Action{action:OttAction::Play(_),..})))));
    assert!(screen.viewport_aperture().is_none());
}


#[test]
fn modern_metadata_painter_leaves_the_acknowledged_aperture_clear() {
    let _serial=plx_base::testlock::serial();
    let screen=modern();
    let route=screen.route();let row=&screen.snapshot.rows[0];
    let measure=plx_ui::fixture::FixtureMeasure;
    let rect=metadata::preview_rect(row,&screen.status,&measure,route,None).unwrap();
    let painted=plx_ui::draw_census::capture_runs(|| {
        metadata::paint(plx_ui::Painter::recording(),&measure,row,&screen.status,false,false,false,route,Some(rect),None);
    });
    let title=painted.iter().find(|run| run.text==row.title).expect("selected title is actually painted");
    assert!(title.at.y>=rect.y+rect.h);
    assert!(painted.iter().all(|run| run.at.y>=rect.y+rect.h),"no title/detail/body/status/hint crosses the video");
    let original=plx_ui::draw_census::capture_runs(|| {
        metadata::paint(plx_ui::Painter::recording(),&measure,row,&screen.status,false,false,false,route,None,None);
    });
    let original_title=original.iter().find(|run| run.text==row.title).unwrap();
    assert!(original_title.at.y<rect.y+rect.h,"no admitted aperture retains the original narrative");
    assert!(screen.has_description());
}

#[test]
fn modern_metadata_excerpt_requires_a_complete_actual_text_line() {
    let _serial=plx_base::testlock::serial();
    let screen=modern();let mut row=screen.snapshot.rows[0].clone();
    row.description="Sentinel excerpt".into();
    let measure=plx_ui::fixture::FixtureMeasure;
    let route=screen.route();
    let rect=metadata::preview_rect(&row,"",&measure,route,None).unwrap();
    let line_h=plx_ui::text_view::TextView::new(&row.description,theme::size::LABEL,theme::TEXT_SECONDARY)
        .with_measure(&measure).line_h();
    for preview in [None,Some(rect)] {
        let measured=metadata::flow(&row,"",&measure,route,preview,None);
        for (remaining,want_excerpt) in [(0.1,false),(line_h-0.1,false),(line_h+0.1,true)] {
            let mut bounded=route;
            bounded.action.y=measured.body.y+remaining+theme::space::MD;
            let runs=plx_ui::draw_census::capture_runs(|| {
                metadata::paint(plx_ui::Painter::recording(),&measure,&row,"",false,false,false,bounded,preview,None);
            });
            assert_eq!(runs.iter().any(|run| run.text==row.description),want_excerpt);
        }
    }
}

fn guide_row() -> crate::ott::OttRow {
    let mut row=modern().snapshot.rows[0].clone();
    row.title="Station".into();
    row.detail="Culture".into();
    row.description="OLD_SNAPSHOT_SYNOPSIS".into();
    row.poster=None;
    row.guide=Some(crate::ott::OttNowNext {
        now_s:1_700_000_010,
        current:Some(crate::ott::OttGuideProgramme {
            title:"CURRENT_TITLE".into(),description:"CURRENT_SYNOPSIS".into(),
            start_s:1_700_000_000,end_s:1_700_000_060,
        }),
        next:Some(crate::ott::OttGuideProgramme {
            title:"NEXT_TITLE".into(),description:"NEXT_SYNOPSIS".into(),
            start_s:1_700_000_060,end_s:1_700_000_120,
        }),
    });
    row
}

#[test]
fn modern_now_next_paints_titles_and_replaces_the_synopsis_at_rollover() {
    let _serial=plx_base::testlock::serial();
    let _language=plx_platform::i18n::language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
    let row=guide_row();
    let measure=plx_ui::fixture::FixtureMeasure;
    for (now,current,next,synopsis) in [
        (1_700_000_010,true,true,"CURRENT_SYNOPSIS"),
        (1_700_000_060,false,true,"NEXT_SYNOPSIS"),
        (1_700_000_120,false,false,""),
    ] {
        let runs=plx_ui::draw_census::capture_runs(|| {
            metadata::paint(plx_ui::Painter::recording(),&measure,&row,"",false,false,false,navigation::route(),None,Some(now));
        });
        assert_eq!(runs.iter().any(|run| run.text.contains("CURRENT_TITLE")),current);
        assert_eq!(runs.iter().any(|run| run.text.contains("NEXT_TITLE")),next);
        assert!(!runs.iter().any(|run| run.text.contains("OLD_SNAPSHOT_SYNOPSIS")));
        assert_eq!(metadata::description(&row,Some(now)),synopsis);
        if !synopsis.is_empty() {assert!(runs.iter().any(|run| run.text==synopsis));}
        else {assert!(!runs.iter().any(|run| run.text.contains("_SYNOPSIS")));}
    }
}

#[test]
fn modern_partial_missing_and_invalid_guide_do_not_reserve_phantom_rows() {
    let measure=plx_ui::fixture::FixtureMeasure;
    let mut row=guide_row();
    row.guide.as_mut().unwrap().current=None;
    let partial=metadata::flow(&row,"",&measure,navigation::route(),None,None);
    assert!(partial.current.is_none() && partial.next.is_some() && partial.progress.is_none());
    assert_eq!(metadata::description(&row,None),"");
    row.guide.as_mut().unwrap().next.as_mut().unwrap().end_s=1;
    let invalid=metadata::flow(&row,"",&measure,navigation::route(),None,None);
    assert!(invalid.current.is_none() && invalid.next.is_none() && invalid.progress.is_none());
    row.guide=None;
    let absent=metadata::flow(&row,"",&measure,navigation::route(),None,None);
    assert!(absent.current.is_none() && absent.next.is_none() && absent.progress.is_none());
    assert_eq!(invalid.body.y,absent.body.y);
    assert_eq!(metadata::description(&row,None),"OLD_SNAPSHOT_SYNOPSIS","VOD/no-guide descriptions are unchanged");
}

#[test]
fn modern_guide_measurement_retires_small_apertures_and_fits_scaled_fallback() {
    let _serial=plx_base::testlock::serial();
    struct Restore(FontScale);
    impl Drop for Restore {
        fn drop(&mut self) { plx_gfx::text::set_ui_typography(self.0); }
    }
    let _restore=Restore(plx_ui::typography::current());
    let _language=plx_platform::i18n::language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
    let row=guide_row();
    let route=navigation::route();
    for percent in [100,200] {
        let mut measure=TypographyMeasure::new(plx_ui::fixture::FixtureMeasure);
        // Match the application's single typography setting for measurement and paint.
        plx_ui::typography::apply(percent,&mut measure).unwrap();
        assert_eq!(plx_ui::Painter::recording().font_scale(),measure.scale());
        let fallback=metadata::flow(&row,"",&measure,route,None,None);
        assert!(fallback.body.h>=0.0,"complete guide must fit the unreserved narrative at {percent}%");
        let current=fallback.current.unwrap();let next=fallback.next.unwrap();let progress=fallback.progress.unwrap();
        assert!(current.y>=fallback.detail.y+fallback.detail.h);
        assert!(next.y>=current.y+current.h);
        assert!(progress.y>=next.y+next.h);
        assert!(fallback.body.y>=progress.y+progress.h);
        let mut short=route;
        short.action.y=fallback.body.y+theme::space::MD;
        assert!(metadata::preview_rect(&row,"",&measure,short,None).is_none(),"aperture may not steal the guide's text area");
        let runs=plx_ui::draw_census::capture_runs(|| {
            metadata::paint(plx_ui::Painter::recording(),&measure,&row,"",false,false,false,route,None,None);
        });
        assert!(runs.iter().any(|run| run.text.contains("CURRENT_TITLE")));
        assert!(runs.iter().any(|run| run.text.contains("NEXT_TITLE")));
        for run in runs.iter().filter(|run| run.text.contains("_TITLE")) {
            assert_eq!(run.size,measure.scale().pixels(theme::size::CAPTION));
            // Run.at is the glyph draw origin. TextView places the cap band at the flow's y.
            let (cap_top,baseline)=plx_gfx::text::text_cap_band(run.size,i32::from(run.bold));
            let y=run.at.y+cap_top;
            assert!(y>=current.y && y<route.action.y);
            let rect=if run.text.contains("CURRENT_TITLE") {current} else {next};
            assert!(y>=rect.y && run.at.y+baseline<=rect.y+rect.h,
                "guide cap band must remain in its measured row at {percent}%: {run:?}");
        }
    }
}

#[test]
fn modern_guide_clock_expiry_keeps_snapshot_and_does_not_fabricate_viewport_ack() {
    let mut screen=modern();
    let mut view=(*screen.snapshot).clone();
    view.revision+=1;view.rows[0]=guide_row();view.rows[0].id="one".into();
    screen.install(Arc::new(view));
    let route=screen.route();
    let measure=plx_ui::fixture::FixtureMeasure;
    // Geometry is only a proposal. No source or ACK is fabricated by a guide update.
    screen.refresh_viewport(&measure);
    assert!(screen.viewport_aperture().is_none());
    let before=screen.snapshot.clone();
    screen.guide_clock.update(before.rows[0].guide.as_ref(),1000);
    screen.guide_clock.update(before.rows[0].guide.as_ref(),111_000);
    screen.refresh_viewport(&measure);
    assert!(screen.viewport_aperture().is_none());
    assert!(screen.viewport_target().is_some(),"expired guide has no phantom reserved rows");
    let flow=metadata::flow(&screen.snapshot.rows[0],&screen.status,&measure,route,None,screen.guide_clock.now());
    assert!(flow.current.is_none() && flow.next.is_none());
    assert!(Arc::ptr_eq(&before,&screen.snapshot),"display time never mutates or re-fetches the source snapshot");
}
