//! Catalogue metadata uses the same title, text, poster and reading primitives
//! as the native media screens. No network or texture ownership lives here.
use super::{OttGuideProgramme, OttPoster, OttRow};
use plx_machine::machine::Measure;
use plx_platform::i18n::msg;
use plx_ui::{Painter,Rect,theme};
use plx_ui::route_screen::{RouteLayout,draw_back_crumb,CRUMB_BAND};
use plx_ui::text_view::TextView;
use plx_ui::widgets::{self,Art};
use plx_ui::hstack::Item;
use plx_ui::vstack::VFlow;

fn guide(row: &OttRow, now: Option<i64>) -> (Option<&OttGuideProgramme>, Option<&OttGuideProgramme>) {
    row.guide.as_ref().map(|guide| guide.at(now.unwrap_or(guide.now_s))).unwrap_or((None, None))
}

fn programme_label(programme: &OttGuideProgramme) -> String {
    let clock = plx_platform::i18n::local_clock_parts(programme.start_s)
        .and_then(|parts| plx_platform::i18n::current().time(parts)).unwrap_or_default();
    if clock.is_empty() { programme.title.clone() } else { format!("{clock}  {}", programme.title) }
}

/// A retained guide can roll forward without a new source snapshot. Never show
/// the previous programme's snapshot synopsis beneath the newly current title.
pub(super) fn description(row: &OttRow, now: Option<i64>) -> &str {
    if row.guide.is_some() { guide(row, now).0.map_or("", |programme| programme.description.as_str()) }
    else { &row.description }
}

/// Only source-attested synopsis bytes can omit generated facts in a preview.
/// Guide rollovers use their current programme; the full reader uses `description`.
pub(super) fn preview_description(row: &OttRow, now: Option<i64>) -> &str {
    if row.guide.is_some() { return description(row, now); }
    row.media.as_ref().and_then(|media| media.synopsis.as_ref())
        .and_then(|span| span.get(&row.description)).unwrap_or(&row.description)
}

/// One measured narrative flow serves both eligibility and painting. The
/// optional video is a lease on the existing player, never selected-item art.
#[derive(Clone, Copy)]
pub(super) struct Flow {
    pub crumb: Option<Rect>,
    pub title: Rect,
    pub detail: Rect,
    pub current: Option<Rect>,
    pub next: Option<Rect>,
    pub progress: Option<Rect>,
    pub body: Rect,
    pub status: Option<Rect>,
}
pub(super) fn flow(row:&OttRow,status:&str,measure:&dyn Measure,layout:RouteLayout,preview:Option<Rect>,now:Option<i64>)->Flow {
    let area=layout.narrative;
    let crumb=preview.is_none().then(|| Rect::new(area.x,area.y,area.w,CRUMB_BAND));
    // Browse already supplies the page header. With live video, start the
    // selected item's text below the aperture rather than drawing a crumb on it.
    let top=preview.map_or(area.y+CRUMB_BAND+theme::space::LG, |rect| rect.y+rect.h+theme::space::MD);
    let title_h=TextView::new(&row.title,theme::size::HEADLINE,plx_ui::palette::color(plx_ui::palette::Role::Text, theme::TEXT_HEADING))
        .bold().max_lines(3).with_measure(measure).measure_h(area.w);
    let title=Rect::new(area.x,top,area.w,title_h);
    let detail_copy=super::media::detail(row);
    let detail_h=TextView::new(&detail_copy,theme::size::CAPTION,plx_ui::palette::color(plx_ui::palette::Role::Muted, theme::TEXT_SECONDARY))
        .max_lines(2).with_measure(measure).measure_h(area.w);
    let detail=Rect::new(area.x,top+title_h+theme::space::MD,area.w,detail_h);
    let mut bottom=layout.action.y-theme::space::MD;
    let status=if status.trim().is_empty() { None } else {
        let height=TextView::new(status,theme::size::CAPTION,plx_ui::palette::color(plx_ui::palette::Role::Muted, theme::TEXT_SECONDARY))
            .max_lines(3).with_measure(measure).measure_h(area.w);
        let rect=Rect::new(area.x,bottom-height,area.w,height);
        bottom-=height+theme::space::MD;
        Some(rect)
    };
    let mut body_top=detail.y+detail.h+theme::space::LG;
    let (current, next)=guide(row,now);
    let mut guide_flow=VFlow::new(body_top,theme::space::SM);
    let mut programme_rect=|programme:Option<&OttGuideProgramme>,bold:bool| {
        programme.and_then(|programme| {
            let label=programme_label(programme);
            let view=TextView::new(&label,theme::size::CAPTION,plx_ui::palette::color(plx_ui::palette::Role::Muted, theme::TEXT_SECONDARY)).max_lines(2).with_measure(measure);
            let view=if bold {view.bold()} else {view};
            guide_flow.push(Item::new(view.measure_h(area.w)))
                .map(|span| Rect::new(area.x,span.y,area.w,span.h))
        })
    };
    let current_rect=programme_rect(current,true);
    let next_rect=programme_rect(next,false);
    let progress=current.and_then(|_| guide_flow.push(Item::new(theme::space::XS)))
        .map(|span| Rect::new(area.x,span.y,area.w,span.h));
    if current.is_some() || next.is_some() {
        body_top=guide_flow.push(Item::new(0.0)).map_or(body_top,|span| span.y);
    }
    Flow { crumb,title,detail,current:current_rect,next:next_rect,progress,
        body:Rect::new(area.x,body_top,area.w,bottom-body_top),status }
}

