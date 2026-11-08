//! In-player Info: Classic expands a read-only synopsis above the shared information footer;
//! Modern retains its horizontal artwork, metadata and action card. The owner routes input and
//! publishes the display sample; this module uses accepted plx_data::metadata only.
#![allow(dead_code)]
use plx_data::metadata;
use plx_ui::consts::{SCR_H, SCR_W};
use plx_ui::frame::Budget;
use plx_ui::geom::IndexElem;
use plx_ui::hstack::{Align, HStack, Item, Kept};
use plx_ui::icons::Icon;
use plx_machine::machine::{Cx, EntryId, FocusKey, GroupId, Host, UiMode};
use plx_ui::screen::{
    Activate, At, AxisMask, Dir, DrawFrame, EdgeRule, ElemKind, Focusable, GroupKind, GroupSpec,
    Hover, Part, Placed, Seat, Step, Stop,
};
use plx_ui::text_view::TextView;
use plx_ui::theme;
use plx_ui::vstack::{Anchor, VStack};
use plx_ui::widgets::{
    badge, badge_w, resolve_tex_wh_on, BadgeStyle, DottedRun,
};
use plx_ui::{Painter, Rect, View};
use std::ffi::CString;
use std::os::raw::c_int;

/// Maximum action count. The second label is presented only when the playing source
/// owns a detail target; focus geometry uses the same availability projection.
const N_ACTIONS: usize = 2;

/// Typed live-channel facts, not a missing finite seek capability.
pub fn live_channel(meta: metadata::MetadataView<'_>) -> bool {
    meta.now_playing().and_then(|now| now.ott.as_ref())
        .is_some_and(|ott| ott.live.is_some() && ott.archive.is_none())
}

/// The synopsis the shared Info card actually renders. OTT facts never borrow
/// an unrelated native detail page when their own synopsis is absent.
pub fn description(meta: metadata::MetadataView<'_>) -> &str {
    meta.now_playing().map(|now| now.summary.as_str()).filter(|text| !text.is_empty())
        .or_else(|| fallback_detail(meta).map(|detail| detail.summary.as_str()))
        .unwrap_or("")
}

#[derive(Clone, Debug, PartialEq)]
pub enum InfoAction {
    None,
    FromBeginning,
    GoToDetail(String), // rk to open: the show (episode) or the movie
}

/// Accepted application display sample shared by the footer and expanded Info.
/// UTC is supplied by the existing runtime clock, never derived from Tick milliseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InfoPresentation {
    pub opacity: crate::player_hud::model::OsdOpacity,
    pub wall_now_s: Option<i64>,
}

/// The card's whole state, owned by the container that mounts this panel — the modal PHASE and the
/// appear spring belong to `ui::containers::modal::ModalStack` now, not to this struct; `draw`
/// takes the appear fraction as a parameter instead of stepping its own `Popover`.
pub struct InfoPanelState {
    focus: c_int, // index into the action-button column
    /// The action column's FOCUS POP — one spring per button ([`plx_ui::widgets::CtlPop`]). Two,
    /// the whole of [`actions`].
    ctl_pop: plx_ui::widgets::CtlPop<2>,
    classic_scroll: plx_ui::prose_scroll::ProseScroll,
    classic_content: Option<(String, String, i64)>,
    classic_max_scroll: f32,
    classic_pages: usize,
    classic_step: f32,
}

/// The ratingKey of the page the playing item belongs to: the show for an episode, the movie
/// itself otherwise. What the Info card's second action opens, and what the `go_to_detail`
/// command opens and is enabled by. `None` when nothing playing has one.
/// OTT metadata belongs to the playing source; an unrelated Plex detail page
/// must not fill an absent description, artwork, track list or action target.
fn fallback_detail(meta: metadata::MetadataView<'_>) -> Option<&metadata::Detail> {
    if meta.now_playing().is_some_and(|playing| playing.ott.is_some()) { None }
    else { meta.current() }
}

pub fn detail_rk<'a>(meta: metadata::MetadataView<'a>) -> Option<&'a str> {
    meta.now_playing()
        .map(|n| n.detail_rk.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| fallback_detail(meta).map(|d| d.rk.as_str()))
        .filter(|s| !s.is_empty())
}

impl InfoPanelState {
    pub fn new() -> Self {
        InfoPanelState {
            focus: 0,
            ctl_pop: plx_ui::widgets::CtlPop::new(),
            classic_scroll: plx_ui::prose_scroll::ProseScroll::new(),
            classic_content: None,
            classic_max_scroll: 0.0,
            classic_pages: 1,
            classic_step: 0.0,
        }
    }

    /// The focused action button, for the focus probe (`crate::focusprobe`). Same reason as the
    /// other panels: the card's UP/DOWN arm moves this alone, so nothing else in the log would show
    /// it.
    pub fn sel(&self) -> c_int {
        self.focus
    }

    /// true when focus is on the last action button — a further DOWN should leave the card (back to
    /// the tabs) rather than staying pinned to the bottom row
    pub fn at_last(&self, can_go_to_detail: bool) -> bool {
        self.focus >= action_count(can_go_to_detail) as c_int - 1
    }

    /// **Write back the engine's own focus cursor** (restructure phase 12): the Column group
    /// [`InfoPanelPart`] answers is the source of geometry, but the ENGINE owns the current
    /// element (§7.3 step 5) — the owner's `step` is the only place that mutates in response to a
    /// `FocusMoved`, and this is `screens::player::overlay::PlayerOverlayScreen::step`'s write.
    pub fn set_focus(&mut self, i: c_int) {
        self.focus = i;
    }

    /// activate the focused action — dismissing the card afterward is the container's job now, not
    /// this method's.
    pub fn on_ok_for_mode(&self, meta: metadata::MetadataView<'_>, mode: UiMode) -> InfoAction {
        if mode == UiMode::Classic { InfoAction::None } else { self.on_ok(meta) }
    }

    pub fn on_ok(&self, meta: metadata::MetadataView<'_>) -> InfoAction {
        let f = self.focus;
        if f <= 0 {
            return InfoAction::FromBeginning;
        }
        // second action opens the show (episode) or the movie
        detail_rk(meta).map_or(InfoAction::None, |rk| InfoAction::GoToDetail(rk.to_owned()))
    }

    /// **Which action button `focus` is on, if it is in the column at all** — asked by
    /// [`update`](Self::update) to decide which button `ctl_pop` pops. Under [`InfoPanelPart`]'s
    /// Engine groups this is always `Some`: the card's only focus group IS the action column
    /// (`ElemKind::Control`), so a focus that reaches this screen at all is always on a control
    /// face — there is no longer a "tabs above" state sharing `focus`'s range to distinguish it
    /// from (restructure phase 12 retired the `focus_is_ctl` predicate this used to also answer,
    /// once `screens::player::overlay::PlayerOverlayScreen`'s own `Key::Ok` arm stopped asking it:
    /// every `ScreenEvent::PressCommit` this card's engine-armed press delivers already IS a
    /// control-face press).
    fn ctl_index(&self) -> Option<usize> {
        usize::try_from(self.focus)
            .ok()
            .filter(|&i| i < N_ACTIONS)
    }

    pub fn update(&mut self, dt: f32) {
        // The card's focus walks the tabs above these buttons too, and `focus` is the button index
        // only while it is inside the column — outside it, every pop closes.
        let idx = self.ctl_index();
        self.ctl_pop.step(idx, dt);
        if self.classic_content.is_some() {
            self.classic_scroll.follow(dt, self.classic_max_scroll, self.classic_pages,
                self.classic_step, "info.classic.prose");
        }
    }

    pub fn canon(&self, c: &mut plx_machine::machine::Canon) {
        c.u32(self.focus as u32);
        c.bool(self.classic_content.is_some());
        if let Some((title, summary, duration)) = &self.classic_content {
            c.str(title).str(summary).u64(*duration as u64);
        }
        c.f32(self.classic_max_scroll).u64(self.classic_pages as u64).f32(self.classic_step);
        self.classic_scroll.write(c);
    }

    /// Classic expands the existing bottom information band; Modern retains the card.
    pub fn draw_for_mode(
        &mut self, ps: &plx_media::route::PlaybackSession, appear: f32,
        measure: &dyn plx_machine::machine::Measure, meta: metadata::MetadataView<'_>, mode: UiMode,
        presentation: InfoPresentation,
    ) {
        if mode != UiMode::Classic { return self.draw(ps, appear, measure, meta); }
        let copy = classic_copy(meta.now_playing(), fallback_detail(meta), presentation.wall_now_s);
        if copy.title.is_empty() && copy.summary.is_empty() { return; }
        let footer_top = crate::player_hud::classic::classic_footer_top(meta.now_playing(), measure);
        // One attached surface: no floating-card rise creates a gap over the footer.
        self.draw_classic(Painter::root().alpha(appear), &copy, footer_top, measure, presentation.opacity);
    }

    /// Reconcile at Enter/Tick/publication, never from draw or resource preparation.
    /// This is the only place metadata replacement resets the logical reading position.
    pub fn reconcile_classic(&mut self, meta: metadata::MetadataView<'_>,
        measure: &dyn plx_machine::machine::Measure, wall_now_s: Option<i64>) -> bool {
        let copy = classic_copy(meta.now_playing(), fallback_detail(meta), wall_now_s);
        let footer_top = crate::player_hud::classic::classic_footer_top(meta.now_playing(), measure);
        self.reconcile_classic_copy(&copy, footer_top, measure)
    }

    /// Shared prose paging owns the scroll. The caller has reconciled the current
    /// publication before consuming the direction, including at the endpoints.
    pub fn scroll_classic(&mut self, down: bool) {
        self.classic_scroll.step_page(if down { plx_ui::consts::SDLK_DOWN } else { plx_ui::consts::SDLK_UP });
        self.classic_scroll.page = self.classic_scroll.page.clamp(1, self.classic_pages);
    }

    fn reconcile_classic_copy(&mut self, copy: &ClassicCopy<'_>, footer_top: f32,
        measure: &dyn plx_machine::machine::Measure) -> bool {
        let replaced = self.classic_content.as_ref().is_none_or(|(title, summary, duration)|
            title != copy.title || summary != copy.summary || *duration != copy.duration_ms);
        if replaced {
            self.classic_scroll = plx_ui::prose_scroll::ProseScroll::new();
            self.classic_content = Some((copy.title.to_owned(), copy.summary.to_owned(), copy.duration_ms));
        }
        let layout = ClassicLayout::new(copy, footer_top, measure);
        let step = plx_ui::prose_scroll::step_for(measure.font_scale().points(CLASSIC_LEADING));
        let (max_scroll, pages) = plx_ui::prose_scroll::paging(layout.content_h, layout.body.h, step);
        let page = self.classic_scroll.page.clamp(1, pages);
        let changed = replaced || max_scroll != self.classic_max_scroll || pages != self.classic_pages
            || step != self.classic_step || page != self.classic_scroll.page;
        self.classic_max_scroll = max_scroll;
        self.classic_pages = pages;
        self.classic_step = step;
        self.classic_scroll.page = page;
        changed
    }

    fn draw_classic(&self, p: Painter, copy: &ClassicCopy<'_>, footer_top: f32,
        measure: &dyn plx_machine::machine::Measure, opacity: crate::player_hud::model::OsdOpacity) {
        let layout = ClassicLayout::new(copy, footer_top, measure);
        let background = theme::with_a(plx_ui::palette::color(plx_ui::palette::Role::Surface, theme::CLASSIC_BACKGROUND), opacity.background_alpha(0.78));
        p.rect(layout.panel, 0.0, background, background, 0.0);
        p.rect(Rect::new(layout.panel.x, layout.panel.y, layout.panel.w, 1.0), 0.0,
            plx_ui::palette::color(plx_ui::palette::Role::Border, theme::CLASSIC_BORDER), plx_ui::palette::color(plx_ui::palette::Role::Border, theme::CLASSIC_BORDER), 0.0);
        classic_title(copy.title, measure).draw(p, layout.title);
        if copy.duration_ms > 0 {
            TextView::new(&plx_ui::fmt::duration(copy.duration_ms), theme::size::CAPTION, plx_ui::palette::color(plx_ui::palette::Role::Muted, theme::CLASSIC_SECONDARY))
                .with_measure(measure).draw(p, layout.duration);
        }
        if !copy.summary.is_empty() {
            plx_ui::prose_scroll::draw_measured(p, &[copy.summary], CLASSIC_LEADING, layout.body,
                self.classic_scroll.offset(self.classic_max_scroll), self.classic_max_scroll, measure);
            plx_ui::prose_scroll::draw_rail(p, layout.body, layout.content_right,
                self.classic_scroll.page, self.classic_pages);
        }
    }