/// The narrative's fixed 16:9 aperture does not move or resize the card grid.
/// Long titles, a large type scale or a short viewport take precedence over
/// video: the original metadata layout remains available without an aperture.
pub(super) fn preview_rect(row:&OttRow,status:&str,measure:&dyn Measure,layout:RouteLayout,now:Option<i64>)->Option<Rect> {
    let area=layout.narrative;
    let rect=Rect::new(area.x.ceil(),area.y.ceil(),area.w.floor(),(area.w.floor()*9.0/16.0).floor());
    if ![rect.x,rect.y,rect.w,rect.h,area.h,layout.action.y].iter().all(|value| value.is_finite())
        || rect.x<0.0 || rect.y<0.0 || rect.w<=0.0 || rect.h<=0.0
        || rect.x+rect.w>Rect::FULL.w || rect.x+rect.w>area.x+area.w
        || rect.y+rect.h>area.y+area.h
        || rect.y+rect.h>Rect::FULL.h { return None; }
    let measured=flow(row,status,measure,layout,Some(rect),now);
    // A description can use the existing full Description reader when the
    // inline excerpt no longer fits; the title, detail and status never overlap.
    (measured.body.h>=0.0).then_some(rect)
}

fn art_frame(frame: Rect, image: widgets::ArtImage) -> Rect {
    plx_appkit::scrub_preview::fit_rect(frame, image.w, image.h)
}

/// Details show the complete asset, whether it is a channel logo or a poster.
/// Resolve once through the ordinary cache and arrival animation; the fitted
/// image keeps the frame's centre, so its motion sample remains the same.
pub(super) fn paint_art(p: Painter, frame: Rect, art: Art<'_>, radius: f32) {
    let image = widgets::resolve_card_art(p, frame, &art);
    let fitted = art_frame(frame, image);
    widgets::card_named_led(p, fitted, art, None, radius, false, 1.0, 0.0, Some(image));
}