    pub fn draw(
        &mut self,
        ps: &plx_media::route::PlaybackSession,
        appear: f32,
        measure: &dyn plx_machine::machine::Measure,
        meta: metadata::MetadataView<'_>,
    ) {
        let np = meta.now_playing();
        let d = fallback_detail(meta);
        if np.is_none() && d.is_none() {
            return;
        }
        // no scrim — the card floats over the transport; reproduces exactly what
        // `Popover::painter(0.0, 20.0)` (i.e. no scrim + `Popover::content_painter(20.0)`) drew.
        let p = Painter::root()
            .alpha(appear)
            .translate(0.0, plx_ui::popover::Popover::RISE * (1.0 - appear));

        // Resolve the playing leaf's fields: `now_playing` describes the episode (show title + SxEy
        // + its still) or the movie; the loaded `Detail` backs the capability badges + genres.
        let is_ep = np.map(|n| n.is_episode).unwrap_or(false);
        let big_title = np
            .map(|n| n.title.clone())
            .or_else(|| d.map(|x| x.title.clone()))
            .unwrap_or_default();
        let ep_name = np.map(|n| n.ep_title.clone()).unwrap_or_default();
        let summary = description(meta).to_owned();
        let year = np
            .map(|n| n.year)
            .or_else(|| d.map(|x| x.year))
            .unwrap_or(0);
        let dur_ms = np
            .map(|n| n.dur_ms)
            .or_else(|| d.map(|x| x.dur_ms))
            .unwrap_or(0);
        let rating = np
            .map(|n| n.rating.clone())
            .filter(|s| !s.is_empty())
            .or_else(|| d.map(|x| x.rating.clone()))
            .unwrap_or_default();
        let thumb_path = np
            .map(|n| n.thumb.clone())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                d.map(|x| {
                    if !x.art.is_empty() {
                        x.art.clone()
                    } else {
                        x.thumb.clone()
                    }
                })
            })
            .unwrap_or_default();
        // capability badges come from the PLAYING item's own tracks — `current()` is the show
        // (episode-1 streams) during a show-page episode play, or another item entirely
        let (audio, subs): (&[metadata::Stream], &[metadata::Stream]) = match meta.playing() {
            Some(t) => (&t.audio, &t.subs),
            None => (
                d.map(|x| x.audio.as_slice()).unwrap_or(&[]),
                d.map(|x| x.subs.as_slice()).unwrap_or(&[]),
            ),
        };

        // card — tall enough that the still gets equal padding on every side (see `pad`/`sh` below)
        let (card, pad) = card_geometry();
        let (cx, cyt, _cw, ch) = (card.x, card.y, card.w, card.h);
        // near-opaque dark card keeps the title/synopsis legible over any scene
        let cardbg = plx_ui::palette::color(plx_ui::palette::Role::Surface, theme::PANEL_TOP);
        p.rrect(card, 28.0, 28.0, cardbg);

        // still (16:9), left — the *episode's* thumbnail (or the movie's landscape art). `ch` is sized
        // so (ch - sh)/2 == pad, giving the still an equal `pad` margin on every side.
        let sw = 320.0f32;
        let sh = 180.0f32;
        let sx = cx + pad;
        let sy = cyt + (ch - sh) * 0.5;
        let mut drawn = false;
        if let Some(art) = np.and_then(|playing| playing.ott.as_ref()).and_then(|info| info.art.as_ref()) {
            let art = match art {
                metadata::MediaArt::Local { key } => plx_ui::widgets::Art::Local { key },
                metadata::MediaArt::Http { url } => plx_ui::widgets::Art::Http { url, res: (480, 270) },
                metadata::MediaArt::Plex { server, key } => plx_ui::widgets::Art::Thumb { sid: *server, key, res: (480, 270) },
            };
            plx_ui::widgets::card_named(p, Rect::new(sx, sy, sw, sh), art, None, 16.0, false, 1.0, 0.0);
            drawn = true;
        }
        if !drawn && !np.is_some_and(|playing| playing.ott.is_some()) && !thumb_path.is_empty() {
            // the PLAYING item's server — the info panel describes what is on the video plane
            let (t, tw, th) = resolve_tex_wh_on(
                plx_media::route::item_sid(plx_media::route::cur_sid(ps)).raw(),
                &thumb_path,
                480,
                270,
                0,
            );
            if t != 0 {
                // cropped to the 16:9 box, never squashed: a 4:3 still or a movie's art of any
                // aspect comes back at its own shape (`minSize=1` covers the box, it does not fit it)
                let still = Rect::new(sx, sy, sw, sh);
                let uv = still.cover_uv(tw, th, plx_ui::Crop::Centre);
                p.tex_uv(t, uv, still, 16.0, theme::TINT_WHITE);
                drawn = true;
            }
        }
        if !drawn {
            plx_ui::placeholder::flat(
                p,
                plx_ui::placeholder::Reason::InfoPanelStill,
                &thumb_path,
                Rect::new(sx, sy, sw, sh),
                16.0,
            );
        }

        // action buttons (right column)
        let acts = actions(meta);
        let focus = self.focus;
        let env = plx_ui::Env::inert();
        for (i, label) in acts.iter().take(action_count(detail_rk(meta).is_some())).enumerate() {
            let icon = if i == 0 {
                Icon::Play
            } else {
                Icon::Info
            };
            if let Ok(cs) = CString::new(*label) {
                plx_ui::widgets::Button::new(cs.as_ptr(), theme::size::BODY, button_rect(i, measure))
                    .icon(icon)
                    .focused(i as c_int == focus)
                    .scale(self.ctl_pop.scale(i))
                    .draw(&env, p);
            }
        }

        // text block (between the still and the buttons): title + synopsis + tags, cap-band centred as a
        // group. Title is the playing leaf's own name (episode name / movie title) — the show-title +
        // SxEy treatment lives on the transport HUD under the playbar, not this card.
        let bx = button_rect(0, measure).x; // the action column's own left edge, every row shares it
        let tx = sx + sw + 34.0;
        let tright = bx - 34.0;
        let tw = tright - tx;
        let white = plx_ui::palette::color(plx_ui::palette::Role::Text, theme::TEXT_PRIMARY);
        let dim = plx_ui::palette::color(plx_ui::palette::Role::Muted, theme::TEXT_SECONDARY);

        let info_title = if is_ep {
            ep_name.clone()
        } else {
            big_title.clone()
        };
        // What the server is actually sending, straight off the resolved route — see `playback_now`.
        // `has_url`, not `!url().is_empty()`: this card sits on the one route exempt from the idle
        // present gate, so it draws at ~60/s, and `url()` clones the longest string in the app (a
        // universal-transcode `start.mkv` query is several hundred bytes) purely to test emptiness.
        // The `play_pending` half is the resolve window — see the doc.
        let now_fact = playback_now(
            plx_media::route::has_url(ps) && !plx_media::route::play_pending(),
            plx_media::route::is_transcoding(ps),
            plx_media::route::is_remux(ps),
            &plx_media::route::stream_vcodec(ps),
        );
        // metadata line (genres · year · duration) + capability badges. Built and FIT here, before the
        // title/synopsis layout below, rather than inside its own draw block further down — see
        // [`ChipRow`]'s doc for issue #26 itself; the reason this part specifically has to happen
        // before `span` is a second, smaller bug the same fix could otherwise reintroduce. `span` folds
        // in a fixed `gap_tags + tag_h` for this row whenever it might have anything to say
        // (`has_tags`), which used to be exactly right because the row always drew SOMETHING once
        // `has_tags` was true. Once a row can be fit down to nothing at all — a single candidate (one
        // very long genre string, say) wider than `tw` on its own — `has_tags` and "this row draws a
        // pixel" stop being the same question, and centring the title/synopsis group as though a tag
        // row existed would leave a blank gap where one doesn't. So the row's actual chip COUNT, `n`,
        // has to be known before `span` is computed, not decided by a separate boolean.
        let has_tags = year > 0
            || dur_ms > 0
            || !rating.is_empty()
            || d.map(|x| !x.genres.is_empty()).unwrap_or(false)
            || !subs.is_empty()
            || audio.iter().any(|s| s.ad)
            || now_fact.is_some()
            || audio.first().and_then(|s| audio_badge(&s.codec)).is_some();

        // `has_tags` is the cheap pre-filter (a handful of comparisons, no allocation) that skips this
        // entirely for the common "nothing at all to report" case; once it's true the row still might
        // fit zero chips (see above), which is exactly what `n` then says.
        let mut chips = ChipRow::new();
        let (kept, n) = if has_tags {
            let mut meta = Vec::new();
            if let Some(x) = d {
                for g in x.genres.iter().take(2) {
                    meta.push(g.clone());
                }
            }
            if year > 0 {
                meta.push(year.to_string());
            }
            if dur_ms > 0 {
                meta.push(plx_ui::fmt::duration(dur_ms));
            }
            // The item's own facts are ONE text run, its parts a dot apart exactly as the Detail
            // page's identity line spaces them (`widgets::DottedRun`: `space::SM` each side
            // of the dot), and the badges follow at the same `SM` / `XS` the identity line gives its
            // badges. Nothing about the spacing is baked into a string. Every candidate's width is
            // known before any of them touches the screen.
            let has_meta = !meta.is_empty();
            if has_meta {
                chips.push(Chip::run(&meta, false, 1, white, measure));
            }
            // …then the live playback fact, in its own quieter ink. It is a separate run rather than
            // one more entry joined into `meta` because it is a different KIND of statement — a fact
            // about this session, not a property of the item — and tertiary is what says so. No chip
            // and no capsule: a conversion that worked is not a warning.
            //
            // REGULAR weight, unlike the item run beside it, and that is the mock's: its whole meta line
            // is `font-weight:400` at `--text-tertiary`. The item run's bold/primary is this screen's own
            // long-standing deviation (the card is read over live video, not over a panel); repeating it
            // on a run the mock has no bold in would make the quiet fine print the loudest thing on the
            // row.
            //
            // It continues the meta run, so it leads with a dot when there is a meta run. Whether it is
            // needed depends only on whether the meta line EXISTS, which is known right here — not on
            // whether the meta line ends up FITTING. By construction a chip only draws once every
            // higher-priority chip already fit (see [`ChipRow`]'s tail-first drop), so by the time
            // this fact chip is actually on screen those two questions have the same answer.
            if let Some(fact) = now_fact.as_ref() {
                chips.push(Chip::run(
                    std::slice::from_ref(fact),
                    has_meta,
                    0,
                    plx_ui::palette::color(plx_ui::palette::Role::Muted, theme::TEXT_TERTIARY),
                    measure,
                ));
            }
            // badges: rating (from the leaf), top-audio Dolby tag, CC/SDH/AD (from the loaded streams)
            if !rating.is_empty() {
                chips.push(Chip::badge(rating.clone(), measure));
            }
            if let Some(tag) = audio.first().and_then(|s| audio_badge(&s.codec)) {
                chips.push(Chip::badge(tag, measure));
            }
            if !subs.is_empty() {
                chips.push(Chip::badge(plx_platform::i18n::msg::widgets_badge_cc().to_string(), measure));
            }
            if subs.iter().any(|s| s.sdh) {
                chips.push(Chip::badge(plx_platform::i18n::msg::widgets_badge_sdh().to_string(), measure));
            }
            if audio.iter().any(|s| s.ad) {
                chips.push(Chip::badge(plx_platform::i18n::msg::widgets_badge_ad().to_string(), measure));
            }

            let row = chips.row(tx);
            let kept = row.fit(tw);
            (kept, row.kept(kept).placed().count())
        } else {
            (Kept::ALL, 0)
        };

        // vertical rhythm — line *advances* (deliberately below the full font line-box) + small gaps
        let title_h = 42.0f32; // title advance (font 40)
        // Issue #29: the synopsis is reading copy (the longest run of prose on this card), which is
        // exactly the case `ui/CLAUDE.md` rule 2 already settled — the hero blurb is `size::LABEL` 26
        // through `ui::hero_synopsis`, one rung below plain `size::BODY`, precisely because a blurb
        // reads as prose rather than as chrome. This card built its own synopsis at `BODY` instead of
        // going through that shared helper (it needs its own 2-line cap and card-local ink, not the
        // hero's 3-line one), so it never inherited the correction; stepping it to `LABEL` brings the
        // player's synopsis in line with both heroes' own settled rung. The line advance keeps the same
        // `font + 3` rhythm the card already used at `BODY` (31 = 28 + 3), rather than carrying the old
        // px figure forward onto a smaller font, which would leave the new rung looking loose.
        let syn_lh = theme::size::LABEL as f32 + 3.0; // synopsis line advance (font 26) — 29
        let tag_h = 34.0f32;
        // Off-ladder gaps, kept at their painted values (named, not rounded to a rung).
        let gap_title = 6.0f32; // title → synopsis
        let gap_tags = 12.0f32; // whatever is above → the tag row (title → tags when there is no synopsis)

        // title (1 line, elided) + synopsis (up to 2 lines, ellipsized) through the shared TextView —
        // its wrap is memoised internally, replacing this panel's old hand-rolled wrap2/WrapCache.
        let title_v = TextView::new(&info_title, theme::size::TITLE, white)
            .bold()
            .max_lines(1);
        let syn_v = TextView::new(&summary, theme::size::LABEL, dim)
            .leading(syn_lh)
            .max_lines(2);
        let syn_h = if summary.is_empty() {
            0.0
        } else {
            syn_v.measure_h(tw)
        };

        // centre the [title + synopsis + tag row] group in the card (cap-top coordinates). The tag
        // row's contribution is gated on `n > 0` — whether the chip row ACTUALLY has something to draw
        // — not `has_tags`; see the comment above where `chips`/`n` are built for why those are two
        // different questions once a fit can drop every candidate.
        let items = card_items(title_h, syn_h, gap_tags, (n > 0).then_some(tag_h));
        let stack = VStack::new(&items, gap_title, Anchor::Centre(cyt + ch * 0.5));
        if let Some((y, _)) = stack.span(0) {
            title_v.draw(p, Rect::new(tx, y, tw, 0.0));
        }
        if let Some((y, _)) = stack.span(1) {
            syn_v.draw(p, Rect::new(tx, y, tw, 0.0));
        }
        // metadata line (genres · year · duration) + capability badges, centred on the tag row —
        // `chips`/`n` were already measured and fit above, so this is draw-only.
        //
        // **Issue #26.** This row used to draw every candidate unconditionally and let `mx` run past
        // the action column whenever an item had enough facts — a rating, a premium audio tag, CC, SDH
        // and AD can all be true of one stream at once — so the chips drew straight under "From
        // Beginning"/"Go to Show". The fix constrains the row to `tw`, the SAME right edge the title
        // and synopsis above it already respect, and decides what fits by measuring every candidate
        // BEFORE drawing any of it, in a fixed priority order: the item's own facts (genres/year/
        // duration) first, then the live playback fact (what the server is actually sending — the
        // technical heart of this panel, and the one thing here that can surprise a viewer), then the
        // rating badge, then the premium-audio badge, then the accessibility badges CC/SDH/AD — each
        // rarer, and each a smaller part of why this panel got opened, than the one before it. Dropped
        // chips are hidden outright rather than faded: each one is a discrete fact (a word, a bordered
        // badge), and a badge sliced in half by a fade band reads as a rendering bug, not as "there was
        // more" — a clean drop keeps every chip that IS shown fully legible, which a fade cannot
        // promise once it crosses a border. [`ChipRow`] + `HStack::fit` is the pure packer that turns the
        // measured candidates into a cut, so the maths is pinned by a host test with no font involved.
        if let Some((y, _)) = stack.span(2) {
            let my = y + tag_h * 0.5; // vertical centre of the tag row
            draw_chip_row(p, &chips, chips.row(tx).kept(kept), my, measure);
        }
    }
}

/// The card's title / synopsis / tag-row column. `syn_h` of zero and `tags` of `None` are absent.
/// The tag row carries its gap as `before`, not the stack gap: with no synopsis it still sits
/// `gap_tags` under the title (the stack's own gap is title -> synopsis).
fn card_items(title_h: f32, syn_h: f32, gap_tags: f32, tags: Option<f32>) -> [Item; 3] {
    [
        Item::new(title_h),
        if syn_h > 0.0 { Item::new(syn_h) } else { Item::absent() },
        tags.map_or(Item::absent(), |h| Item::new(h).before(gap_tags)),
    ]
}

/// The expanded Classic block uses the same reading rung as the shared prose reader.
const CLASSIC_LEADING: f32 = theme::size::BODY as f32 * 1.35;

struct ClassicCopy<'a> {
    title: &'a str,
    summary: &'a str,
    duration_ms: i64,
}

fn classic_copy<'a>(now: Option<&'a metadata::NowPlaying>, detail: Option<&'a metadata::Detail>, wall_now_s: Option<i64>) -> ClassicCopy<'a> {
    if let Some(now) = now {
        if let Some(live) = now.ott.as_ref().and_then(|ott| ott.live.as_ref()) {
            if let Some(programme) = live.guide.as_ref()
                .and_then(|guide| guide.at(wall_now_s.unwrap_or(guide.now_s)).0) {
                return ClassicCopy { title: &programme.title, summary: &programme.description,
                    duration_ms: programme.end_s.saturating_sub(programme.start_s).saturating_mul(1000) };
            }
            // The retained guide no longer owns a current programme. now.summary can
            // still describe its expired row, so it is not a channel-level fallback.
            return ClassicCopy { title: &live.channel_title, summary: "", duration_ms: 0 };
        }
        // A playing leaf is authoritative, including its intentionally empty description.
        return ClassicCopy {
            title: if now.is_episode && !now.ep_title.is_empty() { &now.ep_title } else { &now.title },
            summary: &now.summary, duration_ms: now.dur_ms,
        };
    }
    match detail {
        Some(detail) => ClassicCopy { title: &detail.title, summary: &detail.summary, duration_ms: detail.dur_ms },
        None => ClassicCopy { title: "", summary: "", duration_ms: 0 },
    }
}

fn classic_title<'a>(title: &'a str, measure: &'a dyn plx_machine::machine::Measure) -> TextView<'a> {
    TextView::new(title, theme::size::BODY, plx_ui::palette::color(plx_ui::palette::Role::Text, theme::CLASSIC_TEXT)).bold()
        .with_measure(measure).break_long_words().max_lines(2)
}

struct ClassicLayout {
    panel: Rect,
    title: Rect,
    duration: Rect,
    body: Rect,
    content_h: f32,
    content_right: f32,
}
impl ClassicLayout {
    fn new(copy: &ClassicCopy<'_>, footer_top: f32, measure: &dyn plx_machine::machine::Measure) -> Self {
        // The shared Classic frame owns the safe reading edges. The expanded block is
        // attached to the footer, with no thumbnail or button column consuming its width.
        let frame = plx_ui::classic::Layout::new(Rect::FULL, measure, false).body;
        let x = frame.x + theme::space::LG;
        let width = (frame.w - 2.0 * theme::space::LG).max(1.0);
        let body_w = plx_ui::prose_scroll::text_w(width);
        let title_h = if copy.title.is_empty() { 0.0 } else { classic_title(copy.title, measure).measure_h(width) };
        let duration_h = if copy.duration_ms > 0 {
            TextView::new(&plx_ui::fmt::duration(copy.duration_ms), theme::size::CAPTION, plx_ui::palette::color(plx_ui::palette::Role::Muted, theme::CLASSIC_SECONDARY))
                .with_measure(measure).measure_h(width)
        } else { 0.0 };
        let content_h = if copy.summary.is_empty() { 0.0 } else {
            plx_ui::prose_scroll::content_h_measured(&[copy.summary], body_w, CLASSIC_LEADING, measure)
        };
        let pad = theme::space::MD;
        let gap = theme::space::SM;
        let heading = [if title_h > 0.0 { Item::new(title_h) } else { Item::absent() }, if duration_h > 0.0 { Item::new(duration_h) } else { Item::absent() }];
        let heading_h = VStack::new(&heading, gap, Anchor::Top(0.0)).height();
        let room = (footer_top - plx_ui::consts::MARGIN_Y - 2.0 * pad - heading_h - gap).max(0.0);
        let body_h = content_h.min(room.min(SCR_H * 0.40));
        let items = [heading[0], heading[1], if body_h > 0.0 { Item::new(body_h) } else { Item::absent() }];
        let stack = VStack::new(&items, gap, Anchor::Bottom(footer_top - pad));
        let mut rects = [Rect::new(x, footer_top - pad, width, 0.0); 3];
        for span in stack.placed() { rects[span.i] = Rect::new(x, span.y, width, span.h); }
        rects[2].w = body_w;
        let top = footer_top - 2.0 * pad - stack.height();
        Self { panel: Rect::new(0.0, top, SCR_W, footer_top - top),
            title: rects[0], duration: rects[1], body: rects[2], content_h, content_right: x + width }
    }
}

/// **The Engine-shaped view of this card** (restructure phase 12): one `Column` focus group over
/// the available action buttons, built fresh by `screens::player::overlay::PlayerOverlayScreen` each
/// frame from a `&InfoPanelState` — the same borrowed-view shape `appkit::more_menu::MoreMenuPart`/
/// `appkit::track_menu::TrackMenuPart` use for the other player panels, so the card answers the same
/// [`Focusable`]/[`Part`] query protocol they do. DOWN off the last button is `EdgeRule::Screen` —
/// re-delivered to the owning screen's own `step`, which drops focus back onto the HUD tabs; UP
/// off the first is `Stop`, matching the old ladder's clamp.
///
/// **`state` is a SHARED reference** — every [`Focusable`] method here is a pure read (`&self`),
/// and the owning screen's own `Focusable` impl only ever has `&self` too (§7.1: "the engine never
/// mutates a screen"), so a mutable field would make this type unconstructable from there. The
/// actual paint (`InfoPanelState::draw`) stays a direct call on the owned `Panel` from
/// `PlayerOverlayScreen::draw`'s `&mut self`; [`Part::draw`] below only registers stops.
/// Classic publishes no action groups; its expanded description is read-only.
pub struct InfoPanelPart<'a> {
    pub state: &'a InfoPanelState,
    pub entry: EntryId,
    pub group: GroupId,
    pub can_go_to_detail: bool,
    pub mode: UiMode,
}

impl InfoPanelPart<'_> {
    fn action_count(&self) -> usize {
        if self.mode == UiMode::Classic { 0 } else { action_count(self.can_go_to_detail) }
    }
}

fn action_count(can_go_to_detail: bool) -> usize {
    1 + usize::from(can_go_to_detail)
}

impl<H: Host> Focusable<H> for InfoPanelPart<'_>
where
    H::Elem: IndexElem,
{
    fn groups(&self, _cx: &Cx<'_, H>, out: &mut Vec<GroupSpec>) {
        if self.action_count() == 0 { return; }
        out.push(GroupSpec {
            id: self.group,
            kind: GroupKind::Column,
            seat: Seat::Remembered,
            reachable: AxisMask::VERTICAL,
            edge: [EdgeRule::Stop, EdgeRule::Screen, EdgeRule::Stop, EdgeRule::Stop],
            extent: card_geometry().0,
            len: self.action_count(),
            elem: ElemKind::Control,
        });
    }
    fn group_of(&self, key: &H::Elem, _cx: &Cx<'_, H>) -> Option<GroupId> {
        ((key.index()? as usize) < self.action_count()).then_some(self.group)
    }
    fn neighbour(&self, key: FocusKey<H::Elem>, dir: Dir, _cx: &Cx<'_, H>) -> Step<H::Elem> {
        let Some(i) = key.elem.index() else {
            return Step::Edge;
        };
        let n = self.action_count() as u32;
        if i >= n { return Step::Edge; }
        match dir {
            Dir::Up if i > 0 => Step::Move(FocusKey { entry: self.entry, elem: H::Elem::of_index(i - 1) }),
            Dir::Down if i + 1 < n => Step::Move(FocusKey { entry: self.entry, elem: H::Elem::of_index(i + 1) }),
            _ => Step::Edge,
        }
    }
    fn place(&self, key: &H::Elem, cx: &Cx<'_, H>, _at: At) -> Option<Placed> {
        let i = key.index()? as usize;
        if i >= self.action_count() {
            return None;
        }
        let r = button_rect(i, cx.measure);
        Some(Placed {
            rect: r,
            rest_rect: r,
            clip: card_geometry().0,
            index: Some(i as u32),
        })
    }
    fn reconcile(&self, want: FocusKey<H::Elem>, _cx: &Cx<'_, H>) -> FocusKey<H::Elem> {
        let i = plx_ui::geom::clamp_plain_index(want.elem.index().unwrap_or(0), self.action_count());
        FocusKey { entry: self.entry, elem: H::Elem::of_index(i as u32) }
    }
    fn seat(&self, _g: GroupId, _from: Placed, _cx: &Cx<'_, H>) -> FocusKey<H::Elem> {
        FocusKey {
            entry: self.entry,
            elem: H::Elem::of_index(plx_ui::geom::clamp_plain_index(self.state.focus.max(0) as u32, self.action_count()) as u32),
        }
    }
}

impl<H: Host> Part<H> for InfoPanelPart<'_>
where
    H::Elem: IndexElem,
{
    fn prepare(&mut self, _b: &mut Budget, _cx: &Cx<'_, H>) {}
    /// Registers each action button's stop (§7.6); the card's own paint happens directly on the
    /// owned `InfoPanelState` from `PlayerOverlayScreen::draw` (see the struct doc above).
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, H>, _rect: Rect) {
        let p = Painter::root();
        for i in 0..self.action_count() {
            let r = button_rect(i, f.measure);
            f.stop(
                p,
                Stop {
                    key: FocusKey {
                        entry: self.entry,
                        elem: H::Elem::of_index(i as u32),
                    },
                    rect: r,
                    rest_rect: r,
                    clip: card_geometry().0,
                    hover: Hover::Focus,
                    activate: Activate::Immediate,
                },
            );
        }
    }
}