pub(super) fn paint(p:Painter,measure:&dyn Measure,row:&OttRow,status:&str,status_working:bool,reading:bool,cards:bool,layout:RouteLayout,preview:Option<Rect>,now:Option<i64>){
    let area=layout.narrative;
    let flow=flow(row,status,measure,layout,preview,now);
    if let Some(crumb)=flow.crumb {
        draw_back_crumb(p,crumb,if reading {msg::ott_channel_list()} else {msg::ott_brand()},measure);
    }
    TextView::new(&row.title,theme::size::HEADLINE,plx_ui::palette::color(plx_ui::palette::Role::Text, theme::TEXT_HEADING)).bold().max_lines(3)
        .with_measure(measure).draw(p,flow.title);
    let detail_copy=super::media::detail(row);
    TextView::new(&detail_copy,theme::size::CAPTION,plx_ui::palette::color(plx_ui::palette::Role::Muted, theme::TEXT_SECONDARY)).max_lines(2)
        .with_measure(measure).draw(p,flow.detail);
    if let Some(rect)=flow.status {
        super::media::note_status(p, status, status_working);
        TextView::new(status,theme::size::CAPTION,plx_ui::palette::color(plx_ui::palette::Role::Muted, theme::TEXT_SECONDARY)).max_lines(3)
            .with_measure(measure).draw(p,rect);
    }
    let (current,next)=guide(row,now);
    let content_bottom=flow.body.y+flow.body.h;
    for (programme,rect,bold) in [(current,flow.current,true),(next,flow.next,false)] {
        if let (Some(programme),Some(rect))=(programme,rect) {
            // An unusually large label may exhaust even the no-video layout.
            // Do not let its guide rows paint over the status or action band.
            if rect.y+rect.h>content_bottom {continue;}
            let label=programme_label(programme);
            let view=TextView::new(&label,theme::size::CAPTION,plx_ui::palette::color(plx_ui::palette::Role::Muted, theme::TEXT_SECONDARY)).max_lines(2).with_measure(measure);
            let view=if bold {view.bold()} else {view};
            view.draw(p,rect);
        }
    }
    if let (Some(current),Some(rect))=(current,flow.progress) {
        if rect.y+rect.h<=content_bottom {
            let now=now.unwrap_or_else(|| row.guide.as_ref().map_or(current.start_s,|guide| guide.now_s));
            let progress=((now as i128-current.start_s as i128) as f64
                /(current.end_s as i128-current.start_s as i128) as f64).clamp(0.0,1.0) as f32;
            widgets::progress_bar(p,rect,0.0,rect.h,progress);
        }
    }
    let has_description=!description(row,now).is_empty();
    let description=preview_description(row,now);
    let mut body=Rect::new(flow.body.x,flow.body.y,flow.body.w,flow.body.h.max(0.0));
    if let Some(poster)=row.poster.as_ref().filter(|_| body.h>0.0) {
        let frame=Rect::new(area.x,body.y,plx_ui::consts::CARD_W,plx_ui::consts::CARD_H.min(body.h));
        let art=match poster {
            OttPoster::Plex{server,key}=>Art::Thumb{sid:*server,key,res:(250,375)},
            OttPoster::Http{url}=>Art::Http{url,res:(250,375)},
            OttPoster::Local{key}=>Art::Local{key},
        };
        paint_art(p,frame,art,theme::CARD_RING_RAD);
        body.x+=frame.w+theme::space::LG;body.w-=frame.w+theme::space::LG;
    }
    let line_h=TextView::new(description,theme::size::LABEL,plx_ui::palette::color(plx_ui::palette::Role::Muted, theme::TEXT_SECONDARY)).with_measure(measure).line_h();
    if !reading && !description.is_empty() && body.h>=line_h {
        let lines=(body.h/line_h).floor() as usize;
        TextView::new(description,theme::size::LABEL,plx_ui::palette::color(plx_ui::palette::Role::Muted, theme::TEXT_SECONDARY)).max_lines(lines.max(1))
            .with_measure(measure).fade_last(theme::space::XL).draw(p,body);
    }
    let hint=if reading {msg::ott_read_hint()} else if cards {msg::ott_cards_hint()} else if !has_description {msg::ott_favorite_hint()}
        else {msg::ott_description_hint()};
    TextView::new(hint,theme::size::CAPTION,plx_ui::palette::color(plx_ui::palette::Role::Muted, theme::TEXT_TERTIARY)).with_measure(measure)
        .draw(p,Rect::new(area.x,layout.action.y,area.w,layout.action.h));
}

#[cfg(test)]
mod artwork_tests {
    use super::*;

    #[test]
    fn details_preserve_complete_wide_square_and_tall_art_inside_full_and_clipped_frames() {
        let key = plx_machine::machine::PosterKey(1);
        let arts = [Art::Http { url: "https://example.invalid/logo.png", res: (250, 375) },
            Art::Thumb { sid: 1, key: "/poster", res: (250, 375) }, Art::Local { key: &key }];
        for frame in [Rect::new(96.0, 350.0, 250.0, 375.0),
            Rect::new(0.0, 400.0, 120.0, 180.0), Rect::new(96.0, 850.0, 250.0, 80.0)] {
            for (w, h) in [(400.0, 100.0), (100.0, 100.0), (100.0, 400.0)] {
                let image = widgets::ArtImage { tex: 1, w, h, shown: 1.0 };
                let fitted = art_frame(frame, image);
                assert!(fitted.x >= frame.x - 0.001 && fitted.y >= frame.y - 0.001);
                assert!(fitted.x + fitted.w <= frame.x + frame.w + 0.001);
                assert!(fitted.y + fitted.h <= frame.y + frame.h + 0.001);
                assert!((fitted.cx() - frame.cx()).abs() < 0.001);
                assert!((fitted.cy() - frame.cy()).abs() < 0.001);
                assert!((fitted.w / fitted.h - w / h).abs() < 0.001);
                for art in &arts {
                    assert!(widgets::art_uv(art, w, h, fitted).into_iter()
                        .zip(plx_gfx::gfx::UV_FULL).all(|(actual, full)| (actual - full).abs() < 0.000001),
                        "details must retain every edge of the image");
                }
            }
            assert_eq!(art_frame(frame, widgets::ArtImage::NONE), frame,
                "unloaded art retains the existing bounded placeholder");
        }
        let poster = Rect::new(0.0, 0.0, 250.0, 375.0);
        let logo = widgets::ArtImage { tex: 1, w: 400.0, h: 100.0, shown: 1.0 };
        assert_eq!(art_frame(poster, logo), Rect::new(0.0, 156.25, 250.0, 62.5));
        assert!(widgets::art_uv(&arts[0], logo.w, logo.h, poster)[2] < 1.0,
            "ordinary grid cards still use their established cover crop");
    }
}