/// whether the playing item is an episode (→ "Go to Show") rather than a movie ("Go to Movie").
/// A trailer session does not install a playing-leaf descriptor, so with `now_playing` absent
/// the loaded page is the parent — a show's trailer still says Go to Show.
fn is_episode(meta: metadata::MetadataView<'_>) -> bool {
    meta.now_playing()
        .map(|n| n.is_episode)
        .unwrap_or_else(|| meta.current().is_some_and(|d| d.is_show))
}

/// the action-button labels for the playing item. An ARRAY, not a `Vec`: the count is fixed at
/// [`N_ACTIONS`] (it is what `CtlPop`'s const generic is sized from) — every caller that only
/// needs the count uses that constant instead, so this is called only where the actual label TEXT
/// is drawn ([`InfoPanelState::draw`]).
fn actions(meta: metadata::MetadataView<'_>) -> [&'static str; 2] {
    [
        plx_platform::i18n::msg::widgets_info_from_beginning(),
        if is_episode(meta) {
            plx_platform::i18n::msg::widgets_info_go_show()
        } else {
            plx_platform::i18n::msg::widgets_info_go_movie()
        },
    ]
}

/// The card's own rect and its side padding — **pure**, since every input is a fixed layout
/// constant (`SCR_W`/`SCR_H` alone). Shared by [`InfoPanelState::draw`] (the still/title/synopsis
/// layout) and [`button_rect`] (the action column), so the two formulas can never drift apart —
/// they used to be two copies of the same five numbers, seven lines apart.
pub(crate) fn card_geometry() -> (Rect, f32) {
    let cx = 80.0f32;
    let cw = SCR_W - 160.0;
    let ch = 236.0f32;
    let cyt = SCR_H - 176.0 - ch; // sit just above the Info/Chapters tabs (tabs at SCR_H-128)
    (Rect::new(cx, cyt, cw, ch), 28.0f32)
}

/// The `i`-th action button's rect, widened for the longest translated action label. The same
/// measured formula [`InfoPanelPart::place`] answers the focus engine with, so a stop built from it lands
/// on the pixel the button was drawn at.
pub(crate) fn button_rect(i: usize, measure: &dyn plx_machine::machine::Measure) -> Rect {
    let (card, pad) = card_geometry();
    let labels = [
        plx_platform::i18n::msg::widgets_info_from_beginning_c(),
        plx_platform::i18n::msg::widgets_info_go_show_c(),
        plx_platform::i18n::msg::widgets_info_go_movie_c(),
    ];
    let bw = labels.iter().map(|label| {
        plx_ui::widgets::Button::pill_w_measured(label, theme::size::BODY, true, None, measure)
    }).fold(352.0f32, f32::max);
    let bh = 70.0f32;
    let bx = card.x + card.w - pad - bw;
    let n = N_ACTIONS;
    let total_bh = n as f32 * bh + n.saturating_sub(1) as f32 * 16.0;
    let by0 = card.y + (card.h - total_bh) * 0.5;
    Rect::new(bx, by0 + i as f32 * (bh + 16.0), bw, bh)
}

// ---- helpers ----

/// A premium audio format worth badging on the meta line, named by the ONE codec map
/// ([`metadata::friendly_codec`]); everyday codecs (AAC/MP3/…) get no badge.
fn audio_badge(codec: &str) -> Option<String> {
    matches!(
        codec.to_ascii_lowercase().as_str(),
        "truehd" | "eac3" | "ec-3" | "ac3" | "dts" | "dca"
    )
    .then(|| metadata::friendly_codec(codec))
}

/// the shared outlined chip in this panel's colours (TEXT_HEADING border/label over the card)
fn meta_badge(
    p: Painter,
    x: f32,
    cy: f32,
    text: &str,
    measure: &dyn plx_machine::machine::Measure,
) -> f32 {
    badge(
        p,
        x,
        cy,
        text,
        None,
        BadgeStyle::Outlined {
            col: plx_ui::palette::color(plx_ui::palette::Role::Text, theme::TEXT_HEADING),
            border: plx_ui::palette::color(plx_ui::palette::Role::Border, theme::OVERLAY_BORDER),
            bg: plx_ui::palette::color(plx_ui::palette::Role::Surface, theme::SURFACE_PANEL),
        },
        measure,
    )
}

/// A VIDEO codec's display name. The sibling of [`metadata::friendly_codec`], which is the one
/// AUDIO/subtitle map and spells h264 as "H264" — the dotted form is the one everybody else's
/// player uses for the video lane, and this is the only place the app names a video codec to a
/// user. An unknown codec is upper-cased rather than dropped: "Converting · AV1" is still the
/// truth on a set that cannot decode AV1 (the dev set, and every set whose codec table lists no AV1
/// decoder), and inventing a friendly name for a codec we have never seen would not be.
pub fn video_codec_name(codec: &str) -> String {
    match codec.to_ascii_lowercase().as_str() {
        "h264" | "avc" | "avc1" => "H.264".to_string(),
        "hevc" | "h265" | "hvc1" | "hev1" => "HEVC".to_string(),
        other => other.to_uppercase(),
    }
}

/// The live playback fact for the meta line: **what the server is actually sending**, read off the
/// running stream and never predicted.
///
/// That distinction is the whole design of this line. Everything else on the row is a property of
/// the ITEM (genre, year, runtime), knowable before Play; this is a property of THIS SESSION, and
/// the only honest source for it is `route`'s own record of what was resolved — `stream_vcodec` is
/// the codec the `/decision` OUTPUT named, not the one the profile asked for. Predicting it from
/// the source file is how a client ends up telling the user it is direct playing a file the server
/// quietly re-encoded.
///
/// Three answers, because the server has three behaviours and only one of them touches the pixels:
/// a direct play (we pull the file), a container-only REMUX (the video is always copied and the
/// audio is copied when the profile admits it, converted otherwise — Plex's own "Direct Stream";
/// the mock has no case for it because its model has only "direct play" and "converts", but
/// calling a remux a conversion would state that the server re-encoded the pixels when it did
/// not), and a real re-encode, which names the codec it is producing. Hardware vs software is
/// deliberately NOT stated: that is the server's runtime choice and it reaches the client only in
/// the live transcode session, so naming it would be a guess.
///
/// **No warning belongs on this line.** The tone-mapping case in particular lives on the DETAIL
/// page, before Play: this card is behind a keypress, so a warning here is one the user finds only
/// after watching a washed-out picture. That is `Player Screen.dc.html`'s own standing comment
/// ("Warning before Play is the DETAIL page's job"), and it contradicts a bullet in the **design
/// project's** `CLAUDE.md` — the Pass-rules file in the Claude Design project, NOT this repo's root
/// `CLAUDE.md`, which says nothing about tone mapping — that lists this card beside the detail
/// facts row. The mock is the later artifact and its reasoning matches the owner's standing rule
/// that a warning exists to be seen BEFORE Play, so the code follows it; the two documents still
/// need reconciling by the owner, and that is a docs edit, not a behaviour change.
///
/// PURE, so every arm is host-testable. `streaming` is "there is a resolved stream to describe".
/// The caller composes it from `route::has_url()` **and** `!route::play_pending()`, and the second
/// half is load-bearing rather than belt-and-braces: `request_play` does not clear `URL`,
/// `TSESSION` or `CUR_REMUX` (only a real stop does, in `engine::teardown`), so on an item→item
/// switch — Up Next, or Play from a detail page while a session is live — every one of those three
/// still describes the PREVIOUS item for the whole resolve. Without the pending test this line
/// would state the last session's fact as this one's.
pub fn playback_now(
    streaming: bool,
    transcoding: bool,
    remux: bool,
    vcodec: &str,
) -> Option<String> {
    if !streaming {
        return None;
    }
    if !transcoding {
        return Some(plx_platform::i18n::msg::widgets_playback_direct_play().to_string());
    }
    if remux {
        return Some(plx_platform::i18n::msg::widgets_playback_direct_stream().to_string());
    }
    let name = video_codec_name(vcodec);
    // a re-encode whose output codec we somehow do not know still converted — say that much
    Some(if name.is_empty() {
        plx_platform::i18n::msg::widgets_playback_converting().to_string()
    } else {
        plx_platform::i18n::msg::widgets_playback_converting_codec(&name)
    })
}

/// One candidate on the metadata/badge row — see [`ChipRow`] for why `draw` measures every chip
/// before drawing any of them, rather than drawing straight through and letting the row run under
/// the action-button column (issue #26).
struct Chip {
    /// What this chip IS and, for a text run, how it is inked — a data-bearing enum rather than a
    /// `kind` tag plus a `bold`/`col` pair that only ever meant something for one of the two
    /// variants: a `Badge` chip always draws through [`meta_badge`]'s own fixed style, so a flat
    /// `bold`/`col` field on every `Chip` would carry a value for `Badge` that nothing reads and
    /// that a future caller could plausibly set expecting it to matter.
    kind: ChipKind,
    /// This chip's own measured pixel width: a run's parts, dots and the air around them.
    w: f32,
}

enum ChipKind {
    /// A text run (the meta line, or the live playback fact): its parts only, since the dots
    /// between them and the air around them belong to `widgets::DottedRun` at draw time;
    /// `lead_dot` continues the run before it, and the two runs on this row are inked differently
    /// (see the call site).
    Run { parts: Vec<String>, lead_dot: bool, bold: c_int, col: [f32; 4] },
    /// An outlined capability badge ([`meta_badge`]) saying this.
    Badge(String),
}

impl Chip {
    /// A text run of `parts`, a dot between each pair and, with `lead_dot`, one before the first (a
    /// run that continues the one before it). Spaced exactly like Detail's identity line, by the
    /// same dotted-run idiom (`space::SM` every side of the dot), and measured by it.
    fn run(
        parts: &[String],
        lead_dot: bool,
        bold: c_int,
        col: [f32; 4],
        measure: &dyn plx_machine::machine::Measure,
    ) -> Self {
        let w = Self::dotted(bold, lead_dot, col).width(measure, parts);
        let parts = parts.to_vec();
        Self { kind: ChipKind::Run { parts, lead_dot, bold, col }, w }
    }

    /// The one dotted run a text chip is measured and drawn by.
    fn dotted(bold: c_int, lead_dot: bool, col: [f32; 4]) -> DottedRun<'static> {
        let run = DottedRun::new(theme::size::CAPTION, col, theme::space::SM).bold(bold);
        if lead_dot {
            run.lead_dot()
        } else {
            run
        }
    }

    /// An outlined badge saying `label`, as wide as [`badge_w`] says.
    fn badge(label: String, measure: &dyn plx_machine::machine::Measure) -> Self {
        let w = badge_w(&label, None, measure);
        Self { kind: ChipKind::Badge(label), w }
    }
}

/// How much a chip matters to the fit. They all share one rung: the row is built in order of
/// importance (the item's own facts, the live playback fact, the rating, the premium-audio tag, then
/// CC/SDH/AD), and equal priorities drop the LAST first (`HStack::fit`), so the row sheds from its
/// tail.
const CHIP_PRIORITY: u8 = 1;

/// The most chips the row can hold: one per kind it can show — the item's meta run, the live
/// playback fact, the rating, the premium-audio tag, CC, SDH and AD.
const MAX_CHIPS: usize = 7;

/// **The chip row: every candidate chip and its [`Item`], in priority order** — the input of
/// `HStack::fit` (issue #26). Fixed storage, because the card draws ~60 times a second and a row
/// of at most [`MAX_CHIPS`] kinds needs no heap of its own. Host-testable on purpose: no `Painter`,
/// no `plx_gfx::text::text_width` (that needs a live SDL2_ttf font the host test binary never loads;
/// see `text.rs::text_width`'s own doc on why measurement is the "impure half"). `draw` measures
/// every candidate chip FIRST — meta text (genres/year/duration), then the live playback fact, then
/// the rating/audio/CC/SDH/AD badges, in that fixed priority order (see the call site for why that
/// order) — and builds the row from those widths.
///
/// The spacing is Detail's identity line's: chips stand `space::SM` apart ([`ChipRow::row`]), a badge
/// leaves `space::XS` before the next badge (so a text run → first badge is `SM`, badge → badge
/// `XS`).
///
/// The fit keeps the longest prefix that fits and drops that chip and every lower-priority one
/// behind it, never skipping ahead to try a smaller later chip instead. Skipping ahead would let a
/// narrow low-priority badge bump a wide high-priority one for no reason a viewer could predict,
/// and would make the row's content depend on exactly how much air a drop happened to leave rather
/// than on priority alone — the row is meant to read the same way every time it truncates. The
/// room is `draw`'s `tw`: the action-button column's left edge minus this panel's own text-column
/// gutter, the same right edge the title and synopsis above this row already respect. That is the
/// actual fix for #26 — the CTA disappearing under a long chip list.
struct ChipRow {
    chips: [Option<Chip>; MAX_CHIPS],
    items: [Item; MAX_CHIPS],
    n: usize,
}

impl ChipRow {
    fn new() -> Self {
        Self { chips: [const { None }; MAX_CHIPS], items: [Item::absent(); MAX_CHIPS], n: 0 }
    }

    /// Append the next-lowest-priority chip.
    fn push(&mut self, chip: Chip) {
        debug_assert!(self.n < MAX_CHIPS, "one chip per kind");
        if self.n == MAX_CHIPS {
            return;
        }
        let item = Item::new(chip.w).priority(CHIP_PRIORITY);
        self.items[self.n] = match chip.kind {
            ChipKind::Badge(_) => item.after(theme::space::XS),
            ChipKind::Run { .. } => item,
        };
        self.chips[self.n] = Some(chip);
        self.n += 1;
    }

    /// The chips as one run from the text column's left edge `tx`, `space::SM` between neighbours.
    fn row(&self, tx: f32) -> HStack<'_> {
        HStack::new(&self.items[..self.n], theme::space::SM, Align::Leading(tx))
    }
}

/// Draw the chips `row` places (its survivors), centred on `my`: each text run through the shared
/// dotted-run idiom (parts in the run's ink, dots in the separator ink), each badge through
/// [`meta_badge`].
fn draw_chip_row(
    p: Painter,
    chips: &ChipRow,
    row: HStack<'_>,
    my: f32,
    measure: &dyn plx_machine::machine::Measure,
) {
    for placed in row.placed() {
        let Some(chip) = chips.chips[placed.i].as_ref() else { continue };
        match &chip.kind {
            ChipKind::Run { parts, lead_dot, bold, col } => {
                let y = plx_ui::typography::text_vcenter_y(theme::size::CAPTION, *bold, my);
                Chip::dotted(*bold, *lead_dot, *col).measured(measure).draw(p, parts, placed.x, y);
            }
            ChipKind::Badge(label) => {
                meta_badge(p, placed.x, my, label, measure);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{draw_chip_row, playback_now, Chip, ChipKind, ChipRow};
    use plx_ui::fixture::FixtureMeasure;
    use plx_ui::theme::space;

    /// A text chip / badge chip of a given width, to grade the row's spacing and cut without a font.
    fn text(w: f32) -> Chip {
        Chip { kind: ChipKind::Run { parts: vec![], lead_dot: false, bold: 0, col: plx_ui::theme::TEXT_PRIMARY }, w }
    }
    fn badge(w: f32) -> Chip {
        Chip { kind: ChipKind::Badge(String::new()), w }
    }

    /// The row of `chips`, in priority order.
    fn row_of(chips: impl IntoIterator<Item = Chip>) -> ChipRow {
        let mut row = ChipRow::new();
        for chip in chips {
            row.push(chip);
        }
        row
    }

    /// How many of `chips` the row keeps in `room` px (what `draw` computes as `n`).
    fn kept_in(chips: impl IntoIterator<Item = Chip>, room: f32) -> usize {
        let chips = row_of(chips);
        let row = chips.row(0.0);
        row.kept(row.fit(room)).placed().count()
    }

    /// The row's spacing rule, checked directly rather than only through the cut: a text run → first
    /// badge is `space::SM`, a badge → badge is `space::XS` (the Detail identity line's own gaps —
    /// no raw 18 / 12), and a run → run is `SM` too (the fact run carries its own leading dot).
    #[test]
    fn the_rows_gaps_are_the_identity_lines_tokens() {
        let chips = row_of([text(100.0), text(50.0), badge(60.0), badge(70.0), badge(80.0)]);
        let spans: Vec<_> = chips.row(462.0).placed().collect();
        let gap = |i: usize| spans[i + 1].x - (spans[i].x + spans[i].w);
        assert_eq!(spans[0].x, 462.0);
        assert_eq!([gap(0), gap(1), gap(2), gap(3)], [space::SM, space::SM, space::XS, space::XS]);
    }

    /// **Issue #26, reproduced.** The exact geometry `draw` computes for a real card: `bx` (the
    /// action column's left edge) at 1460, this panel's own 34px gutter in front of it, and the
    /// text column starting (`tx`) at 462 — so `avail_w` below is the literal `tw` a real frame
    /// would pass to the fit. The chip list is what the issue names: an item with a
    /// meta line, a live playback fact, a rating, a premium audio tag and all three of CC/SDH/AD —
    /// "enough chips/actions... that they overlap the main CTA." Before this fix `draw` had no
    /// bound at all here, so every one of these drew and the row reached roughly 1060px into a
    /// 964px lane — squarely under the action buttons. The two assertions are the guarantee that
    /// replaces that: not everything fits, and whatever DOES fit never crosses into the CTA.
    #[test]
    fn chip_row_never_overlaps_the_action_column_with_a_long_chip_list() {
        let cta_x = 1460.0f32; // `bx` in `draw`
        let gutter = 34.0f32; // `bx - tright` in `draw`
        let text_x = 462.0f32; // `tx` in `draw`
        let avail_w = (cta_x - gutter) - text_x; // == `tw`, 964.0
        assert_eq!(avail_w, 964.0);

        // in this row's fixed priority order — meta, fact, rating, audio, CC, SDH, AD — mirroring
        // real measured widths at `size::CAPTION`.
        let chips = row_of([
            text(480.0),  // "Action · Adventure · 2019 · 1h 32m"
            text(300.0),  // "· Converting · HEVC"
            badge(90.0),  // rating badge
            badge(140.0), // audio badge ("Dolby Atmos")
            badge(60.0),  // CC
            badge(74.0),  // SDH
            badge(60.0),  // AD
        ]);
        let all = chips.row(text_x);
        assert!(all.width() > avail_w, "this list must not all fit in {avail_w}px — that is the bug being reproduced");
        let row = all.kept(all.fit(avail_w));
        assert!(row.placed().count() < 7);
        let end = row.placed().last().map_or(text_x, |s| s.x + s.w);
        assert!(end - text_x <= avail_w, "drawn chips must never reach the action column");
    }

    /// The packer never skips ahead to a smaller lower-priority chip once a higher-priority one
    /// does not fit — dropping stops the row at that chip, full stop. Chip 1 alone leaves no room
    /// for chip 2, even though chip 2 would fit on its own in the space chip 1 wanted.
    #[test]
    fn the_fit_drops_the_tail_rather_than_skipping_ahead() {
        let chips = [text(50.0), text(60.0), text(10.0)];
        // room 100: chip 0 (50) fits, chip 1 needs SM + 60 more (126 total, too much) — stop.
        // chip 2 alone would fit in the remaining 50px, but it is never reached.
        assert_eq!(kept_in(chips, 100.0), 1);
    }

    /// A single chip wider than the whole lane draws nothing rather than spilling past the room —
    /// there is no partial chip.
    #[test]
    fn a_single_oversized_chip_is_dropped_whole() {
        assert_eq!(kept_in([text(2000.0)], 964.0), 0);
    }

    /// Exact-fit boundary: a chip that lands EXACTLY on the room is kept, not dropped.
    #[test]
    fn an_exact_fit_is_kept() {
        assert_eq!(kept_in([text(100.0), text(50.0)], 150.0 + space::SM), 2);
        assert_eq!(kept_in([text(100.0), text(50.0)], 150.0 + space::SM - 1.0), 1);
    }

    /// No candidates, or no room at all — both are 0, not a panic.
    #[test]
    fn empty_or_zero_width_is_zero_chips() {
        assert_eq!(kept_in([], 964.0), 0);
        assert_eq!(kept_in([text(10.0)], 0.0), 0);
        assert_eq!(kept_in([text(10.0)], -5.0), 0);
    }

    /// **Q3: the info card's metadata line is spaced exactly like Detail's identity line** — read off
    /// the PAINTED runs. Each part and each dot stands one `space::SM` from its neighbour (the
    /// measured advance of the one before it), the dot is regular weight in the separator ink, and
    /// the fact run continues the meta run through one more dot with the same pad; then the first
    /// badge is `SM` after the text and the next badge `XS` after that. The runs stand at the
    /// caller's `x`, so a baked "   ·   " (or a raw 18 / 12) cannot hide.
    #[test]
    fn the_meta_line_is_spaced_like_the_identity_line() {
        use plx_machine::machine::Measure as _;
        let m = FixtureMeasure;
        let sz = plx_ui::theme::size::CAPTION;
        let w = |s: &str, bold: i32| m.width_str(s, sz, bold != 0);
        let ink = plx_ui::theme::TEXT_PRIMARY;
        let meta = Chip::run(&["Action".into(), "2019".into(), "1h 32m".into()], false, 1, ink, &m);
        let fact = Chip::run(&["Direct Play".into()], true, 0, plx_ui::theme::TEXT_TERTIARY, &m);
        let chips = row_of([meta, fact, Chip::badge("PG-13".into(), &m), Chip::badge("CC".into(), &m)]);
        let row = chips.row(462.0);
        let draw = || draw_chip_row(plx_ui::Painter::recording(), &chips, row, 600.0, &m);
        let runs = plx_ui::draw_census::capture_runs(draw);
        let rrects: Vec<_> = plx_ui::draw_census::capture(draw).into_iter().filter(|(tag, _)| *tag == 2).map(|(_, r)| r).collect();
        let texts: Vec<_> = runs.iter().map(|r| (r.text.clone(), r.at.x, r.bold, r.ink)).collect();
        let words: Vec<&str> = texts.iter().map(|(t, ..)| t.as_str()).collect();
        const DOT: &str = "\u{b7}";
        assert_eq!(words, ["Action", DOT, "2019", DOT, "1h 32m", DOT, "Direct Play", "PG-13", "CC"]);
        // each text run begins one SM after the advance of the one before it, in the row's one pad
        for pair in texts[..7].windows(2) {
            let advance = w(&pair[0].0, i32::from(pair[0].2));
            assert!((pair[1].1 - (pair[0].1 + advance) - space::SM).abs() < 0.5, "{pair:?}");
        }
        assert_eq!(texts[0].1, 462.0);
        // the dots are regular weight in the separator ink; the meta parts are bold
        for (text, _, bold, ink) in &texts[..7] {
            if text == DOT {
                assert!(!bold && *ink == plx_ui::theme::TEXT_SEPARATOR, "the dot idiom");
            }
        }
        assert!(texts[0].2 && !texts[6].2, "the item's facts are bold, the playback fact regular");
        // badges: the keyline rect of the first one stands SM after the last text run's advance, the
        // second XS after the first badge
        let keylines: Vec<_> = rrects.chunks(2).map(|c| c[0]).collect();
        assert_eq!(keylines.len(), 2);
        let last = &texts[6];
        assert!((keylines[0].x - (last.1 + w(&last.0, 0)) - space::SM).abs() < 0.5);
        assert!((keylines[1].x - (keylines[0].x + keylines[0].w) - space::XS).abs() < 0.5);
    }

    /// One hidden-item case: a lone part draws no dot at all, and a fact run with no meta run in front
    /// of it leads with none — no stray separator at the head of the row.
    #[test]
    fn a_lone_part_or_a_fact_without_a_meta_run_draws_no_dot() {
        use plx_machine::machine::Measure as _;
        let m = FixtureMeasure;
        let sz = plx_ui::theme::size::CAPTION;
        let ink = plx_ui::theme::TEXT_PRIMARY;
        let lone = Chip::run(&["2019".into()], false, 1, ink, &m);
        assert_eq!(lone.w, m.width_str("2019", sz, true), "one part: its width, no dot, no gap");
        let chips = row_of([lone, Chip::run(&["Direct Play".into()], false, 0, ink, &m)]);
        let runs = plx_ui::draw_census::capture_runs(|| {
            draw_chip_row(plx_ui::Painter::recording(), &chips, chips.row(0.0), 0.0, &m);
        });
        assert_eq!(runs.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(), ["2019", "Direct Play"]);
        let first = m.width_str("2019", sz, true);
        assert!((runs[1].at.x - (runs[0].at.x + first) - space::SM).abs() < 0.5, "two runs stand one SM apart");
    }

    /// One slot-free fit case through the drawn row: dropping the tail removes its runs from the
    /// paint, not just from the count.
    #[test]
    fn a_dropped_chip_is_not_painted() {
        let m = FixtureMeasure;
        let chips = row_of([text(100.0), Chip::badge("CC".into(), &m)]);
        let all = chips.row(0.0);
        let row = all.kept(all.fit(100.0));
        let log = plx_ui::draw_census::capture(|| draw_chip_row(plx_ui::Painter::recording(), &chips, row, 0.0, &m));
        assert!(log.iter().all(|(tag, _)| *tag != 2), "the badge was dropped, so no rrect is drawn");
    }

    /// The meta line's live fact, arm by arm. It is the one thing on that row the app could get
    /// wrong by GUESSING, so each arm pins a different way of guessing:
    ///   * a direct play says so and names no codec — there is no conversion to describe;
    ///   * a container remux is NOT a conversion (the video is copied, and the audio too when the
    ///     profile admits it), and calling it one would state that the server touched the pixels
    ///     when it did not — `route::is_remux`'s own doc
    ///     is that these are different facts;
    ///   * a re-encode names the codec the server is actually PRODUCING (`route::stream_vcodec` is
    ///     the `/decision` OUTPUT), spelled the way a viewer reads it, and HEVC output is reachable
    ///     only on a Pass server — which this function neither knows nor asks, because it reports
    ///     what happened rather than what was allowed;
    ///   * and with no stream resolved there is no fact at all: during the resolve window nothing
    ///     has been sent yet, and stating a fact about it would be exactly the prediction the whole
    ///     line refuses to make.
    #[test]
    fn the_meta_lines_playback_fact_describes_the_stream_that_is_running() {
        assert_eq!(
            playback_now(true, false, false, "hevc").as_deref(),
            Some("Direct Play")
        );
        // remux: `vcodec` is the SOURCE codec copied through, and it is still not a conversion
        assert_eq!(
            playback_now(true, true, true, "hevc").as_deref(),
            Some("Direct Stream")
        );
        assert_eq!(
            playback_now(true, true, false, "h264").as_deref(),
            Some("Converting \u{b7} H.264")
        );
        assert_eq!(
            playback_now(true, true, false, "hevc").as_deref(),
            Some("Converting \u{b7} HEVC")
        );
        // a codec map we have never met is upper-cased, not dropped and not renamed
        assert_eq!(
            playback_now(true, true, false, "av1").as_deref(),
            Some("Converting \u{b7} AV1")
        );
        // a re-encode whose output codec never landed still converted
        assert_eq!(
            playback_now(true, true, false, "").as_deref(),
            Some("Converting")
        );
        // nothing resolved yet → no fact, on every combination of the flags
        for (t, r) in [(false, false), (true, false), (true, true)] {
            assert!(
                playback_now(false, t, r, "h264").is_none(),
                "no stream, no fact"
            );
        }
    }
}

#[cfg(test)]
mod focus_tests {
    use super::*;
    use plx_machine::machine::{FocusRead, InputOwner, PressRead, Tick};

    // TEST ONLY: a thread-confined store, so `set_current_for_test`/`apply` and the `view()`
    // this test's `on_ok`/`is_episode` calls read from are the SAME owner, not two disconnected
    // `MetadataStore::default()`s (same pattern as `screens::detail::tests`'s `TEST_METADATA`).
    thread_local! {
        static TEST_METADATA: std::cell::UnsafeCell<plx_data::stores::metadata::MetadataStore> =
            std::cell::UnsafeCell::new(plx_data::stores::metadata::MetadataStore::default());
    }

    fn test_store() -> &'static mut plx_data::stores::metadata::MetadataStore {
        TEST_METADATA.with(|cell| unsafe { &mut *cell.get() })
    }

    struct HostFixture;
    impl Host for HostFixture {
        type Arg = plx_ui::fixture::FixtureArg;
        type Fx = plx_ui::fixture::FixtureFx;
        type Msg = plx_ui::fixture::FixtureMsg;
        type Elem = u32;
        type Views<'a> = ();
        type Init = plx_ui::fixture::FixtureInit;
        type Memory = ();
    }

    fn with_cx<R>(entry: EntryId, test: impl FnOnce(&Cx<'_, HostFixture>) -> R) -> R {
        let measure = plx_ui::fixture::FixtureMeasure;
        test(&Cx {
            views: (),
            tick: Tick::default(),
            measure: &measure,
            focus: FocusRead::default(),
            press: PressRead::default(),
            owner: InputOwner::Entry(entry),
        })
    }

    /// **UP/DOWN step by one button and clamp at both ends** over the fixed two-button column.
    #[test]
    fn up_down_step_by_one_and_clamp_at_both_ends() {
        let e = EntryId(6);
        let st = InfoPanelState::new();
        let part = InfoPanelPart { state: &st, entry: e, group: GroupId(0), can_go_to_detail: true, mode: UiMode::Modern };
        with_cx(e, |cx| {
            let step = |i: u32, dir: Dir| {
                match <InfoPanelPart as Focusable<HostFixture>>::neighbour(
                    &part,
                    FocusKey { entry: e, elem: i },
                    dir,
                    cx,
                ) {
                    Step::Move(k) => Some(k.elem),
                    Step::Edge => None,
                }
            };
            assert_eq!(step(0, Dir::Down), Some(1));
            assert_eq!(step(1, Dir::Down), None, "the last button is the screen's own edge");
            assert_eq!(step(0, Dir::Up), None, "the first button does not wrap");
        });
    }

    /// `place` reports exactly the rect [`InfoPanelState::draw`] paints the button at.
    #[test]
    fn place_matches_the_draw_formula() {
        let e = EntryId(6);
        let st = InfoPanelState::new();
        let part = InfoPanelPart { state: &st, entry: e, group: GroupId(0), can_go_to_detail: true, mode: UiMode::Modern };
        with_cx(e, |cx| {
            for i in 0..N_ACTIONS as u32 {
                let placed = <InfoPanelPart as Focusable<HostFixture>>::place(&part, &i, cx, At::Drawn)
                    .expect("both buttons are placeable");
                let want = button_rect(i as usize, cx.measure);
                assert_eq!(
                    (placed.rect.x, placed.rect.y, placed.rect.w, placed.rect.h),
                    (want.x, want.y, want.w, want.h)
                );
            }
        });
    }

    /// Two buttons stack vertically, sharing an x and a width, in the order `draw` paints them —
    /// "From Beginning" above "Go to Show"/"Go to Movie".
    #[test]
    fn the_two_buttons_share_a_column_and_stack_in_draw_order() {
        let measure = plx_ui::fixture::FixtureMeasure;
        let (a, b) = (button_rect(0, &measure), button_rect(1, &measure));
        assert_eq!(a.x, b.x);
        assert_eq!(a.w, b.w);
        assert!(a.y < b.y, "From Beginning sits above the second action");
    }

    /// DOWN off the last button is the SCREEN's edge (`FocusTabs`), never `Stop` — the group's
    /// declared edges pin the direction each rule below answers.
    #[test]
    fn down_off_the_last_button_is_the_screens_edge() {
        let e = EntryId(6);
        let st = InfoPanelState::new();
        let part = InfoPanelPart { state: &st, entry: e, group: GroupId(0), can_go_to_detail: true, mode: UiMode::Modern };
        with_cx(e, |cx| {
            let mut groups = Vec::new();
            <InfoPanelPart as Focusable<HostFixture>>::groups(&part, cx, &mut groups);
            let g = groups.into_iter().next().expect("one group");
            assert!(matches!(g.edge[1], EdgeRule::Screen), "down");
            assert!(matches!(g.edge[0], EdgeRule::Stop), "up");
        });
    }

    #[test]
    fn unavailable_detail_has_no_focus_stop_and_reconciles_stale_selection() {
        let e = EntryId(6);
        let mut st = InfoPanelState::new();
        st.set_focus(1);
        let part = InfoPanelPart { state: &st, entry: e, group: GroupId(0), can_go_to_detail: false, mode: UiMode::Modern };
        with_cx(e, |cx| {
            let mut groups = Vec::new();
            <InfoPanelPart as Focusable<HostFixture>>::groups(&part, cx, &mut groups);
            assert_eq!(groups[0].len, 1);
            assert!(<InfoPanelPart as Focusable<HostFixture>>::place(&part, &1, cx, At::Drawn).is_none());
            assert!(<InfoPanelPart as Focusable<HostFixture>>::group_of(&part, &1, cx).is_none());
            assert!(matches!(<InfoPanelPart as Focusable<HostFixture>>::neighbour(&part,
                FocusKey { entry: e, elem: 0 }, Dir::Down, cx), Step::Edge));
            let key = <InfoPanelPart as Focusable<HostFixture>>::reconcile(&part,
                FocusKey { entry: e, elem: 1 }, cx);
            assert_eq!(key.elem, 0);
            let placed = <InfoPanelPart as Focusable<HostFixture>>::place(&part, &0, cx, At::Drawn).unwrap();
            assert_eq!(<InfoPanelPart as Focusable<HostFixture>>::seat(&part, GroupId(0), placed, cx).elem, 0);
        });
        st.set_focus(0);
        assert!(st.at_last(false));
        assert!(!st.at_last(true));
    }

    #[test]
    fn ott_playing_metadata_never_falls_back_to_an_unrelated_plex_detail() {
        let _g = plx_base::testlock::serial();
        plx_data::metadata::set_current_for_test(test_store().state_mut(), Some(plx_data::metadata::Detail {
            rk: "unrelated".into(), kind: "movie".into(), summary: "Wrong description".into(),
            thumb: "/wrong-art".into(), ..Default::default()
        }));
        test_store().run(plx_data::stores::metadata::MetadataCmd::SetNowPlaying(Some(metadata::NowPlaying {
            ott: Some(metadata::OttNowPlaying { live: None, archive: None, collection_label: "Folder".into(),
                confirmed_start_unix_ms: None, next: None,
                art: Some(metadata::MediaArt::Http { url: "https://example.test/poster.jpg".into() }) }),
            is_episode: false, is_real_episode: false, title: "Current video".into(), ep_title: String::new(),
            season: 0, index: 0, summary: String::new(), year: 0, dur_ms: 0, rating: String::new(),
            thumb: String::new(), detail_rk: String::new(),
        })));
        assert!(fallback_detail(test_store().view()).is_none());
        assert!(detail_rk(test_store().view()).is_none());
        let mut panel = InfoPanelState::new(); panel.set_focus(1);
        assert_eq!(panel.on_ok(test_store().view()), InfoAction::None);
        test_store().run(plx_data::stores::metadata::MetadataCmd::SetNowPlaying(None));
    }

    #[test]
    fn go_to_after_a_trailer_opens_the_loaded_parent() {
        let _g = plx_base::testlock::serial();
        test_store().run(plx_data::stores::metadata::MetadataCmd::SetNowPlaying(None));
        plx_data::metadata::set_current_for_test(test_store().state_mut(), Some(plx_data::metadata::Detail {
            rk: "parent-movie".into(),
            kind: "movie".into(),
            ..Default::default()
        }));
        let mut movie = InfoPanelState::new();
        movie.set_focus(1);
        assert_eq!(
            movie.on_ok(test_store().view()),
            InfoAction::GoToDetail("parent-movie".into())
        );
        assert!(!is_episode(test_store().view()), "a movie parent labels Go to Movie");

        plx_data::metadata::set_current_for_test(test_store().state_mut(), Some(plx_data::metadata::Detail {
            rk: "parent-show".into(),
            kind: "show".into(),
            is_show: true,
            ..Default::default()
        }));
        let mut show = InfoPanelState::new();
        show.set_focus(1);
        assert_eq!(show.on_ok(test_store().view()), InfoAction::GoToDetail("parent-show".into()));
        assert!(is_episode(test_store().view()), "a show parent labels Go to Show");

        plx_data::metadata::set_current_for_test(test_store().state_mut(), Some(plx_data::metadata::Detail {
            sid: plx_plex::plex::ServerId::UNSET,
            rk: "parent-show".into(),
            kind: "show".into(),
            is_show: true,
            title: "Show".into(),
            extras: vec![plx_data::metadata::Extra {
                rk: "9".into(),
                title: "Official Trailer".into(),
                dur_ms: 120_000,
                part: "/p".into(),
                ..Default::default()
            }],
            ..Default::default()
        }));
        let trailer = plx_data::metadata::trailer_now_playing(test_store().state(), plx_plex::plex::ServerId::UNSET, "9");
        test_store().run(plx_data::stores::metadata::MetadataCmd::SetNowPlaying(trailer));
        let mut playing = InfoPanelState::new();
        playing.set_focus(1);
        assert_eq!(playing.on_ok(test_store().view()), InfoAction::GoToDetail("parent-show".into()));
        assert!(is_episode(test_store().view()), "an installed show-trailer card still says Go to Show");
        assert_eq!(
            test_store().view().now_playing().map(|n| n.dur_ms),
            Some(120_000),
            "the extra's duration, not the show's"
        );

        test_store().run(plx_data::stores::metadata::MetadataCmd::SetNowPlaying(None));
        plx_data::metadata::set_current_for_test(test_store().state_mut(), None);
    }
}

#[cfg(test)]
mod vstack_tests {
    use super::*;

    fn ys(syn_h: f32, tags: Option<f32>) -> Vec<(usize, f32)> {
        let items = card_items(42.0, syn_h, 12.0, tags);
        VStack::new(&items, 6.0, Anchor::Top(0.0)).placed().map(|s| (s.i, s.y)).collect()
    }

    /// Title, synopsis, tags: 6 then 12 apart; with no synopsis the tags stay 12 under the title, not 6.
    #[test]
    fn the_gaps_survive_an_absent_synopsis() {
        assert_eq!(ys(58.0, Some(34.0)), vec![(0, 0.0), (1, 48.0), (2, 118.0)]);
        assert_eq!(ys(0.0, Some(34.0)), vec![(0, 0.0), (2, 54.0)]);
        assert_eq!(ys(58.0, None), vec![(0, 0.0), (1, 48.0)]);
    }
}

#[cfg(test)]
#[path = "info_panel_classic_tests.rs"]
mod classic_tests;
