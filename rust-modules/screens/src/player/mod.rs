//! **The player as an owned `Screen`** (restructure spec §9, phase 9).
//!
//! `PlayerScreen` is the instance the container mounts for the player route. It owns every piece
//! of state the route used to keep in two places at once — the HUD's timer, cursor and dismissal,
//! the scrub gesture, the held-key timer, the repeat gate, the control row's springs and resume
//! clock, the Up Next countdown and the image-subtitle texture set — and it draws the whole route:
//! subtitles, the transport, the read-out, and (through the page-owned `ModalStack`) its overlays.
//!
//! It answers `RenderStrategy::VideoPlane`, which is what tells the frame that what is behind this
//! page is a hardware plane GL cannot read: no `popover::host` snapshot, no `Glass` source pass, no
//! `RouteGround`, no capture. The CONSEQUENCES of that strategy — the present gate, the opaque
//! region and the capture skip — are the loop's three privileged calls (§3.3 steps 1, 9 and 10) and
//! are not made here.
//!
//! **What is here and what is not.** Phase 9 moves the STATE and the DRAW; the key ladder in
//! `app/run.rs` remains a CALLER of this screen for the bare transport, exactly as `ui::press`'s
//! typed facade left the ladders callers of `App.input.press` in phase 2 (§14, "Press during
//! coexistence"). The overlays are different: they are entries on this page's own
//! `ModalStack` and own their input outright, so their arms left the ladder with them.
//!
//! Two fields are PUBLISHED rather than owned by `player::TX` (§2.3): `hud.until` and `scrub.ns`.
//! [`PlayerScreen::publish`] mirrors both into the atomics the draw path and the engine read, and
//! is the only writer of either.

mod ass_subtitles;
mod seek_jump;
pub mod input;
pub mod overlay;
#[cfg(test)]
mod overlay_tests;
#[cfg(test)]
mod ott_remote_tests;
#[cfg(test)]
mod osd_tests;
#[cfg(test)]
mod channel_transition_tests;
#[cfg(test)]
mod multiview_tests;

use std::borrow::Cow;

use crate::registry::{AppFx, AppLike, PlayerLike, PlayerReq};
use plx_ui::consts;
use plx_ui::frame::Budget;
use plx_machine::machine::{
    By, Canon, Cx, Edge, Effects, EntryId, Fx, FocusKey, GroupId, Handled, InputKind, InstanceId,
    LogicalState, Machine,
};
use plx_appkit::player_hud::geometry::{HudGeometry, GROUP_FAILURE, GROUP_ROW, GROUP_SCRUB, GROUP_TABS};
use plx_appkit::player_hud::model::HudModel;
use plx_appkit::player_hud::{self, ControlSlot, SubtitleBitmaps, motion::HudMotion};
use plx_ui::screen::{
    Activate, At, AxisMask, Dir, DrawFrame, EdgeRule, FocusSource, Focusable, GroupKind, GroupSpec,
    HitSource, Hover, Link, Placed, RenderStrategy, Screen, ScreenEvent, Seat, Step, Stop,
};
use plx_appkit::up_next::Countdown;
use plx_ui::Rect;

use input::{HudState, Scrub};

/// The player's heartbeat word — byte-identical to `app::words::route_word(&AppArg::Player)`, and
/// what `tests/run.py` selects fps samples by (`tests/manifest.json`'s `route` field).
///
/// **This constant IS the heartbeat's `route=` now.** `bridge::frame` used to print
/// `route_word(app.route)` and `debug_assert_eq!` it against the top screen's `Screen::name`; phase
/// 12 (D1) deleted the route mirror, so there is one source and the assert has nothing left to
/// compare. Keeping the two tables equal is `app::words::heartbeat_word_tests`' job, and it derives
/// both rather than transcribing either — a word the app cannot print reads on the television as
/// "0 post-warmup samples", which is indistinguishable from a real regression.
pub const WORD: &str = "player";

const VOLUME_NOTICE_MS: u32 = 2_000;

/// The fields [`PlayerScreen::write`] canonicalises, for the recorder's shape pin (§5.4).
pub const SHAPE: &str =
    "PlayerScreen{hud:{focus:i32,btn:i32,tab:i32,until:u32,info_timeout_s:u32,dismissed:bool,visible_at_press:bool,\
     exit_armed:bool,back_leaves_at_press:bool,offer:Option<(u32,i64)>,was_standin:bool},scrub:{dir:i32,hold:bool,reveal:bool,drag:bool,\
     ns:i64},seek_burst:Option<(i64,u32)>,transport_tap:bool,origin:Option<u32>,repair_alert:{open:bool,confirm:bool,scroll:u32},reminder:{sheet:bool,focus:bool,token:Option<str>,until:Option<u32>,notice:bool},volume:{until:Option<u32>,notice:bool,number:bool}}";

/// **The page this playback was launched from**, as the container's own identity.
///
/// A pair rather than the `EntryId` alone: an entry can be EVICTED at `CAP` and remounted, so its
/// `InstanceId` is what says "the same body", while the `EntryId` is what survives that eviction
/// and is what a `PopTo` names (§6.1's three tiers). Recorded once at `Mount`, from whatever page
/// was on top when the player was pushed, and never rewritten — an overlay opening on this page's
/// own `ModalStack` changes the input owner and must not be mistaken for a new origin, which is
/// the whole reason this is captured at `Mount` and not read live.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Origin {
    pub entry: EntryId,
    pub instance: Option<InstanceId>,
}

/// The route's RENDER resources — recreated freely, never read as logical state (§6.1).
#[derive(Default)]
pub struct PlayerRender {
    /// The decoded image-subtitle display set and its cache key (`player_hud`'s `SET`/`KEY`/`SEL`).
    pub subs: SubtitleBitmaps,
    pub ass: ass_subtitles::AssSubtitles,
    pub osd_clock: player_hud::osd::Clock,
}

/// **What the surfaces over the player page do to the transport this frame** — one value
/// rather than overlapping per-frame flags, so an impossible pair (lifted AND hidden) cannot be
/// stated. Resolved by `app::bridge::player_hud_policy` from every surface that is not `Hidden`
/// (a closing panel is still on screen), and pushed once per frame through
/// [`PlayerScreen::set_hud_policy`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HudPolicy {
    /// No surface: the transport follows its own timer and dismissal.
    #[default]
    Normal,
    /// A surface is being read over the page (a player panel, or anything else presented over
    /// it): the transport stays drawn and subtitles lift clear of it, since that is exactly when
    /// the user is reading the bottom of the screen.
    Lifted,
    /// A surface sits where the transport would (the Timing capsule,
    /// `OverlayKind::hud_policy`): the transport, the caption lift and the pointer/D-pad stops
    /// under it are all withheld. Outranks `Lifted` when both kinds of surface are up.
    Hidden,
}

pub struct PlayerScreen {
    entry: EntryId,
    /// The transport HUD's timer, cursor, dismissal and control-row edges.
    pub hud: HudState,
    /// The scrub-seek gesture, including its preview position.
    pub scrub: Scrub,
    /// Requested relative position across a short burst of immediate tap-seeks.
    seek_burst: player_hud::seek_jump::SeekBurst,
    /// A dedicated FF/REW press that has not yet repeated or been released.
    transport_tap: bool,
    // (`held: HeldKey` stood here — "the client-side hold-repeat for the bare transport's
    // directions". Nothing armed it after phase 9 put the panels' input on the dispatcher, so it
    // wrote two constant zeros into this screen's canonical state; the type is deleted with the
    // loop's timer in phase 10 and this field with it.
    //
    // `repeat: RepeatGate` is NOT here either, and spec §9's target shape lists it: the paced
    // 110 ms admission is the OVERLAY's — `PlayerOverlayScreen` is what has a list to walk under
    // a held key. The bare transport's own directions run the continuous scrub instead, which is
    // a ramp rather than a discrete move and was never gated.)
    /// The control row's focus springs, its label-width memo and the resume clock
    /// (`player_hud`'s `ROW_POP`/`MEMO`/`PLAY_AT`/`PAUSE_SEEN`).
    pub row: HudMotion,
    /// The Up Next auto-advance countdown (`up_next`'s `DEADLINE`/`CANCELLED`).
    pub up_next: Countdown,
    /// This frame's control-row occupant, sampled ONCE by the loop and pushed here (`slot()`'s own
    /// doc: `playpos_ns` is written by LG's media thread and `player::pump` runs between the input
    /// handlers and the draw, so re-deriving it per call site let a keypress dispatch to a control
    /// that the same frame then declined to draw).
    pub slot: ControlSlot,
    /// Who owns the "pipeline is working" signal this frame — resolved once, after `player::pump`,
    /// and handed to both the transport and the read-out so they can never both light.
    pub busy: plx_appkit::player_hud::Busy,
    /// A viewer-requested channel change keeps its information visible through acquisition.
    channel_change_hud: bool,
    /// Is the transport's MIDDLE drawn at all this frame? False while an Info card or Chapters
    /// strip has taken it — which panel is up is the container's answer, so the loop pushes it.
    pub transport: bool,
    /// What the surfaces over this page do to the transport this frame — see [`HudPolicy`].
    /// Pushed once per frame by `app/run.rs` through [`Self::set_hud_policy`].
    hud_policy: HudPolicy,
    // Sampled by the app before the frame fingerprint; never inferred from a stale draw.
    osd_uncovered: bool,
    /// Where this playback returns to — see [`Origin`].
    pub origin: Option<Origin>,
    render: PlayerRender,
    /// The page's one decision card: the sandbox repair's confirmation.
    repair_alert: plx_ui::decision_alert::DecisionAlert,
    repair_scroll: u32,
    reminder_sheet: bool,
    reminder_prompt: Option<plx_data::ott::reminders::PromptToken>,
    reminder_focus: bool,
    reminder_notice: plx_ui::decision_alert::DecisionAlert,
    reminder_notice_until: Option<u32>,
    /// Passive application-volume feedback; never owns input or extends the transport HUD.
    volume_notice: plx_ui::decision_alert::DecisionAlert,
    volume_notice_until: Option<u32>,
    number_notice: bool,
    /// Notice lifetime is independent of the transport auto-hide timer.
    failure_up: bool,
    failure_until: Option<u32>,
    failure_dismissed: bool,
    failure_attempt: Option<(Option<plx_media::route::PlaybackIdentity>, plx_media::player::FailureKind)>,
    /// The transport clocks' prewarm memo — a render resource, not logical state (`prepare`).
    clock_warm: plx_appkit::player_hud::motion::ClockWarm,
    /// **This frame's snapshot of everything the HUD's presentation reads from the player's
    /// globals** (`TX.paused`, the scrub target, the playhead, the duration, the subtitle picks).
    /// The app assigns it ONCE a frame, before `prepare` and `draw`; only those two read it.
    /// Key handling, the scrub arithmetic and the Tick keep sampling what they always did.
    pub model: HudModel,
    /// Paint-only pending channel facts; never consulted by input or transport.
    pub pending_channel_display: Option<plx_data::metadata::NowPlaying>,
    pub pending_channel_display_stamp: Option<(u64, u64, u64, bool)>,
}

impl PlayerScreen {
    pub fn new(entry: EntryId) -> Self {
        Self {
            entry,
            hud: HudState::IDLE,
            scrub: Scrub::IDLE,
            seek_burst: player_hud::seek_jump::SeekBurst::IDLE,
            transport_tap: false,
            row: HudMotion::new(),
            up_next: Countdown::default(),
            slot: ControlSlot::Discs(player_hud::Controls::NONE),
            busy: plx_appkit::player_hud::Busy::None,
            channel_change_hud: false,
            transport: true,
            hud_policy: HudPolicy::Normal,
            osd_uncovered: false,
            origin: None,
            render: PlayerRender::default(),
            repair_alert: {
                let mut alert = plx_ui::decision_alert::DecisionAlert::new();
                alert.set_tone(plx_ui::decision_alert::Tone::Neutral);
                alert
            },
            repair_scroll: 0,
            reminder_sheet: false,
            reminder_prompt: None,
            reminder_focus: false,
            reminder_notice: plx_ui::decision_alert::DecisionAlert::new(),
            reminder_notice_until: None,
            volume_notice: plx_ui::decision_alert::DecisionAlert::new(),
            volume_notice_until: None,
            number_notice: false,
            failure_up: false,
            failure_until: None,
            failure_dismissed: false,
            failure_attempt: None,
            clock_warm: Default::default(),
            model: HudModel::default(),
            pending_channel_display: None,
            pending_channel_display_stamp: None,
        }
    }

    /// Apply the current Runtime's bounded effect without a second event queue.
    /// Lead notices never register input stops or extend the transport footer.
    pub fn reminder_effect(&mut self, effect: plx_data::ott::reminders::Fx, now: u32) -> bool {
        use plx_data::ott::reminders::Fx as ReminderFx;
        use plx_ui::decision_alert::Answers;
        use plx_platform::i18n::msg;
        match effect {
            ReminderFx::LeadNotice { record, minutes } => {
                if self.repair_alert.visible() { return false; }
                self.clear_volume_notice();
                self.reminder_notice.open_notice(msg::ott_reminder_title_c(), vec![
                    record.title.into(), msg::ott_reminder_soon(i64::from(minutes)).into(),
                ]);
                self.reminder_notice_until = Some(now.wrapping_add(input::HUD_LINGER_MS));
            }
            ReminderFx::PromptSwitch { token, record } => {
                if self.repair_alert.visible() && !self.reminder_sheet { return false; }
                self.clear_volume_notice();
                self.reminder_notice.close();
                self.reminder_notice_until = None;
                self.repair_scroll = 0;
                self.reminder_sheet = true;
                self.reminder_prompt = Some(token);
                self.reminder_focus = true;
                self.repair_alert.open_card(msg::ott_reminder_title_c(), vec![
                    record.title.into(), msg::ott_reminder_starting().into(),
                ], Answers::Two);
            }
            ReminderFx::ClosePrompt(token) => {
                if self.reminder_prompt == Some(token) {
                    self.reminder_prompt = None;
                    self.reminder_sheet = false;
                    self.reminder_focus = false;
                    self.repair_alert.close();
                }
            }
        }
        true
    }

    /// Share the existing noninteractive notice slot; a confirmation keeps priority.
    /// This does not extend the transport footer or register any input stops.
    pub fn favorite_notice(&mut self, message: &'static str, now: u32) -> bool {
        if self.repair_alert.visible() { return false; }
        self.clear_volume_notice();
        self.reminder_notice.open_notice(plx_platform::i18n::msg::ott_favorites_c(), vec![message.into()]);
        self.reminder_notice_until = Some(now.wrapping_add(input::HUD_LINGER_MS));
        plx_machine::idle::invalidate();
        true
    }

    /// Show the chosen application volume, without claiming measured speaker output.
    /// Repeated changes update the shared passive card in place, never restarting its entrance.
    pub fn volume_notice(&mut self, desired: plx_media::player::volume::DesiredVolume, now: u32) -> bool {
        use plx_platform::i18n::msg;
        if self.repair_alert.visible() || self.reminder_notice.visible() { return false; }
        let body = if desired.muted() { msg::widgets_toggle_off().into() }
            else { format!("{}%", desired.level()).into() };
        if self.volume_notice.is_open() {
            self.volume_notice.reconcile_card(msg::widgets_tracks_audio_c(), vec![body],
                plx_ui::decision_alert::Answers::One);
        } else {
            self.volume_notice.open_notice(msg::widgets_tracks_audio_c(), vec![body]);
        }
        self.number_notice = false;
        self.volume_notice_until = Some(now.wrapping_add(VOLUME_NOTICE_MS));
        plx_machine::idle::invalidate();
        true
    }

    /// Pure presentation gate used by the common application number owner.
    pub fn accepts_channel_number(&self) -> bool {
        self.hud_policy == HudPolicy::Normal && !self.scrub.is_live()
            && !self.repair_alert.visible() && !self.reminder_notice.visible()
    }

    /// Finite-video 1/3 seeks retain the bare-video policy. A visible live HUD
    /// does not prevent the separately source-bound channel number owner.
    pub fn accepts_numeric_seek(&self, session: &plx_media::route::PlaybackSession, now: u32, paused: bool) -> bool {
        self.accepts_channel_number() && !self.hud.visible(session, now, paused)
            && !plx_appkit::player_hud::transport_hidden(session)
    }

    pub fn set_number_notice(&mut self, value: &Option<(String, String)>, now: u32) {
        let Some((number, title)) = value else {
            if self.number_notice { self.clear_volume_notice(); }
            return;
        };
        if self.repair_alert.visible() || self.reminder_notice.visible() { return; }
        let body = vec![number.clone().into(), title.clone().into()];
        if self.volume_notice.is_open() {
            self.volume_notice.reconcile_card(plx_platform::i18n::msg::ott_channel_list_c(), body, plx_ui::decision_alert::Answers::One);
        } else { self.volume_notice.open_notice(plx_platform::i18n::msg::ott_channel_list_c(), body); }
        self.number_notice = true;
        self.volume_notice_until = Some(now.wrapping_add(VOLUME_NOTICE_MS));
        plx_machine::idle::invalidate();
    }

    /// Called only after the application accepts an explicit next/previous request.
    /// Keep the installed channel's facts until the admitted replacement publishes its own.
    pub fn begin_channel_change(&mut self, now: u32) {
        self.channel_change_hud = true;
        self.hud.dismissed = false;
        self.hud.extend_default(now);
        if self.number_notice { self.clear_volume_notice(); }
        self.publish();
    }

    /// Presentation consumes the application's source-fenced loading fact, never clears media errors.
    pub fn set_playback_feedback(&mut self, busy: player_hud::Busy, now: u32) {
        self.busy = busy;
        if self.channel_change_hud {
            // A deliberate Back/dismiss remains authoritative even during a slow resolve.
            if !self.hud.dismissed { self.hud.extend_default(now); }
            if matches!(busy, player_hud::Busy::None
                | player_hud::Busy::Readout(plx_ui::widgets::StatusKind::Failed, _)) {
                self.channel_change_hud = false;
            }
        }
    }

    fn clear_volume_notice(&mut self) {
        if self.volume_notice.visible() || self.volume_notice_until.is_some() {
            self.volume_notice.close();
            self.volume_notice_until = None;
        }
        self.number_notice = false;
    }

    pub fn clear_reminders(&mut self) {
        self.clear_volume_notice();
        if self.reminder_sheet { self.repair_alert.close(); }
        self.reminder_sheet = false;
        self.reminder_prompt = None;
        self.reminder_focus = false;
        self.reminder_notice.close();
        self.reminder_notice_until = None;
    }

    /// **Mirror the two published fields into `player::TX`** (§2.3).
    ///
    /// `hud.until` and `scrub.ns` are this screen's decisions; the draw path
    /// (`player_hud::classic::draw_hud` reads `TX.scrub_ns`) and the engine read them, so the owner writes
    /// a snapshot after every step rather than letting either reader reach into the screen. The
    /// loop calls this once per iteration, after the ladders and the dispatcher frame have both
    /// had their turn — one publication per frame, and nothing between them can observe a
    /// half-written pair.
    pub fn publish(&self) {
        use std::sync::atomic::Ordering::Relaxed;
        plx_media::player::TX.hud_until.store(self.hud.until, Relaxed);
        // A reveal (LEFT/RIGHT on a hidden transport, `ScrubPress::Reveal`) has armed the gesture
        // with its preview on the playhead, but nothing has travelled: it is not yet a scrub, so
        // it is not published as one. `TX.scrub_ns >= 0` is what fades the title, control row and
        // tabs away and floats the card, and a tap that only raises the transport must not blink
        // them. The bar shows the same position either way (the seed IS the playhead); the ramp
        // retires `reveal` on real travel and the preview is published from that frame.
        plx_media::player::TX.scrub_ns.store(self.published_scrub(), Relaxed);
    }

    /// The scrub target [`publish`](Self::publish) writes to `TX.scrub_ns`, and so what the draw's
    /// `HudModel::scrub_ns` reads: -1 while nothing has travelled.
    fn published_scrub(&self) -> i64 {
        if self.scrub.reveal { -1 } else { self.scrub.ns }
    }

    /// **A teardown or an engine reload has retired the transport's mailboxes.**
    ///
    /// `Transport::reset`/`reset_for_reload` used to clear `hud_until`/`scrub_ns` themselves, which
    /// is a write to another owner's state (§2.3). They no longer touch either; `player::teardown`
    /// runs on the main thread and the loop delivers this instead, so the screen clears its own
    /// copy and the next [`publish`](Self::publish) republishes it.
    ///
    /// `for_reload` is the same distinction the transport draws: a reload is the SAME playback at a
    /// new position (a seek, a quality pick, an app-switch resume), so the pending scrub is retired
    /// while a relative tap burst keeps its requested target. A real stop ends the session, so the
    /// burst, HUD timer, dismissal and Up Next countdown go with it.
    pub fn transport_reset(&mut self, for_reload: bool) {
        self.transport_tap = false;
        self.scrub.cancel();
        self.row.reset_title();
        if !for_reload {
            self.seek_burst.reset();
            self.hud.until = 0;
            self.hud.dismissed = false;
            self.hud.last_offer = None;
            self.up_next.reset();
        }
        self.publish();
    }

    /// Draw the image-subtitle display set — the render half this screen owns.
    pub fn draw_subtitle_bitmap(&mut self, hud_up: bool, band: plx_appkit::player_hud::HudBand) {
        plx_appkit::player_hud::draw_subtitle_bitmap(&mut self.render.subs, hud_up, band, &self.model);
    }

    /// The transport, with this instance's own springs, memo and countdown. Paint only: what a
    /// pointer can hit is registered by [`Self::record_stops`], from this screen's `Focusable`.
    pub fn draw_hud(
        &mut self,
        ps: &plx_media::route::PlaybackSession,
        now: u32,
        measure: &dyn plx_machine::machine::Measure,
        meta: plx_data::metadata::MetadataView<'_>,
    ) {
        self.draw_hud_in_mode(ps, now, measure, meta, plx_machine::machine::UiMode::Modern);
    }

    fn draw_hud_in_mode(
        &mut self,
        ps: &plx_media::route::PlaybackSession,
        now: u32,
        measure: &dyn plx_machine::machine::Measure,
        meta: plx_data::metadata::MetadataView<'_>,
        mode: plx_machine::machine::UiMode,
    ) {
        plx_appkit::player_hud::classic::draw_hud_for_display(
            ps,
            &self.model,
            &mut self.row,
            &self.up_next,
            self.slot,
            self.busy,
            self.hud.nav.focus,
            self.hud.nav.btn,
            self.hud.nav.tab,
            now,
            self.transport,
            measure,
            meta,
            mode,
            self.pending_channel_display.as_ref(),
        );
    }

    /// **Every element this screen's `Focusable` declares and the frame DRAWS, registered as a
    /// pointer stop at the rect `place` gives it** — the rect D-pad focus uses, so "reachable by
    /// the D-pad" and "clickable with the pointer" are one statement (issue #162).
    ///
    /// Until #162 the stops were a by-product of painting: `player_hud::classic::draw_hud` pushed each rect
    /// as it drew, in PAINT order, and the hit map resolves the LAST stop under the pointer. The
    /// scrubber's grab band (160 px tall) was pushed after the control row it overlaps, so a click
    /// on the middle of any disc resolved to the scrubber and seeked instead; and a stand-in
    /// (Skip / Up Next) got one floor-width region, so *Watch Credits* had no stop at all and a
    /// click on *Next Episode* activated item 0. Here the ORDER of [`Focusable::groups`] is the
    /// z-order — the scrubber's band first, underneath every control it overlaps — and every item
    /// of every group is registered, so neither can recur without the census test in this module
    /// failing.
    ///
    /// `hud_drawn` is the draw's own gate (the transport or a panel lifting it is on screen); the
    /// per-group half is [`Self::group_drawn`].
    fn record_stops<H: PlayerLike + crate::registry::MetadataLike>(
        &self,
        f: &mut DrawFrame<'_, '_, H>,
        hud_drawn: bool,
    ) {
        if !f.records_stops() { return; }
        // ONE report of where the layout put everything this frame (it measures the row's stand-ins
        // and the tab pills), shared by the group list and every stop placed from it.
        let geo = self.hud_geometry(f.cx);
        let mut groups = Vec::new();
        self.groups_in(&geo, f.cx, &mut groups);
        for g in groups.iter().filter(|g| self.group_drawn(g.id, hud_drawn)) {
            if g.id == GROUP_SCRUB && H::ui_mode(&f.cx.views) == plx_machine::machine::UiMode::Modern
                && !player_hud::modern::shows_timeline(&self.model, H::metadata(f.cx).now_playing()) { continue; }
            if g.id == GROUP_SCRUB && player_hud::classic::rail_domain(H::ui_mode(&f.cx.views),
                &self.model, H::metadata(f.cx).now_playing()).is_read_only() { continue; }
            // The repair alert's buttons are ordinary pressable controls (hover parks focus, the
            // press dips and commits on release); the transport's are `Direct` — see `draw`.
            let (hover, activate) = if g.id == GROUP_REPAIR {
                (Hover::Focus, Activate::Press)
            } else {
                (Hover::Ignore, Activate::Direct)
            };
            for i in 0..g.len as u32 {
                let elem = Self::elem_of(g.id, i);
                let Some(p) = self.place_in(&geo, &elem, f.cx) else { continue };
                f.stop(
                    plx_ui::Painter::root(),
                    Stop {
                        key: FocusKey { entry: self.entry, elem },
                        rect: p.rect,
                        rest_rect: p.rest_rect,
                        clip: p.clip,
                        hover,
                        activate,
                    },
                );
            }
        }
    }

    fn failure_banner_visible(&self) -> bool {
        player_hud::failure_row_drawn(self.busy) && !self.failure_dismissed
    }

    fn refresh_failure_banner(&mut self, now: u32) {
        if player_hud::failure_row_drawn(self.busy) {
            self.failure_dismissed = false;
            self.failure_until = Some(now.wrapping_add(5_000));
        }
    }

    fn settle_failure_banner(&mut self, now: u32,
        attempt: Option<(Option<plx_media::route::PlaybackIdentity>, plx_media::player::FailureKind)>) {
        let failed = attempt.is_some();
        if failed && (!self.failure_up || self.failure_attempt != attempt) {
            self.refresh_failure_banner(now);
        }
        if !failed {
            self.failure_until = None;
            self.failure_dismissed = false;
        } else if self.failure_until.is_some_and(|until| now.wrapping_sub(until) < (1 << 31)) {
            self.failure_dismissed = true;
        }
        self.failure_attempt = attempt;
        self.failure_up = failed;
    }

    /// Is group `g` on screen this frame — the same gates `player_hud::classic::draw_hud`/`draw_readout`
    /// paint behind. A transient failure notice owns no group or hit target;
    /// source navigation remains separate from unavailable decoder transport.
    fn group_drawn(&self, g: GroupId, hud_drawn: bool) -> bool {
        let failed = player_hud::failure_row_drawn(self.busy);
        match g {
            GROUP_REPAIR => self.repair_alert.is_open() && self.repair_alert.settled(),
            GROUP_FAILURE => false,
            GROUP_SCRUB => hud_drawn && !failed && self.transport,
            // faded away by a live scrub (and until they are back) means not hoverable or
            // clickable: `Motion::chrome_interactive`
            GROUP_ROW => hud_drawn && !failed && self.transport && self.row.chrome_interactive(),
            GROUP_TABS => hud_drawn && !failed && self.row.chrome_interactive(),
            _ => false,
        }
    }

    /// Element `i` of group `g` — the inverse of `group_of`.
    fn elem_of(g: GroupId, i: u32) -> u32 {
        match g {
            GROUP_SCRUB => player_hud::ELEM_SCRUB,
            GROUP_ROW => player_hud::control_elem(i as i32),
            GROUP_TABS => player_hud::ELEM_TAB_BASE + i,
            GROUP_FAILURE => player_hud::ELEM_FAILURE_BASE + i,
            _ => REPAIR_CANCEL + i,
        }
    }

    /// **Everything this page draws from a CLOCK, folded into ONE value** (spec §8.3, §9).
    ///
    /// The loop compares it with the previous frame's and raises `plx_machine::idle::invalidate()` when it
    /// changes; `Player::note_clock` owns the comparison. This is what replaces the exemption the
    /// player route used to have from the present gate.
    ///
    /// **Why a fingerprint and not an `invalidate` per animator.** `note_spring` sees every spring
    /// in the app for free, because a spring KNOWS whether it is still travelling. None of the
    /// terms below does: each is a value read fresh from a clock (`playpos_ns`, written by LG's
    /// media thread), from a DEADLINE (the HUD's auto-hide, the two-second `Play` mark) or from
    /// pipeline state, and an animator that reported unconditionally from such a read would
    /// present every frame — the gate turned off by another name, which is exactly what the route
    /// exemption was. A frame is owed when the ANSWER CHANGES, and that is one comparison for the
    /// whole page instead of a dozen hand-placed calls with nothing keeping them in step.
    ///
    /// The terms, and the animator class each stands for (the inventory this replaces):
    /// * `playpos_ns` / `seek_display_ns` / `scrub_ns`, WHILE THE TRANSPORT IS UP — the scrub bar,
    ///   the playhead knob, both clocks and the accelerating hold-to-scrub ramp: the MEDIA CLOCK
    ///   class. Conditional because none of it is drawn with the HUD hidden, which is ~99% of a
    ///   playback; reporting it there would present every frame for a picture nobody draws.
    /// * `ass.fingerprint()`, `sidecar::notice_shown()` and `active_bitmap_key` — text subtitles
    ///   (libass's frame serial), the sidecar failure notice and its 6 s expiry, and image
    ///   subtitles: a whole-screen content change with no spring behind it, drawn on every frame
    ///   of the route.
    /// * `hud_up` — the auto-hide DEADLINE. At `now == until` the entire transport disappears.
    /// * `since_play_ms < PLAY_MARK_MS` — the two-second `Play` mark, plus `paused`: the transport
    ///   MARK class, which is otherwise pure arithmetic on the same clocks.
    /// * `slot` — the Skip Intro / Skip Credits / Up Next occupant, which changes when the
    ///   playhead crosses a marker boundary. (`up_next`'s countdown reports from its own draw and
    ///   is not repeated here.)
    /// * `busy` — the read-out appearing and vanishing, including the `Playing -> Error` edge with
    ///   the HUD auto-hidden, where nothing else in the frame moves at all.
    pub fn clock_fingerprint(&self, ps: &plx_media::route::PlaybackSession, now: u32) -> u64 {
        self.clock_fingerprint_in_mode(ps, now, plx_machine::machine::UiMode::Modern, None)
    }

    pub fn clock_fingerprint_in_mode(&self, ps: &plx_media::route::PlaybackSession, now: u32,
        mode: plx_machine::machine::UiMode, playing: Option<&plx_data::metadata::NowPlaying>) -> u64 {
        use std::sync::atomic::Ordering::Relaxed;
        let playing = self.pending_channel_display.as_ref().or(playing);
        let pos = plx_media::player::playpos_ns();
        let hud_up = self.hud_up(ps, now);
        let mut h: u64 = 0;
        let mut mix = |v: u64| {
            // FNV-1a's constants, one round per term: this is an equality fingerprint, not a
            // digest, and the cost is paid on every frame of the route.
            h = (h ^ v).wrapping_mul(0x0100_0000_01b3);
        };
        mix(u64::from(self.pending_channel_display_stamp.is_some()));
        if let Some((generation, play_generation, revision, clock_ready)) = self.pending_channel_display_stamp {
            mix(generation); mix(play_generation); mix(revision); mix(u64::from(clock_ready));
        }
        // The playhead's three readings move the SCRUB BAR and the two clocks, which exist only
        // while the transport is up. Folding them in unconditionally would report motion for a
        // picture nobody is drawing — playback spends ~99% of its time with the HUD auto-hidden,
        // where the frame is already 0 draw calls, and a report there is the gate turned off.
        // Paused/stalled decoder time cannot drive the visible wall clock or
        // guide interval rollover. Fold only the supplied whole-second sample,
        // and only while the Classic information band or typed Modern live programme is actually drawn.
        if (mode == plx_machine::machine::UiMode::Classic
            || playing.and_then(|now| now.ott.as_ref()).is_some_and(|ott| ott.live.is_some()))
            && self.hud_drawn(ps, now)
            && self.transport && !player_hud::readout_owns_frame(self.busy)
            && self.row.chrome_visible(now) {
            mix(1);
            mix(u64::from(self.model.wall_now_s.is_some()));
            mix(self.model.wall_now_s.unwrap_or_default() as u64);
            mix(self.model.video_raster.0.max(0) as u64);
            mix(self.model.video_raster.1.max(0) as u64);
        }
        // Background preferences owe a frame only while their ground is drawn.
        if self.hud_drawn(ps, now) {
            mix(u64::from(self.model.osd_opacity.level()));
        }
        if let Some((style, minute, opacity)) = self.permanent_clock_key() {
            mix(style);
            mix(minute as u64);
            mix(u64::from(opacity));
        }
        if hud_up {
            mix(pos as u64);
            mix(plx_media::player::seek_display_ns() as u64);
            mix(plx_media::player::TX.scrub_ns.load(Relaxed) as u64);
            // the scrub's fades, card and knob clock: a frame is owed for as long as one runs
            if self.row.scrub_motion_animating(now) {
                mix(u64::from(now));
            }
        }
        mix(u64::from(plx_media::player::TX.paused.load(Relaxed)));
        // …subtitles are NOT conditional: they are drawn on every frame of the route, HUD or no
        // HUD, which is exactly why a cue appearing had to become a report.
        mix(self.render.ass.fingerprint());
        // A paused cue must move with an accepted picture change even with no new cue or clock.
        let picture = self.model.video_presentation;
        for v in [picture.x, picture.y, picture.width, picture.height] { mix(v as u64); }
        // the sidecar failure notice lapsing on the wall clock: nothing else on the screen moves
        // then, so the instant it expires still owes a frame
        mix(u64::from(plx_media::player::sidecar::notice_shown()));
        mix(plx_media::player::active_bitmap_key(pos).unwrap_or(0) as u64);
        mix(u64::from(hud_up));
        mix(u64::from(self.failure_banner_visible()));
        mix(u64::from(
            self.row
                .since_play_ms(now)
                .is_some_and(|d| d < plx_appkit::player_hud::PLAY_MARK_MS),
        ));
        mix(match self.slot {
            ControlSlot::Discs(controls) => 4 + u64::from(controls.pair()) + 2 * u64::from(controls.previous) + 4 * u64::from(controls.next) + 8 * u64::from(controls.restart),
            ControlSlot::Skip(_) => 2,
            ControlSlot::UpNext(_) => 3,
        });
        mix(match plx_appkit::player_hud::busy(ps) {
            plx_appkit::player_hud::Busy::None => 1,
            plx_appkit::player_hud::Busy::Transport => 2,
            // the caption is a `&'static CStr`: its ADDRESS is a stable identity for the message
            plx_appkit::player_hud::Busy::Readout(k, c) => {
                4 ^ ((k as u64) << 32) ^ (c.as_ptr() as u64)
            }
        });
        h
    }

    /// Is the transport on screen right now? `false` whenever a surface HIDES it
    /// ([`HudPolicy::Hidden`], the Timing capsule) regardless of what the timer/dismissal say —
    /// gated HERE, not just at the draw call site, so [`Self::clock_fingerprint`] (which calls this
    /// too) sees the same answer the frame draws.
    pub fn hud_up(&self, ps: &plx_media::route::PlaybackSession, now: u32) -> bool {
        self.hud.visible(ps, now, plx_media::player::TX.paused.load(std::sync::atomic::Ordering::Relaxed))
            && self.hud_policy != HudPolicy::Hidden
    }

    /// Complete an information close only for the playback the overlay captured.
    pub fn hide_info(&mut self, ps: &plx_media::route::PlaybackSession,
        source: &plx_media::route::PlaybackIdentity) -> bool {
        if !plx_media::route::playback_identity_matches(ps, source) { return false; }
        self.scrub.cancel();
        self.end_jump_gesture();
        self.hud.hide(false);
        self.publish();
        true
    }

    /// **Push this frame's [`HudPolicy`]** (`app/run.rs`, from `bridge::player_hud_policy`) — and
    /// the ONE place a HUD-hiding surface's close becomes a dismissed transport. On the falling
    /// edge out of [`HudPolicy::Hidden`] (the capsule's surface has just left, whatever closed it:
    /// OK/BACK, a pointer miss through `OnMiss::Dismiss`, a playback teardown) `hud.dismissed` is
    /// set, so the transport does not reappear over the film just because the capsule stopped
    /// hiding it. It stays down until a bound key raises it, exactly like an UP-hide.
    ///
    /// **A pending scrub target ends with the page being the viewer's**: on a surface taking it
    /// (the rising edge out of [`HudPolicy::Normal`]) — Chapters and *From Beginning* seek from
    /// inside their panel, and the panel's own BACK is not the scrub's, and OK there is not the
    /// scrub's confirmation — and on that dismissal, since a transport that is not on screen has
    /// nothing to confirm. The target is CANCELLED there, never committed.
    pub fn set_hud_policy(&mut self, policy: HudPolicy) {
        if self.hud_policy == HudPolicy::Hidden && policy != HudPolicy::Hidden {
            self.hud.hide(false);
        }
        if self.hud_policy == HudPolicy::Normal && policy != HudPolicy::Normal {
            self.scrub.cancel();
            self.end_jump_gesture();
            self.publish();
        }
        self.hud_policy = policy;
    }

    /// The app samples route, window/standby and covering surfaces before the
    /// fingerprint. The clock never keeps the transport timer or input owner alive.
    pub fn set_osd_uncovered(&mut self, uncovered: bool) {
        self.osd_uncovered = uncovered;
    }

    fn permanent_clock_key(&self) -> Option<(u64, i64, u8)> {
        if !self.osd_uncovered || self.hud_policy != HudPolicy::Normal
            || self.repair_alert.visible() || self.reminder_notice.visible()
            || player_hud::readout_owns_frame(self.busy) {
            return None;
        }
        let minute = player_hud::osd::clock_minute_key(&self.model)?;
        let opacity = if self.model.permanent_clock == player_hud::model::PermanentClock::Background {
            self.model.osd_opacity.level()
        } else { 0 };
        Some((self.model.permanent_clock.index(), minute, opacity))
    }

    /// A fresh segment offer raises the transport onto the control row (`app/run.rs`, once per
    /// segment): [`HudState::raise_for_offer`]. The ring leaves the scrubber, so a target pending
    /// on it is cancelled — never committed by the OK that is now meant for the offer.
    pub fn raise_for_offer(&mut self, now: u32, primary: std::os::raw::c_int) {
        self.scrub.cancel();
        self.hud.raise_for_offer(now, primary);
        self.publish();
    }

    /// Does the caption block lift clear of the transport this frame? While the transport is up,
    /// or while a panel is being read over it.
    fn subs_lift(&self, ps: &plx_media::route::PlaybackSession, now: u32) -> bool {
        self.hud_up(ps, now) || self.hud_policy == HudPolicy::Lifted
    }

    /// Is the transport actually DRAWN this frame? Whenever the captions lift for it, unless the
    /// repair alert has the screen.
    fn hud_drawn(&self, ps: &plx_media::route::PlaybackSession, now: u32) -> bool {
        self.subs_lift(ps, now) && !self.repair_alert.visible()
    }
}

/// The focus groups [`PlayerScreen`] registers (restructure phase 12, D2): the scrubber, the
/// control row (discs, or the one region a Skip/Up Next stand-in occupies), the bottom tabs, and —
/// only while a failure owns the frame — the read-out's row of actions. The first four are the
/// HUD's own and live in `appkit::player_hud::geometry` ([`GROUP_SCRUB`], [`GROUP_ROW`],
/// [`GROUP_TABS`], [`GROUP_FAILURE`]), because the HUD's motion and drawing compare a focus against
/// the same numbers; element addresses within each are [`player_hud::ELEM_SCRUB`]/
/// [`player_hud::ELEM_CONTROL_BASE`]/[`player_hud::ELEM_TAB_BASE`]/[`player_hud::ELEM_FAILURE_BASE`].
/// The sandbox-repair confirmation's group is this page's alone.
const GROUP_REPAIR: GroupId = GroupId(4);
const REPAIR_CANCEL: u32 = 40_000;
const REPAIR_CONFIRM: u32 = REPAIR_CANCEL + 1;


impl<H: PlayerLike + crate::registry::MetadataLike> Machine<H> for PlayerScreen {
    type Ev = ScreenEvent<H>;
    /// **The transport's key/click ladder, as the receiving end of the phase-12 contract freeze.**
    ///
    /// `Tick`/`Unmount` are unchanged from before this phase; `Input` is this package's addition.
    /// An owned key is classified with [`consts::classify_input`] into the full [`consts::Key`] alphabet —
    /// never a type under `crate::app::`, which `ci/check-deps.sh`'s `layer` gate forbids `screens/`
    /// from naming — and dispatched to one of the private `key_*` helpers below, each a direct port
    /// of the `app/run.rs` arm it replaces (`key_ok`/`key_player_updown`/`key_scrub`'s Route::Player
    /// cases). A raw click's `hit` (filled by the dispatcher from this screen's own registered
    /// [`Stop`]s — see [`Screen::draw`]) is resolved the same way `PlayerOverlayScreen::step`
    /// resolves one. Anything privileged (pausing/resuming, seeking, presenting an overlay, leaving
    /// the player) is a `fx.push(Fx::App(AppFx::Player(PlayerReq::…)))` request, exactly as
    /// [`PlayerOverlayScreen::ask`] performs one — `PlayerReq::OpenOverlay`/`ArmControlRow`/`Exit`
    /// are the three variants this package added for it. `Exit`, the remote's own key, is the one
    /// this screen deliberately does NOT try to own (see the doc this replaced); it is not matched
    /// below and falls to `Handled::No`.
    ///
    /// **The scrub gesture has exactly ONE owner, and it is this screen** (restructure phase 12,
    /// PX-PLAYER). Phase 9 left the loop a CO-owner of `PlayerScreen::scrub`: `app/input.rs`'s
    /// `on_key_up`/`on_auto_repeat` wrote it on every key edge, and `app/run.rs` ran the continuous
    /// advance and its lost-keyup net on it once per frame. The "documented simplifications" that
    /// stood in this doc were consequences of that split rather than choices, and they are gone
    /// with the arms that forced them: the lost-keyup net is reproduced on the same `Tick` as the
    /// ramp it guards, and the screen states its INTENT while the pump keeps a pause across a seek,
    /// which is what a request is for.
    ///
    /// **D-pad previews and held FF/REW scrubs wait for OK** (click-to-confirm, the same on a
    /// playing film as on a paused one). Default LEFT/RIGHT gestures only move the TARGET; FF/REW taps jump on release, while holds preview.
    /// With instant D-pad seeking enabled, hidden-HUD LEFT/RIGHT commits a jump on press;
    /// releasing a key, the lost-keyup net and every timer end the key gesture and seek nothing, and
    /// the film plays on underneath with the transport held up ([`Self::hold_the_transport_for_a_target`]).
    /// **OK, PLAY and PLAY-PAUSE take the target** ([`Self::take_target`]): one
    /// [`PlayerReq::SeekTo`], which leaves the film playing (a paused one resumes). BACK drops it
    /// with no seek, and so does anything else that takes the page — a panel or the capsule over the
    /// transport ([`Self::set_hud_policy`]), UP/DOWN off the scrubber, a Skip offer or the control
    /// row's press, a teardown: a target is cancelled, never committed, by the transport going away.
    ///
    /// **BACK around a scrub.** While a target is live (a hold, a pending target, a seated drag) BACK
    /// CANCELS it — no seek, the knob springs home (`knob_spring`, in `appkit`). A commit (OK / PLAY,
    /// or a drag's release) is final: BACK afterwards is BACK's ordinary meaning, there is no way
    /// back to where the scrub started.
    ///
    /// The POINTER half is here for the same reason. A click on the scrubber's registered stop
    /// seeds the preview under the pointer, an `InputKind::Drag` carries it (§7.5: a drag drives
    /// its control with no hover and no click), and the button coming up — which reaches a screen
    /// as `Ok`/`Edge::Up`, the only release the dispatcher's `InputKind` has — commits it
    /// ([`PlayerReq::CommitSeek`]: a playing film plays on, a paused one stays paused). That
    /// release IS the pointer's confirmation, so unlike a key target it asks for no OK. Before
    /// this the stop landed on `key_ok`'s final `else` and toggled play/pause, and the drag flag
    /// was a field of the loop's pointer machine whose producer had become unreachable.
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, H>, fx: &mut Effects<'_, H>) -> Handled {
        self.hud.set_info_timeout(H::info_timeout(cx));
        // Ingress queues a complete batch before delivery. Sample each bound press here,
        // before it can change the HUD, so a later queued key cannot overwrite this one's
        // pre-press visibility or BACK arming. The dispatcher drops stale owners first.
        if let ScreenEvent::Input(input) = ev {
            if let InputKind::Key { sym, wcode, edge: Edge::Down, at_edge: false, .. } = input.kind {
                if consts::is_bound(sym, wcode) {
                    if let Some(paused) = H::hud_key_prelude(cx, input.source) {
                        self.hud.note_fresh_press(H::session(cx), input.at.ms, paused);
                        self.publish();
                    }
                }
            }
        }
        if self.repair_alert.visible() {
            match ev {
                ScreenEvent::FocusMoved { to, .. } => {
                    self.repair_alert.set_choice(if to.elem == REPAIR_CONFIRM { plx_ui::decision_alert::Choice::Destructive } else { plx_ui::decision_alert::Choice::Cancel });
                    return Handled::Yes;
                }
                ScreenEvent::PressCommit(_) => {
                    if let Some(key) = cx.focus.current {
                        if matches!(key.elem, REPAIR_CANCEL | REPAIR_CONFIRM) {
                            self.repair_answer(key.elem == REPAIR_CONFIRM, fx);
                        }
                    }
                    return Handled::Yes;
                }
                ScreenEvent::Activate(_) => return Handled::Yes,
                ScreenEvent::Input(input) => {
                    if !self.repair_alert.is_open() { return Handled::Yes; }
                    return match input.kind {
                        InputKind::Key { key, sym, wcode, edge, .. } => match consts::classify_input(key, sym, wcode) {
                            consts::Key::Back | consts::Key::Stop if edge == Edge::Down => {
                                self.repair_answer(false, fx); Handled::Yes
                            }
                            direction @ (consts::Key::Up | consts::Key::Down) if edge != Edge::Up => {
                                self.repair_scroll = self.repair_alert.scroll_by(cx.measure,
                                    if matches!(direction, consts::Key::Up) { -1 } else { 1 });
                                Handled::Yes
                            }
                            consts::Key::Left { .. } | consts::Key::Right { .. } | consts::Key::Ok | consts::Key::Exit => Handled::No,
                            _ => Handled::Yes,
                        },
                        InputKind::Pointer { .. } | InputKind::Click { .. } => Handled::No,
                        _ => Handled::Yes,
                    };
                }
                _ => {}
            }
        }
        // Page-owned modal entries receive input before this screen. The local
        // repair sheet above also wins; only an explicit scene-selection offer
        // can take directions/confirmation from the ordinary transport.
        if let Some(frame) = H::multiview_offer(cx).filter(|frame| frame.state().input_owned) {
            match ev {
                ScreenEvent::Input(input) => if let Some(event) = multiview_input(&frame, &input.kind) {
                    if let Some(event) = event { fx.push(Fx::App(AppFx::Player(PlayerReq::Multiview(event)))); }
                    return Handled::Yes;
                },
                // A HUD press queued before scene selection acquired input may
                // not turn into an ordinary playback action under the new scene.
                ScreenEvent::PressCommit(_) | ScreenEvent::Activate(_) => return Handled::Yes,
                _ => {}
            }
        }
        // A mode/geometry change can retire a control after its input was queued.
        // Do not reinterpret that old target as the picture's play/pause action.
        let target = match ev {
            ScreenEvent::FocusMoved { to, .. } => Some(to.elem),
            ScreenEvent::Activate(elem) => Some(*elem),
            ScreenEvent::PressCommit(_) => cx.focus.current.map(|key| key.elem),
            ScreenEvent::Input(input) => match input.kind {
                InputKind::Click { hit, .. } => hit,
                InputKind::Key { key, sym, wcode, .. }
                    if consts::classify_input(key, sym, wcode) == consts::Key::Ok => cx.focus.current.map(|key| key.elem),
                _ => None,
            },
            _ => None,
        };
        let mirrored = match self.hud.nav.focus {
            GROUP_ROW => Some(player_hud::control_elem(self.hud.nav.btn)),
            GROUP_TABS => Some(player_hud::ELEM_TAB_BASE + self.hud.nav.tab.max(0) as u32),
            _ => None,
        };
        let retired_target = !self.repair_alert.visible()
            && target.is_some_and(|elem| self.hidden_classic_control(cx, elem));
        if retired_target || (!self.repair_alert.visible()
            && mirrored.is_some_and(|elem| self.hidden_classic_control(cx, elem))) {
            self.hud.nav.focus = GROUP_SCRUB;
            Self::repair_focus(fx, GROUP_SCRUB);
        }
        if retired_target { return Handled::Yes; }
        match ev {
            // **The engine moved focus: `hud.nav` follows.** The engine is the authority on where
            // focus is; `nav` is the mirror the HUD's motion, drawing and published state read. A
            // re-seat (`By::Restore`, `By::Reconcile`: the app's and this screen's own writes to
            // `nav`, each paired with one) only mirrors. A DIRECTIONAL move that changes group is
            // the user travelling, and carries its side effects: leaving the scrubber drops the
            // scrub target, and any travel is a fresh interaction that re-arms the linger.
            // A move inside a group (the row's Left/Right) keeps its own key handling's extend.
            ScreenEvent::FocusMoved { from, to, by } if let Some(group) = Self::hud_group_of(to.elem) => {
                if let Some(btn) = player_hud::row_index(to.elem) {
                    if self.slot.enabled(btn) {
                        self.hud.nav.focus = GROUP_ROW;
                        self.hud.nav.btn = btn;
                        self.scrub.cancel();
                    }
                } else {
                    self.hud.nav.focus = group;
                    if group == GROUP_TABS {
                        self.hud.nav.tab = (to.elem - player_hud::ELEM_TAB_BASE) as i32;
                    }
                }
                if *by == By::Dir && from.and_then(|f| Self::hud_group_of(f.elem)) != Some(group) {
                    if group != GROUP_SCRUB {
                        self.scrub.ns = -1;
                        self.scrub.disengage();
                    }
                    self.hud.extend_default(cx.tick.ms);
                }
                Handled::Yes
            }
            // **The engine's press on the control row committed** — the dip has played, on the
            // spring-back. What the control the engine holds means is asked of the app.
            ScreenEvent::PressCommit(_) => {
                self.row_pressed(cx.focus.current.map(|k| k.elem), fx);
                Handled::Yes
            }
            ScreenEvent::Tick(tick) => {
                if self.hud.nav.focus == GROUP_ROW && !self.slot.enabled(self.hud.nav.btn) {
                    self.hud.nav.btn = self.slot.primary_btn();
                    Self::repair_focus(fx, GROUP_ROW);
                }
                self.render.ass.update(H::session(cx), tick.ms);
                self.hud.settle(plx_media::player::loading(H::session(cx)));
                let attempt = player_hud::failure_row_drawn(self.busy).then(||
                    (plx_media::route::playback_identity(H::session(cx)),
                     plx_media::player::error_now(H::session(cx)).kind));
                self.settle_failure_banner(tick.ms, attempt);
                let stale = !H::session(cx).jail_load_blocked;
                if self.repair_alert.visible() && !self.reminder_sheet && stale { self.repair_alert.close(); }
                if std::mem::take(&mut self.reminder_focus) { Self::repair_focus(fx, GROUP_REPAIR); }
                self.repair_alert.update(tick.dt());
                if !self.repair_alert.visible() { self.reminder_sheet = false; }
                if self.reminder_notice_until.is_some_and(|until| tick.ms.wrapping_sub(until) < (1 << 31)) {
                    self.reminder_notice.dismiss();
                    self.reminder_notice_until = None;
                }
                self.reminder_notice.update(tick.dt());
                if self.repair_alert.visible() || self.reminder_notice.visible() {
                    self.clear_volume_notice();
                } else if self.volume_notice_until.is_some_and(|until| tick.ms.wrapping_sub(until) < (1 << 31)) {
                    self.volume_notice.dismiss();
                    self.volume_notice_until = None;
                    self.number_notice = false;
                }
                self.volume_notice.update(tick.dt());
                // The control row's springs and the resume clock are stepped once per FRAME and
                // never from `draw_hud` — this row is not drawn on every frame of the route, so a
                // spring advanced in the draw would run at a rate that depended on which overlay
                // was open (`HudMotion::step`'s own doc).
                self.row
                    .step(self.slot, self.hud.nav.focus, self.hud.nav.btn, tick.dt(), tick.ms);
                // …and the Up Next countdown, which arms on APPEARANCE and is dropped with the
                // segment. Stepped for the whole route, not per-overlay: the countdown must keep
                // running whichever panel is up (`up_next::tick`).
                self.up_next.tick(self.slot, tick.ms);
                // The scrub gesture's whole per-frame half — `app/run.rs`'s old
                // per-loop-iteration block, now on this screen's own Tick: the continuous
                // accelerating advance with its lost-keyup net. Neither it nor anything else on
                // this Tick seeks: a key target waits for OK.
                self.step_scrub_hold(tick.ms);
                self.hold_the_transport_for_a_target(tick.ms);
                self.publish();
                Handled::Yes
            }
            ScreenEvent::Unmount => {
                // The GL names in the subtitle set are OWNED. A static could never be told that a
                // playback had ended; an instance is told exactly once.
                self.clear_reminders();
                self.render.subs.release();
                self.render.ass.release();
                Handled::Yes
            }
            ScreenEvent::Input(input) => {
                let ps = H::session(cx);
                match &input.kind {
                    InputKind::Key { key, sym, wcode, edge, at_edge } => {
                        if *edge == Edge::Down && !*at_edge { self.refresh_failure_banner(input.at.ms); }
                        // The ENGINE's focus decides what OK means and which group a re-delivered
                        // edge belongs to; `hud.nav` only mirrors it.
                        let focused = cx.focus.current.map(|k| k.elem)
                            .filter(|elem| !self.hidden_classic_control(cx, *elem));
                        if let Some(handled) = self.handle_ott_key(
                            ps, H::ott_player_offer(cx, input.source), focused, H::ui_mode(&cx.views),
                            plx_appkit::info_panel::live_channel(H::metadata(cx)), *key, *sym, *wcode,
                            *edge, *at_edge, fx,
                        ) { return handled; }
                        self.handle_key(
                            ps, consts::classify_input(*key, *sym, *wcode), *edge, *at_edge, input.at.ms,
                            focused, self.focused_command(focused), fx, H::metadata(cx),
                        )
                    }
                    InputKind::Click { hit, x, y } => {
                        if let Some(handled) = self.classic_menu_click(cx, *hit, *x, *y, fx) {
                            return handled;
                        }
                        self.end_jump_gesture();
                        self.handle_click(cx, *hit, *x, input.at.ms, fx)
                    }
                    // §7.5: a drag drives its CONTROL. Only the scrubber has one here, and only
                    // once a click has seated the gesture on it — a drag that began anywhere else
                    // must not capture the bar the moment it passes over it.
                    InputKind::Drag { x, .. } => {
                        self.end_jump_gesture();
                        self.handle_drag(cx, *x, input.at.ms)
                    },
                    // Pointer MOTION over the bare transport raises it and clears a hand
                    // dismissal, which is how a Magic Remote user finds the HUD over full-screen
                    // video. `app/run.rs`'s own motion arm did this and then RETURNED, so the page
                    // never saw a pointer event at all.
                    InputKind::Pointer { .. } => {
                        self.end_jump_gesture();
                        self.hud.dismissed = false;
                        self.hud.extend_default(input.at.ms);
                        self.publish();
                        Handled::Yes
                    }
                    _ => Handled::No,
                }
            }
            _ => Handled::No,
        }
    }
}

/// `Some(None)` consumes a scene-selection edge without falling through to
/// transport (key-up or a boundary); `None` leaves unrelated controls untouched.
fn multiview_input(frame: &plx_appkit::multiview_chrome::ChromeFrame,
    kind: &InputKind<u32>) -> Option<Option<plx_appkit::multiview_chrome::Event>>
{
    use plx_appkit::multiview_chrome::Input;
    use plx_appkit::multiview_layout::{PointerAction, PointerMap};
    match *kind {
        InputKind::Key { key, sym, wcode, edge, at_edge } => {
            let input = match consts::classify_input(key, sym, wcode) {
                consts::Key::Up => Input::Direction(Dir::Up),
                consts::Key::Down => Input::Direction(Dir::Down),
                consts::Key::Left { alt: false } => Input::Direction(Dir::Left),
                consts::Key::Right { alt: false } => Input::Direction(Dir::Right),
                consts::Key::Ok => Input::Confirm,
                _ => return None,
            };
            Some(if edge != Edge::Up && !at_edge
                && (!matches!(input, Input::Confirm) || edge == Edge::Down)
                { frame.input(input) } else { None })
        }
        InputKind::Click { x, y, .. } => {
            let viewport = frame.layout().viewport();
            let map = PointerMap::contain(viewport, viewport).ok()?;
            Some(frame.pointer(&map, PointerAction::PrimaryClick, x, y))
        }
        _ => None,
    }
}

/// **Does this key press continue the relative-seek gesture in progress?** Every key but LEFT and
/// RIGHT ends it ([`PlayerScreen::handle_key`] resets the burst), except OK on a control whose
/// command is a seek: two seek controls pressed inside [`BURST_GAP_MS`](plx_appkit::player_hud::seek_jump::BURST_GAP_MS) are one gesture, and their steps
/// accumulate into one target.
fn continues_seek_burst(key: consts::Key, command: Option<player_hud::command::HudCommand>) -> bool {
    key == consts::Key::Ok && matches!(command, Some(player_hud::command::HudCommand::Seek(_)))
}

/// Whether OK currently belongs to an enabled control in the transport row.
/// A live-channel catalog shortcut may use OK only when the HUD has no such
/// focused target; otherwise the focused control keeps its normal activation.
fn actionable_hud_target_focused(slot: ControlSlot, focused: Option<u32>) -> bool {
    focused.is_some_and(|elem| {
        player_hud::row_index(elem).is_some_and(|index| slot.enabled(index))
            || (player_hud::ELEM_TAB_BASE..player_hud::ELEM_FAILURE_BASE).contains(&elem)
    })
}

fn live_catalog_shortcut_on_ok(
    key: consts::Key,
    live_channel: bool,
    accept: crate::registry::OttPlayerAction,
    hud_visible: bool,
    actionable_control_focused: bool,
) -> bool {
    key == consts::Key::Ok
        && live_channel
        && accept == crate::registry::OttPlayerAction::OpenCatalog
        && !(hud_visible && actionable_control_focused)
}

impl PlayerScreen {
    fn ask<H: AppLike>(fx: &mut Effects<'_, H>, req: PlayerReq) {
        fx.push(Fx::App(AppFx::Player(req)));
    }

    /// Re-seat the engine on `group` by that group's own seat policy: the pairing of every write
    /// this screen makes to `hud.nav`, so the engine (which decides what OK means) and the mirror
    /// never disagree. The arrival is a `By::Restore`, which `step` only mirrors.
    fn repair_focus<H: AppLike>(fx: &mut Effects<'_, H>, group: GroupId) {
        fx.push(Fx::Deliver(
            fx.from(),
            plx_machine::machine::Delivery::Screen(ScreenEvent::Enter(plx_ui::screen::Enter::Fresh {
                focus: plx_ui::screen::FocusTarget::ContainerGroup(group),
            })),
        ));
    }

    /// Re-seat the engine on one element: a pointer click on a control, which `Activate::Direct`
    /// stops never move focus to themselves.
    fn seat_elem<H: AppLike>(&self, fx: &mut Effects<'_, H>, elem: u32) {
        fx.push(Fx::Deliver(
            fx.from(),
            plx_machine::machine::Delivery::Screen(ScreenEvent::Enter(plx_ui::screen::Enter::Fresh {
                focus: plx_ui::screen::FocusTarget::Elem(FocusKey { entry: self.entry, elem }),
            })),
        ));
    }
    fn repair_answer<H: AppLike>(&mut self, confirm: bool, fx: &mut Effects<'_, H>) {
        if !self.repair_alert.is_open() { return; }
        self.repair_alert.dismiss();
        if self.reminder_sheet {
            if let Some(token) = self.reminder_prompt.take() {
                if let plx_machine::machine::MachineId::Instance(reply) = fx.from() {
                    fx.push(Fx::App(crate::registry::reminder_answer(reply, token, confirm)));
                }
            }
            Self::repair_focus(fx, GROUP_ROW);
        } else {
            if confirm { Self::ask(fx, PlayerReq::RepairSandbox); }
            Self::repair_focus(fx, GROUP_ROW);
        }
    }
    fn repair_rect(&self, confirm: bool, measure: &dyn plx_machine::machine::Measure) -> Rect {
        let (cancel, repair) = self.repair_alert.frames(measure);
        if confirm { repair } else { cancel }
    }

    /// One registered click's element resolves to an action — the pointer twin of `handle_key`'s
    /// `Ok`/`Left`/`Right` arms.
    ///
    /// The scrubber's arm takes the click's own `x`, which is the whole difference from the phase-9
    /// version: a click on the bar is a SEEK TO THE POINTED POSITION and the opening of a possible
    /// drag, not the play/pause toggle it fell through to. Nothing is committed here — the commit
    /// is the button coming up ([`Self::handle_release`]), exactly as it was when `app/run.rs`
    /// owned this: a click with no motion is simply a drag of length zero, so one mechanism serves
    /// both and a click cannot commit a position the user never saw previewed.
    ///
    /// A click can only be resolved to an element the frame REGISTERED, and the transport's stops
    /// are registered only while it is drawn ([`Screen::draw`]) — which is the same protection the
    /// old ladder spelled out by sampling `hud_vis` before re-arming the HUD, after "a click in the
    /// invisible timed-out scrub band committed a blind seek".
    fn pointer_seek_duration<H: PlayerLike + crate::registry::MetadataLike>(&self, cx: &Cx<'_, H>) -> i64 {
        let mode = H::ui_mode(&cx.views);
        if mode == plx_machine::machine::UiMode::Modern {
            return if player_hud::modern::shows_timeline(&self.model, H::metadata(cx).now_playing()) {
                self.model.duration_ns.max(0)
            } else { 0 };
        }
        player_hud::classic::rail_domain(mode, &self.model, H::metadata(cx).now_playing())
            .seek_duration(self.model.duration_ns)
    }

    /// FOSS's top-fifth video click opens Menu. A presented control, modal,
    /// editor or existing gesture keeps its owner; the visible HUD itself does
    /// not block this background gesture. Coordinates are already normalized
    /// to the same logical frame used by the shared hit map.
    fn classic_menu_click<H: PlayerLike + crate::registry::MetadataLike>(
        &mut self, cx: &Cx<'_, H>, hit: Option<u32>, x: f32, y: f32,
        fx: &mut Effects<'_, H>,
    ) -> Option<Handled> {
        if H::ui_mode(&cx.views) != plx_machine::machine::UiMode::Classic || hit.is_some() {
            return None;
        }
        if !x.is_finite() || !y.is_finite() || x < 0.0 || y < 0.0
            || x >= Rect::FULL.w || y >= Rect::FULL.h {
            return Some(Handled::Yes);
        }
        if y >= Rect::FULL.h * 0.2 { return None; }
        // An unavailable or stale top-band request must not turn into a pause.
        let ps = H::session(cx);
        if cx.owner != plx_machine::machine::InputOwner::Entry(self.entry)
            || self.hud_policy != HudPolicy::Normal || self.scrub.is_live()
            || self.transport_tap || cx.press.owner.is_some()
            || player_hud::failure_row_drawn(self.busy) {
            return Some(Handled::Yes);
        }
        if let (Some(source), plx_machine::machine::MachineId::Instance(owner)) =
            (H::ott_catalog_offer(cx), fx.from()) {
            if plx_media::route::playback_identity_matches(ps, &source) {
                self.end_jump_gesture();
                Self::ask(fx, PlayerReq::OttAction(crate::registry::OttPlayerRequest {
                    owner, source: source.into(), action: crate::registry::OttPlayerAction::OpenMenu,
                }));
            }
        }
        Some(Handled::Yes)
    }

    fn handle_click<H: PlayerLike + crate::registry::MetadataLike>(
        &mut self,
        cx: &Cx<'_, H>,
        hit: Option<u32>,
        x: f32,
        now: u32,
        fx: &mut Effects<'_, H>,
    ) -> Handled {
        use player_hud::{ELEM_FAILURE_BASE, ELEM_FAILURE_END, ELEM_SCRUB, ELEM_TAB_BASE};
        let ps = H::session(cx);
        let seek_duration_ns = self.pointer_seek_duration(cx);
        let failed = plx_appkit::player_hud::transport_hidden(ps);
        let Some(elem) = hit else {
            // **A click that lands on no control at all toggles play/pause** — the `else` arm of
            // the old pointer block, and the reason clicking the PICTURE works: over full-screen
            // video most of the frame is not a control. Not while a failure owns it, where the old
            // block returned before reaching any of this: there is nothing to toggle and the one
            // drawn target is the read-out's own escape above.
            if failed {
                return Handled::Yes;
            }
            self.hud.dismissed = false;
            self.hud.extend_default(now);
            self.publish();
            Self::ask(fx, PlayerReq::Transport(None));
            return Handled::Yes;
        };
        if failed && !(ELEM_FAILURE_BASE..ELEM_FAILURE_END).contains(&elem) {
            return Handled::Yes;
        }
        if self.hidden_classic_control(cx, elem) { return Handled::Yes; }
        // A click on a control is a fresh interaction with the transport, exactly as a key is: it
        // re-arms the linger. Without it a pointer that had rested still for most of the linger
        // could click a disc whose deferred press (~210 ms) then committed AFTER the auto-hide had
        // reset `hud.nav` to HOME — opening Subtitles for a click on Audio.
        self.hud.extend_default(now);
        self.publish();
        match elem {
            e if (ELEM_FAILURE_BASE..ELEM_FAILURE_END).contains(&e) => {
                // Retired failure-page element IDs cannot activate a notice.
                Handled::Yes
            }
            e if e == ELEM_SCRUB => {
                let dur = seek_duration_ns;
                if dur > 0 {
                    self.hud.nav.focus = GROUP_SCRUB;
                    self.seat_elem(fx, ELEM_SCRUB);
                    self.scrub.ns = self.pointed_ns(&self.hud_geometry(cx), x, dur);
                    self.scrub.drag = true; // the release commits; nothing else does
                    self.hud.extend_default(now);
                    self.publish();
                }
                Handled::Yes
            }
            e if let Some(btn) = player_hud::row_index(e) => {
                self.hud.nav.focus = GROUP_ROW;
                if !self.slot.enabled(btn) {
                    Self::repair_focus(fx, GROUP_ROW);
                    return Handled::Yes;
                }
                self.hud.nav.btn = btn;
                self.seat_elem(fx, e);
                self.arm_control_row(fx);
                Handled::Yes
            }
            e if (ELEM_TAB_BASE..ELEM_FAILURE_BASE).contains(&e) => {
                let tab = e - ELEM_TAB_BASE;
                self.hud.nav.focus = GROUP_TABS;
                self.hud.nav.tab = tab as i32;
                self.seat_elem(fx, e);
                let kind = if tab == 0 {
                    overlay::OverlayKind::Info
                } else {
                    overlay::OverlayKind::Chapters
                };
                Self::ask(fx, PlayerReq::OpenOverlay(kind));
                Handled::Yes
            }
            _ => Handled::No,
        }
    }

    /// Where on the timeline a pointer at `x` is pointing, bounded by [`Scrub::clamp_target`].
    /// `HudGeometry::scrub_frac` is x-only on purpose: an engaged drag tracks the pointer even when it
    /// wanders off the band vertically, which is why the drag arm below never re-tests the hit.
    fn pointed_ns(&self, geometry: &HudGeometry, x: f32, duration_ns: i64) -> i64 {
        let frac = geometry.scrub_frac(x) as f64;
        Scrub::clamp_target((frac * duration_ns as f64) as i64, duration_ns)
    }

    /// **A pointer DRAG carries the preview.** Only while a click has seated the gesture on the
    /// bar (`Scrub::drag`): a drag that began on some other control, or on nothing, must not
    /// capture the scrubber the moment it passes over it. Nothing is committed — that is the
    /// release's job.
    fn handle_drag<H: PlayerLike + crate::registry::MetadataLike>(
        &mut self, cx: &Cx<'_, H>, x: f32, now: u32,
    ) -> Handled {
        if !self.scrub.drag {
            return Handled::No;
        }
        let dur = plx_media::player::duration_ns();
        if dur > 0 {
            self.scrub.ns = self.pointed_ns(&self.hud_geometry(cx), x, dur);
        }
        self.hud.dismissed = false;
        self.hud.extend_default(now);
        self.publish();
        Handled::Yes
    }

    /// **The pointer button coming up commits the drag it was previewing.**
    ///
    /// It arrives as `Ok`/`Edge::Up` because that is the only release `InputKind` has: the loop
    /// turns a `SDL_MOUSEBUTTONUP` into one (`bridge::release_input`, whose own doc carries the
    /// reasoning) and the dispatcher's press machine reads it the same way. A real OK key-up
    /// reaches here too and is inert, since nothing but a click on the bar ever sets `drag`.
    fn handle_release<H: AppLike>(&mut self, now: u32, fx: &mut Effects<'_, H>) {
        if !self.scrub.drag {
            return;
        }
        self.scrub.drag = false;
        if self.scrub.ns >= 0 {
            plx_base::eventlog::log(&format!("scrub: pointer commit ns={}", self.scrub.ns));
            self.commit_scrub(false, fx);
        }
        self.scrub.disengage();
        self.hud.extend_default(now);
        self.publish();
    }

    /// The adapter publishes an exact OTT source for this native input. The
    /// ordinary input event remains the sole dispatcher path; no second key
    /// handler, timer or transport owner is installed.
    #[allow(clippy::too_many_arguments)]
    fn handle_ott_key<H: AppLike>(
        &mut self,
        ps: &plx_media::route::PlaybackSession,
        offer: Option<crate::registry::OttPlayerOffer>,
        focused: Option<u32>,
        mode: plx_machine::machine::UiMode,
        live_channel: bool,
        canonical: plx_machine::machine::Key,
        sym: u32,
        wcode: u32,
        edge: Edge,
        at_edge: bool,
        fx: &mut Effects<'_, H>,
    ) -> Option<Handled> {
        use crate::registry::{OttPlayerAction as Action, OttPlayerRequest};
        use consts::{Key, RemoteButton};
        let offer = offer?;
        let key = consts::classify_input(canonical, sym, wcode);
        let direction_or_ok = matches!(key, Key::Up | Key::Down | Key::Left { alt: false }
            | Key::Right { alt: false } | Key::Ok);
        let remote = (key == Key::Other).then(|| consts::remote_button(sym, wcode)).flatten();
        let channel_step = matches!(remote, Some(RemoteButton::ChannelUp | RemoteButton::ChannelDown));
        if offer.source.pending() && !channel_step {
            // Pending authority is a step-only capability. Preserve the
            // ordinary cancel/back and volume paths, never lend it to a menu,
            // list, seek or pause command.
            return if direction_or_ok || remote.is_some() { Some(Handled::Yes) } else { None };
        }
        let recovery_channel = offer.recovery_only && channel_step;
        let recovery_browse = offer.recovery_only && key == Key::Ok && offer.accept == Action::OpenCatalog;
        if offer.recovery_only && !recovery_channel && !recovery_browse { return None; }
        if !offer.recovery_only && !offer.source.playback().is_some_and(|source| plx_media::route::playback_identity_matches(ps, source)) {
            // A captured OTT command is not permission to control a replacement
            // source through the generic fallback. No offer is different: that
            // remains an ordinary player or desktop event.
            return (direction_or_ok || matches!(remote, Some(RemoteButton::ChannelUp
                | RemoteButton::ChannelDown | RemoteButton::Red
                | RemoteButton::Yellow | RemoteButton::Blue))).then_some(Handled::Yes);
        }
        // A generic arrow may only have revealed an unmoved preview. Blue still
        // opens categories; accepted panel ownership cancels that preview through
        // set_hud_policy. A moved target, hold, drag or transport tap keeps its owner.
        let reveal_only_categories = matches!(remote, Some(RemoteButton::Blue))
            && self.scrub.reveal && !self.scrub.key.engaged && !self.scrub.drag
            && !self.transport_tap;
        if self.hud_policy != HudPolicy::Normal
            || (self.scrub.is_live() && !reveal_only_categories)
        { return None; }
        // This is the PRE-press sample. note_global_press has already cleared
        // dismissal by this point and must not turn a bare press into HUD input.
        // UP expands live programme information in either presentation. CH+/CH−
        // retain channel stepping; finite Local/Plex/portal arrows keep their
        // source-owned sequence behavior. Classic RIGHT retains its info cycle.
        let info_cycle = live_channel && !offer.seek_arrows
            && (key == Key::Up || (mode == plx_machine::machine::UiMode::Classic
                && key == (Key::Right { alt: false })));
        // On a live channel, the center/OK button is the channel-list command. Keep
        // that action available while the transport HUD is visible too: otherwise
        // the first press only reveals the footer and every later press is consumed
        // by its default focus, leaving no remote path from fullscreen to channels.
        let open_catalog_on_ok = live_catalog_shortcut_on_ok(
            key,
            live_channel,
            offer.accept,
            self.hud.visible_at_press,
            actionable_hud_target_focused(self.slot, focused),
        );
        if direction_or_ok && self.hud.visible_at_press && !info_cycle && !open_catalog_on_ok && !recovery_browse
            && !plx_appkit::player_hud::transport_hidden(ps) {
            return None;
        }
        let action = match key {
            Key::Up if info_cycle => if self.hud.visible_at_press { Action::ShowDescription }
                else { Action::ShowInfo },
            Key::Up => Action::Next,
            Key::Down => Action::Previous,
            Key::Left { alt: false } => if offer.seek_arrows { Action::SeekBackward } else { Action::OpenMenu },
            Key::Right { alt: false } => if offer.seek_arrows { Action::SeekForward }
                else if info_cycle && self.hud.visible_at_press { Action::ShowDescription }
                else { Action::ShowInfo },
            Key::Ok => offer.accept,
            Key::Other => match remote? {
                RemoteButton::ChannelUp => Action::Next,
                RemoteButton::ChannelDown => Action::Previous,
                RemoteButton::Red if offer.guide => Action::OpenGuide,
                RemoteButton::Red => return Some(Handled::Yes),
                RemoteButton::Yellow => Action::OpenMenu,
                RemoteButton::Blue => Action::OpenCategories,
                // No native recordings backend or verified physical Info code.
                RemoteButton::Green => return None,
            },
            _ => return None,
        };
        // A propagated edge or release must not perform the bare action again.
        // Navigation/seek repeat uses the existing dispatcher cadence. OK keeps
        // its existing native one-action-per-press safety; unlike legacy raw
        // keydown repeats, a held OK never toggles repeatedly or arms a hidden control.
        if plx_appkit::player_hud::transport_hidden(ps)
            && matches!(action, Action::SeekBackward | Action::SeekForward) {
            return Some(Handled::Yes);
        }
        if at_edge || edge == Edge::Up || (edge == Edge::Repeat && (key == Key::Ok || info_cycle)) {
            return Some(Handled::Yes);
        }
        let plx_machine::machine::MachineId::Instance(owner) = fx.from() else {
            return Some(Handled::Yes);
        };
        if !matches!(action, Action::SeekBackward | Action::SeekForward) {
            self.end_jump_gesture();
        }
        Self::ask(fx, PlayerReq::OttAction(OttPlayerRequest { owner, source: offer.source, action }));
        Some(Handled::Yes)
    }

    /// Classify → dispatch, the one entry every `Input(Key)` event goes through.
    ///
    /// `at_edge` marks the dispatcher's RE-DELIVERY of a direction the engine could not move (an
    /// `EdgeRule::Screen` edge, §7.3 step 4): the screen's answer is [`Self::answer_edge`], and an
    /// edge is never asked twice. `focused` is the element the ENGINE holds focus on, which is what
    /// decides OK ([`Self::key_ok`]) and which group an edge belongs to. `command` is the command of
    /// the control `focused` names ([`Self::focused_command`]), resolved by the caller: the burst
    /// guard below turns on it, and no slot holds a seek control until a package can place one.
    fn handle_key<H: AppLike>(
        &mut self,
        ps: &plx_media::route::PlaybackSession,
        key: consts::Key,
        edge: Edge,
        at_edge: bool,
        now: u32,
        focused: Option<u32>,
        command: Option<player_hud::command::HudCommand>,
        fx: &mut Effects<'_, H>,
        meta: plx_data::metadata::MetadataView<'_>,
    ) -> Handled {
        use consts::Key;
        if edge == Edge::Down && !matches!(key, Key::Left { .. } | Key::Right { .. }) {
            // A press of a seek CONTROL belongs to the jump gesture in progress, so two of them
            // inside one burst give one target (`seek_jump::SeekBurst`).
            if !continues_seek_burst(key, command) {
                self.seek_burst.reset();
            }
            self.drop_transport_tap();
        }
        // A dead source has no transport, but its short notice owns no focus.
        // Source-bound OTT browse/channel requests were handled above. Only an
        // explicit ordinary Back/Stop leaves a generic failed playback.
        if plx_appkit::player_hud::transport_hidden(ps) && !matches!(key, Key::Exit) {
            if edge == Edge::Down && matches!(key, Key::Back | Key::Stop) {
                Self::ask(fx, PlayerReq::Exit);
            }
            return Handled::Yes;
        }
        if self.jump_key(ps, key, edge, now, fx) {
            return Handled::Yes;
        }
        if self.hud.visible_at_press && self.hud.nav.focus == GROUP_ROW
            && matches!(key, Key::Left { alt: false } | Key::Right { alt: false }) {
            self.hud.extend_default(now);
            return if edge == Edge::Up { Handled::Yes } else { Handled::No };
        }
        match key {
            Key::Play | Key::Pause | Key::PlayPause => {
                if edge == Edge::Down {
                    let play = match key {
                        Key::Play => Some(true),
                        Key::Pause => Some(false),
                        _ => None,
                    };
                    let req = self.toggle_play(play, now);
                    Self::ask(fx, req);
                }
                Handled::Yes
            }
            // The STOP key and a plain BACK (nothing else open — an overlay's own BACK is answered
            // by `PlayerOverlayScreen::key` before this screen ever sees the press, since the
            // overlay is a SURFACE and owns input while it is up) both perform the same ritual
            // `key_back`/the Stop arm did.
            //
            // …except a BACK that finds the transport ON SCREEN: that one hides it, the way an
            // UP-hide does, and the film keeps playing. A second BACK, now over a bare picture,
            // leaves. `visible_at_press` and not `hud_up`, for the reason its own doc gives.
            //
            // The two presses apply whether or not the hide can take effect. While the pipeline is
            // busy (`loading`) the transport is ALWAYS on screen — `hud_visible` ORs it in, so the
            // hide is latched but invisible, and every later BACK would find the transport up and
            // hide it again: a first load that stalls with no picture and no error could never be
            // left. So a BACK that hid the transport while busy ARMS the exit
            // (`HudState::exit_armed`), and the NEXT BACK leaves even though the transport stayed
            // up. Leaving on the FIRST busy BACK instead would end the film on one press to clear
            // the transport after a seek, an audio retranscode hold or an HLS rebuffer. Only a hide
            // done WHILE busy arms it, and any other bound press spends it
            // (`HudState::note_fresh_press`); so does the pipeline settling with nothing pressed
            // (`HudState::settle`, on the Tick). A rebuffer that pops the transport up after the
            // viewer hid it in normal play, or long after a busy hide, still takes two presses.
            Key::Back if edge == Edge::Down && self.scrub.is_live() => {
                self.key_back_scrub(now);
                Handled::Yes
            }
            Key::Back if edge == Edge::Down && self.hud.back_leaves_at_press => {
                Self::ask(fx, PlayerReq::Exit);
                Handled::Yes
            }
            Key::Back if edge == Edge::Down && self.hud.visible_at_press => {
                self.hud.nav.focus = GROUP_SCRUB;
                Self::repair_focus(fx, GROUP_SCRUB);
                self.scrub.ns = -1;
                self.scrub.disengage();
                self.hud.hide(plx_media::player::loading(ps));
                self.publish();
                Handled::Yes
            }
            Key::Stop | Key::Back => {
                if edge == Edge::Down {
                    Self::ask(fx, PlayerReq::Exit);
                }
                Handled::Yes
            }
            // **UP and DOWN travel through the FocusEngine** (`Screen::links`, the groups' edge
            // rules). A fresh press on a transport that is on screen is left unhandled for it: the
            // landing arrives as `FocusMoved` and `step` carries the travel's side effects. A
            // repeat or release is swallowed, as a held UP never walked the ring. A transport
            // that is hidden spends the press on being raised, and moves nothing.
            Key::Up | Key::Down => {
                if at_edge {
                    return self.answer_edge(key, focused, now, fx);
                }
                if edge != Edge::Down {
                    return Handled::Yes;
                }
                if !self.hud.visible_at_press {
                    self.hud.nav.focus = GROUP_SCRUB;
                    Self::repair_focus(fx, GROUP_SCRUB);
                    self.hud.extend_default(now);
                    return Handled::Yes;
                }
                Handled::No
            }
            Key::Left { .. } | Key::Right { .. } => {
                match edge {
                    Edge::Down => {
                        // A press that is not the held key coming back settles the hold's
                        // pending release first, as the `Up` itself used to.
                        let fwd = matches!(key, consts::Key::Right { .. });
                        if self.scrub.key.release_pending() && !self.scrub.key.continues(fwd, now) {
                            self.end_hold("a different press settled the release");
                        }
                        self.key_scrub_fresh(ps, key, now, fx, meta)
                    }
                    Edge::Repeat => self.key_scrub_repeat(now),
                    Edge::Up => self.key_scrub_release(now),
                }
                Handled::Yes
            }
            Key::Ok => match edge {
                Edge::Down => self.key_ok(now, focused, fx),
                // …and the pointer button's own release, which the loop spells as this edge.
                Edge::Up => {
                    self.handle_release(now, fx);
                    Handled::Yes
                }
                Edge::Repeat => Handled::Yes,
            },
            _ => Handled::No,
        }
    }

    /// **OK, off a panel** — a pending target is taken first; a hidden transport asks for the
    /// plain play/pause toggle. Otherwise the ENGINE's focus decides what was pressed: a control
    /// row element is left to the engine (`Handled::No`), whose press machinery dips the control
    /// and delivers `PressCommit` on the spring-back ([`Self::row_pressed`] performs it), a tab
    /// opens its panel (Info for the first, Chapters for the second), and everything else, the
    /// scrubber included, is the toggle — exactly as the old ladder's `else` arm. The scrubber and
    /// the tabs stay `Handled::Yes`: the scrubber's OK means the toggle or a target, which a bare
    /// press cannot carry, and a tab opens at once with no dip to wait for.
    fn key_ok<H: AppLike>(&mut self, now: u32, focused: Option<u32>, fx: &mut Effects<'_, H>) -> Handled {
        let vis = self.hud.visible_at_press;
        let focused = focused.filter(|_| vis);
        let mut handled = Handled::Yes;
        if self.take_target(now, fx) {
            // OK confirmed the target and the film plays on from it; that is all this press means
        } else if focused.and_then(player_hud::row_index).is_some() {
            // the press takes the page from the scrubber, so a pending target goes with it
            self.scrub.cancel();
            handled = Handled::No;
        } else if let Some(tab) = focused
            .filter(|e| Self::hud_group_of(*e) == Some(GROUP_TABS))
            .map(|e| e - player_hud::ELEM_TAB_BASE)
        {
            let kind = match tab {
                0 => Some(overlay::OverlayKind::Info),
                1 => Some(overlay::OverlayKind::Chapters),
                _ => None,
            };
            if let Some(kind) = kind {
                Self::ask(fx, PlayerReq::OpenOverlay(kind));
            }
        } else {
            let req = self.toggle_play(None, now);
            Self::ask(fx, req);
        }
        self.hud.extend_default(now);
        self.publish();
        handled
    }

    /// **The play/pause toggle, the one function the Play/Pause/PlayPause keys, OK on the picture
    /// and the `toggle_play` command share.** A key that confirms (PLAY, PLAYPAUSE and OK; never
    /// PAUSE, which only pauses) takes a pending scrub target first ([`Self::take_target`]) and is
    /// a [`PlayerReq::SeekTo`]; otherwise it is the [`PlayerReq::Transport`] toggle, with `play`
    /// naming a one-way key. The caller performs the request it returns, as a screen does through
    /// `fx`, or as the app's performer does directly.
    pub fn toggle_play(&mut self, play: Option<bool>, now: u32) -> PlayerReq {
        if play != Some(false) {
            if let Some(target) = self.take_target_ns(now) {
                return PlayerReq::SeekTo(target);
            }
        }
        PlayerReq::Transport(play)
    }

    /// The command of the control the engine holds, if it holds one of the row's.
    fn focused_command(&self, focused: Option<u32>) -> Option<player_hud::command::HudCommand> {
        self.slot.command(focused.and_then(player_hud::row_index)?)
    }

    /// **A press on the row committed** (the engine's `PressCommit`, once the dip has played): ask
    /// the app to perform what the control the engine holds means. A disc is its command
    /// ([`PlayerReq::Command`]); a stand-in's meaning is the app's, so it is [`PlayerReq::StandIn`].
    /// Nothing happens when the engine is not on an enabled control of the row any more (the row
    /// changed under the press).
    fn row_pressed<H: AppLike>(&mut self, focused: Option<u32>, fx: &mut Effects<'_, H>) {
        let Some(ordinal) = focused.and_then(player_hud::row_index).filter(|&i| self.slot.enabled(i)) else {
            return;
        };
        self.hud.nav.btn = ordinal;
        match self.slot.command(ordinal) {
            Some(command) => Self::ask(fx, PlayerReq::Command(command)),
            None => Self::ask(fx, PlayerReq::StandIn),
        }
    }

    /// The control row's deferred press (`PlayerReq::ArmControlRow`). It takes the page from the
    /// scrubber, so a pending target is cancelled with it.
    fn arm_control_row<H: AppLike>(&mut self, fx: &mut Effects<'_, H>) {
        self.scrub.cancel();
        Self::ask(fx, PlayerReq::ArmControlRow);
    }

    /// **The screen's answer to `Step::Edge`**: the dispatcher re-delivers a direction the engine
    /// could not move, from a group whose edge rule is `EdgeRule::Screen`, as the same key with
    /// `at_edge` set. `focused` names the group it left.
    ///
    /// * UP off the top of the control row HIDES the transport (and the film plays on): the
    ///   scrubber is parked for the next reveal, the pending target dropped, and NOT re-armed — a
    ///   hide that extended its own linger would not hide.
    /// * DOWN off the bottom of the tabs has nowhere to go and stays: the target is dropped and the
    ///   press re-arms the linger like any other.
    ///
    /// Anything else swallows the press; an edge nobody answers would be dropped as a dead key.
    fn answer_edge<H: AppLike>(
        &mut self,
        key: consts::Key,
        focused: Option<u32>,
        now: u32,
        fx: &mut Effects<'_, H>,
    ) -> Handled {
        match (focused.and_then(Self::hud_group_of), key) {
            (Some(GROUP_ROW), consts::Key::Up) => {
                self.hud.nav.focus = GROUP_SCRUB;
                self.scrub.ns = -1;
                self.scrub.disengage();
                self.hud.hide(false);
                Self::repair_focus(fx, GROUP_SCRUB);
            }
            (Some(GROUP_TABS), consts::Key::Down) => {
                self.scrub.ns = -1;
                self.scrub.disengage();
                self.hud.extend_default(now);
            }
            _ => {}
        }
        Handled::Yes
    }

    /// A key hold is over — its release survived the bounce window, or a press that is not the
    /// held key coming back settled it, or the keyup never came (the lost-keyup net): end the KEY
    /// gesture and leave the target pending. **Nothing is committed here**; OK is the only thing
    /// that takes a key target.
    fn end_hold(&mut self, why: &str) {
        plx_base::eventlog::log(&format!(
            "scrub: hold over ({why}), target {}s pending",
            self.scrub.ns / 1_000_000_000
        ));
        self.scrub.disengage();
    }

    /// While a key target is pending the transport is held up: the 4.5 s linger would otherwise
    /// hide it, and with it the only sign that there is a target to confirm or drop. A transport
    /// that was dismissed anyway (by whichever path) takes the target with it — cancelled, never
    /// committed. A reveal (a press spent raising the transport, `Scrub::reveal`) has no target.
    fn hold_the_transport_for_a_target(&mut self, now: u32) {
        if !self.scrub.is_live() || self.scrub.reveal {
            return;
        }
        if self.hud.dismissed {
            self.scrub.cancel();
        } else {
            self.hud.extend_default(now);
        }
    }

    /// **OK or PLAY on a pending key target — the confirmation**: seek there and leave the film
    /// playing ([`PlayerReq::SeekTo`], which resumes a paused film and is only a seek on a playing
    /// one). `false` when there is none, and the press means what it always did. A pointer drag in
    /// progress is not a key target (its release is its own confirmation), and neither is a
    /// reveal's seed.
    fn take_target<H: AppLike>(&mut self, now: u32, fx: &mut Effects<'_, H>) -> bool {
        match self.take_target_ns(now) {
            Some(target) => {
                Self::ask(fx, PlayerReq::SeekTo(target));
                true
            }
            None => false,
        }
    }

    /// [`Self::take_target`]'s state change, for a caller that performs the seek itself: the
    /// target taken (and the gesture retired), or `None` when there is none to take.
    fn take_target_ns(&mut self, now: u32) -> Option<i64> {
        if self.scrub.ns < 0 || self.scrub.drag || self.scrub.reveal {
            return None;
        }
        plx_base::eventlog::log(&format!(
            "scrub: target taken {}s",
            self.scrub.ns / 1_000_000_000
        ));
        let target = self.scrub.ns;
        self.scrub.ns = -1;
        self.scrub.disengage(); // a key may still be down: the gesture ends with its target
        self.hud.extend_default(now);
        self.publish();
        Some(target)
    }

    /// **BACK while a scrub is live**: the scrub is cancelled — no seek, the preview gone, the knob
    /// springs home (`knob_spring::KnobSpring`) — a pending key target is dropped the same way.
    fn key_back_scrub(&mut self, now: u32) {
        plx_base::eventlog::log("scrub: BACK cancels the scrub");
        self.scrub.cancel();
        self.hud.extend_default(now);
        self.publish();
    }

    /// A FRESH LEFT/RIGHT press: port of `key_scrub`'s `ScrubPress` dispatch. It commits NOTHING,
    /// and neither does its release (`key_scrub_release`): the press moves the target, OK takes it.
    fn key_scrub_fresh<H: AppLike>(
        &mut self,
        ps: &plx_media::route::PlaybackSession,
        key: consts::Key,
        now: u32,
        fx: &mut Effects<'_, H>,
        meta: plx_data::metadata::MetadataView<'_>,
    ) {
        let fwd = matches!(key, consts::Key::Right { .. });
        let dur = plx_media::player::duration_ns();
        // **A key-up then a key-down inside the bounce window is one hold, not two gestures.**
        // The key driver was measured doing exactly that mid-hold, and the old code committed the
        // seek on the `Up` and began again on the `Down`. Now the pending release is cancelled and
        // the hold simply goes on: no hop, no commit, the ramp keeps its speed — and the
        // lost-keyup net is told the remote's repeat delay has restarted.
        if self.scrub.key.bounce(now, fwd) {
            self.hud.extend_default(now);
            plx_base::eventlog::log("scrub: key bounce merged (hold continues)");
            return;
        }
        // A key gesture SUPERSEDES a pointer one, and throws its preview away rather than hopping
        // from it — `key_scrub`'s own first act, when the flag was `Pointer::drag`. The pointer's
        // release is never going to arrive as far as this gesture is concerned, so a preview left
        // standing would be taken later by an OK that knows nothing about where it came from.
        if self.scrub.drag {
            self.scrub.drag = false;
            self.scrub.ns = -1;
        }
        let act = input::scrub_press(self.hud.visible_at_press, self.hud.nav.focus, dur > 0);
        self.hud.extend_default(now);
        match act {
            input::ScrubPress::Reveal => {
                self.hud.nav.focus = GROUP_SCRUB;
                Self::repair_focus(fx, GROUP_SCRUB);
                if dur > 0 {
                    self.scrub.begin(now, fwd);
                    self.scrub.reveal = true;
                    self.scrub.ns = plx_media::player::intended_pos_ns(ps);
                }
            }
            // Horizontal row travel is delegated to FocusEngine in handle_key.
            input::ScrubPress::Row => {}
            input::ScrubPress::Tabs => {
                let max_tab = if plx_appkit::chapters_panel::has_chapters(meta) { 1 } else { 0 };
                self.hud.nav.tab =
                    (self.hud.nav.tab + if fwd { 1 } else { -1 }).clamp(0, max_tab);
                // the tab pill the engine holds follows, or OK would open the one just left
                Self::repair_focus(fx, GROUP_TABS);
            }
            input::ScrubPress::Jump => {
                // scrubber focus, FRESH press: the Skip interval hop of the TARGET, and NOTHING
                // committed. A held key's repeats engage the continuous ramp below; neither its
                // release nor a tap's seeks — the target waits for OK, so a burst of taps is
                // simply a target that moved several times. The commit belonging to the press was
                // this package's first regression.
                self.scrub.reveal = false; // a visible press is a real gesture whatever raised the HUD
                if self.scrub.key.dir == 0 && self.scrub.ns < 0 {
                    self.scrub.ns = plx_media::player::intended_pos_ns(ps);
                }
                if !self.scrub.key.engaged {
                    let step = if fwd { input::scrub_step_ns() } else { -input::scrub_step_ns() };
                    self.scrub.ns = Scrub::clamp_target(self.scrub.ns.max(0) + step, dur);
                }
                self.scrub.key.press(now, fwd);
            }
            input::ScrubPress::Nothing => {}
        }
    }

    /// A hardware auto-repeat while the scrubber holds a direction: port of `on_auto_repeat`'s
    /// engage-only half — the continuous ramp's per-frame advance is `step_scrub_hold`, driven by
    /// `Tick` rather than by the repeat event itself (a held key's repeats and the frame rate are
    /// two different clocks).
    fn key_scrub_repeat(&mut self, now: u32) {
        if self.hud.nav.focus == GROUP_SCRUB && self.scrub.key.dir != 0 {
            if self.scrub.key.repeat(now) {
                // `reveal` is deliberately NOT cleared here. Engaging the hold is not the same
                // event as the preview MOVING: `t` is stamped now, so the advance's first pass
                // computes `sdt ≈ 0` and travels nothing, and at `SCRUB_BASE` it takes
                // `1/SCRUB_BASE` of a real second before the preview has moved even a second.
                // A firm tap that trips one hardware repeat and releases inside that window would otherwise leave a target on the spot
                // playback is already sitting on, for an OK to take with a full reopen + prime and
                // a visible stall, out of a press the reveal rule promises moves nothing.
                // `step_scrub_hold` clears it once there is real travel.
                plx_base::eventlog::log("scrub: hold engaged (0x101 repeat)");
            }
        }
    }

    /// **The release**, port of `on_key_up`'s scrub half — the arm `app/input.rs` kept running on
    /// this very field after phase 9 had given the gesture to this screen.
    ///
    /// **No outcome seeks.** A reveal-only gesture throws the preview away: the press was spent
    /// raising the transport and the preview sits on the seed, so it is not a target worth
    /// confirming. This is tested BEFORE the key release (`HeldKey::release`), because a hold that
    /// engaged but has not TRAVELLED yet is still that case — the ramp is what retires `reveal`, on
    /// real travel. A travelled hold's release is deferred inside `HeldKey` (the driver bounces
    /// keys mid-hold) and `step_scrub_hold` ends the hold once the window has passed without a
    /// re-press. A plain tap simply ends its key gesture. In both the target stays pending for OK
    /// or BACK, and a later tap or hold goes on from it.
    fn key_scrub_release(&mut self, now: u32) {
        if self.scrub.key.dir == 0 {
            return;
        }
        if self.scrub.reveal {
            self.scrub.ns = -1;
            self.scrub.disengage();
        } else if !self.scrub.key.release(now) {
            self.scrub.key.disengage();
        }
    }

    /// Ask the loop to perform the scrub, and retire the target that asked for it. The two
    /// confirmations share this path and differ only in the request:
    ///
    /// * `resume` — OK / PLAY taking a key target ([`Self::take_target`]): [`PlayerReq::SeekTo`],
    ///   which seeks and leaves the film PLAYING from there (it resumes a paused one).
    /// * otherwise — a pointer drag's release ([`Self::handle_release`]): [`PlayerReq::CommitSeek`],
    ///   which seeks and holds a paused film paused (the pump opens the seek preroll when it
    ///   dispatches the seek under a viewer Pause and re-pauses on the landing,
    ///   `plx_media::player::pump::settle_paused_seek`; the loop's arm for it is a bare
    ///   `request_seek`).
    ///
    /// Clearing `ns` here rather than in the loop is the §2.3 half of the same change —
    /// `app::playback::commit_seek` used to reach into this gesture and blank it, which is a write
    /// to another owner's state.
    ///
    /// A commit is final: nothing remembers where the scrub started.
    fn commit_scrub<H: AppLike>(&mut self, resume: bool, fx: &mut Effects<'_, H>) {
        if self.scrub.ns < 0 {
            return;
        }
        let target = self.scrub.ns;
        Self::ask(fx, if resume { PlayerReq::SeekTo(target) } else { PlayerReq::CommitSeek(target) });
        self.scrub.ns = -1;
    }

    /// The continuous accelerating scrub's per-frame advance while a KEY is held, and the
    /// lost-keyup safety net that bounds it — port of `app/run.rs`'s per-loop-iteration block,
    /// formula for formula (the net ends the hold and no longer commits: see [`Self::end_hold`]).
    ///
    /// Skipped while a pointer drag owns the preview: the hand says where it is, and an advance
    /// underneath it would fight the pointer. That `!dragging` term was the loop's too, read off
    /// `app::input::Pointer::drag`; the flag moved onto the gesture with the gesture.
    ///
    /// **The safety net is not optional.** This remote emits a held key as auto-repeat keydowns
    /// followed by ONE keyup, and a dropped keyup leaves the hold engaged with the preview slewing at
    /// up to `SCRUB_MAX` for as long as the page is up. Every repeat proves the key down, so
    /// `HeldKey::lost` (`SCRUB_LOST_MS` without one) is the release that never arrived.
    fn step_scrub_hold(&mut self, now: u32) {
        // The release that survived the bounce window is the real one.
        if self.scrub.key.release_due(now) {
            self.end_hold("keyup survived the bounce window");
            return;
        }
        if self.scrub.key.dir == 0
            || !self.scrub.key.engaged
            || self.scrub.ns < 0
            || self.scrub.drag
        {
            return;
        }
        let was = self.scrub.ns;
        let dur = plx_media::player::duration_ns();
        let s = Scrub::clamp_target(was + self.scrub.key.advance_ns(now), dur);
        self.scrub.ns = s;
        // Real travel is what turns a reveal into a scrub — not the hold edge, which fires a beat
        // earlier with nothing moved yet. Once the preview has left the seed it is a target like
        // any other held gesture's.
        if s != was {
            self.scrub.reveal = false;
        }
        self.hud.extend_default(now);
        if self.scrub.key.lost(now) {
            self.end_hold("lost keyup");
        }
    }
}

impl PlayerScreen {
    /// **Where the layout puts the HUD this frame**, as the `HudGeometry` the classic layout
    /// reports for the row's occupant and the tab count (Info alone, or Info and Chapters). It
    /// measures, like the draw it describes, so the callers that need several facts of one frame
    /// ([`Self::record_stops`]) ask once and read it through [`Self::groups_in`] and
    /// [`Self::place_in`]; the `Focusable` methods ask for their own.
    fn hud_geometry<H: PlayerLike + crate::registry::MetadataLike>(&self, cx: &Cx<'_, H>) -> HudGeometry {
        let tabs = if plx_appkit::chapters_panel::has_chapters(H::metadata(cx)) { 2 } else { 1 };
        player_hud::classic::geometry_in_mode(self.slot, &self.row, tabs, cx.measure, H::ui_mode(&cx.views),
            (self.hud.nav.focus == GROUP_ROW).then_some(self.hud.nav.btn),
            H::metadata(cx).now_playing(), self.model.wall_now_s)
    }

    /// Classic advertises only geometry it paints. Read the layout, including
    /// contextual Skip/Up Next, rather than maintaining a second mode whitelist.
    fn hidden_classic_control<H: PlayerLike + crate::registry::MetadataLike>(&self, cx: &Cx<'_, H>, elem: u32) -> bool {
        if H::ui_mode(&cx.views) != plx_machine::machine::UiMode::Classic { return false; }
        match Self::hud_group_of(elem) {
            Some(GROUP_ROW) => {
                let index = player_hud::row_index(elem).unwrap();
                !self.slot.enabled(index) || self.hud_geometry(cx).items.get(index as usize).copied().flatten().is_none()
            }
            Some(GROUP_TABS) => self.hud_geometry(cx).tabs
                .get((elem - player_hud::ELEM_TAB_BASE) as usize).copied().flatten().is_none(),
            _ => false,
        }
    }

    /// Which of the HUD's three transport groups holds `elem` — the failure row and the repair
    /// card are not the transport's, and answer `None`.
    fn hud_group_of(elem: u32) -> Option<GroupId> {
        use player_hud::{ELEM_FAILURE_BASE, ELEM_SCRUB, ELEM_TAB_BASE};
        match elem {
            e if e == ELEM_SCRUB => Some(GROUP_SCRUB),
            e if player_hud::row_index(e).is_some() => Some(GROUP_ROW),
            e if (ELEM_TAB_BASE..ELEM_FAILURE_BASE).contains(&e) => Some(GROUP_TABS),
            _ => None,
        }
    }

    fn groups_in<H: PlayerLike + crate::registry::MetadataLike>(
        &self,
        geo: &HudGeometry,
        cx: &Cx<'_, H>,
        out: &mut Vec<GroupSpec>,
    ) {
        if self.repair_alert.visible() {
            let len = match self.repair_alert.answers() { plx_ui::decision_alert::Answers::One => 1, _ => 2 };
            out.push(GroupSpec { id: GROUP_REPAIR, kind: GroupKind::Row { wrap: false }, seat: Seat::First,
                reachable: AxisMask::BOTH, edge: [EdgeRule::Stop; 4], extent: self.repair_rect(false, cx.measure).union(self.repair_rect(true, cx.measure)), len, elem: plx_ui::screen::ElemKind::Control });
            return;
        }
        out.push(GroupSpec {
            id: GROUP_SCRUB,
            kind: GroupKind::Free,
            seat: Seat::First,
            reachable: AxisMask::BOTH,
            edge: [EdgeRule::Screen; 4],
            extent: geo.grab,
            len: 1,
            elem: plx_ui::screen::ElemKind::Control,
        });
        // Every item of whatever occupies the row — the Up Next PAIR included, which is two
        // controls the D-pad walks and so two stops (`ControlSlot::item_rect`).
        let row_len = self.slot.items().max(1) as usize;
        let classic = H::ui_mode(&cx.views) == plx_machine::machine::UiMode::Classic;
        if !classic || geo.items.iter().any(Option::is_some) { out.push(GroupSpec {
            id: GROUP_ROW,
            kind: GroupKind::Row { wrap: false },
            seat: Seat::First,
            reachable: AxisMask::BOTH,
            // Up off the row's top is the screen's to answer (`answer_edge`: it hides the HUD);
            // Down is a declared link back to the scrubber (`links`), and the row's ends stay put.
            edge: [EdgeRule::Screen, EdgeRule::Stop, EdgeRule::Stop, EdgeRule::Stop],
            extent: geo.row,
            len: row_len,
            elem: plx_ui::screen::ElemKind::Control,
        }); }
        let has_ch = plx_appkit::chapters_panel::has_chapters(H::metadata(cx));
        if !classic || geo.tabs.iter().any(Option::is_some) { out.push(GroupSpec {
            id: GROUP_TABS,
            kind: GroupKind::Row { wrap: false },
            seat: Seat::First,
            reachable: AxisMask::BOTH,
            edge: [EdgeRule::Screen; 4],
            extent: geo.tabs[0].unwrap_or(Rect::FULL),
            len: if has_ch { 2 } else { 1 },
            elem: plx_ui::screen::ElemKind::Control,
        }); }

    }

    /// Where `key` sits, read off the frame's [`HudGeometry`] and so measuring nothing itself: the
    /// scrubber's grab band, the row's item `i` at the rect it is DRAWN at (a disabled or absent
    /// disc has none), a tab's pill, or the failure read-out's control.
    fn place_in<H: PlayerLike + crate::registry::MetadataLike>(
        &self,
        geo: &HudGeometry,
        key: &u32,
        cx: &Cx<'_, H>,
    ) -> Option<Placed> {
        use player_hud::{ELEM_FAILURE_BASE, ELEM_FAILURE_END, ELEM_SCRUB, ELEM_TAB_BASE};
        if self.repair_alert.visible() {
            if !matches!(*key, REPAIR_CANCEL | REPAIR_CONFIRM) { return None; }
            let rect = self.repair_rect(*key == REPAIR_CONFIRM, cx.measure);
            return Some(Placed { rect, rest_rect: rect, clip: Rect::FULL, index: None });
        }
        let rect = match *key {
            e if e == ELEM_SCRUB => geo.grab,
            e if let Some(index) = player_hud::row_index(e) => {
                if !self.slot.enabled(index) { return None; }
                geo.items.get(index as usize).copied().flatten()?
            }
            e if (ELEM_TAB_BASE..ELEM_FAILURE_BASE).contains(&e) => {
                geo.tabs.get((e - ELEM_TAB_BASE) as usize).copied().flatten()?
            }
            e if (ELEM_FAILURE_BASE..ELEM_FAILURE_END).contains(&e) => return None,
            _ => return None,
        };
        let clip = if H::ui_mode(&cx.views) == plx_machine::machine::UiMode::Modern
            && matches!(self.slot, ControlSlot::Discs(_)) && player_hud::row_index(*key).is_some() {
            geo.row
        } else { Rect::FULL };
        Some(Placed { rect, rest_rect: rect, clip, index: None })
    }
}

/// **The four registered regions**: scrubber, control row, bottom tabs and failure actions.
/// The established player key policy chooses vertical HUD transitions and dismissal. Horizontal
/// control-row travel belongs to FocusEngine: neighbour uses the shared enabled-index helper,
/// absent/disabled discs have no placement, and seat chooses the row's enabled primary.
///
/// **The order `groups` pushes in is the pointer's z-order**, lowest first: `record_stops`
/// registers in it, and the hit map resolves the last stop under the pointer. The scrubber's tall
/// grab band therefore comes FIRST, under the control row and tabs it overlaps (issue #162).
impl<H: PlayerLike + crate::registry::MetadataLike> Focusable<H> for PlayerScreen {
    fn groups(&self, cx: &Cx<'_, H>, out: &mut Vec<GroupSpec>) {
        let geo = self.hud_geometry(cx);
        self.groups_in(&geo, cx, out);
    }
    fn group_of(&self, key: &u32, cx: &Cx<'_, H>) -> Option<GroupId> {
        use player_hud::{ELEM_FAILURE_BASE, ELEM_FAILURE_END};
        if self.repair_alert.visible() {
            return matches!(*key, REPAIR_CANCEL | REPAIR_CONFIRM).then_some(GROUP_REPAIR);
        }
        if self.hidden_classic_control(cx, *key) { return None; }
        if (ELEM_FAILURE_BASE..ELEM_FAILURE_END).contains(key) { return None; }
        Self::hud_group_of(*key)
    }
    fn neighbour(&self, key: FocusKey<u32>, dir: Dir, cx: &Cx<'_, H>) -> Step<u32> {
        if self.repair_alert.is_open() {
            let two = self.repair_alert.answers() == plx_ui::decision_alert::Answers::Two;
            let elem = match (key.elem, dir) {
                (REPAIR_CANCEL, Dir::Right) if two => REPAIR_CONFIRM,
                (REPAIR_CONFIRM, Dir::Left) => REPAIR_CANCEL,
                _ => return Step::Edge,
            };
            return Step::Move(FocusKey { entry: self.entry, elem });
        }
        if self.hidden_classic_control(cx, key.elem) { return Step::Edge; }
        if let Some(index) = player_hud::row_index(key.elem) {
            let forward = match dir { Dir::Left => false, Dir::Right => true, _ => return Step::Edge };
            if let Some(next) = plx_ui::focus::next_enabled_index(index as usize, self.slot.items() as usize, forward,
                |i| self.slot.enabled(i as i32) && !self.hidden_classic_control(cx, player_hud::control_elem(i as i32))) {
                return Step::Move(FocusKey { entry: self.entry, elem: player_hud::control_elem(next as i32) });
            }
        }
        Step::Edge
    }
    fn place(&self, key: &u32, cx: &Cx<'_, H>, _at: At) -> Option<Placed> {
        let geo = self.hud_geometry(cx);
        self.place_in(&geo, key, cx)
    }
    fn reconcile(&self, want: FocusKey<u32>, cx: &Cx<'_, H>) -> FocusKey<u32> {
        if self.hidden_classic_control(cx, want.elem) {
            return FocusKey { entry: self.entry, elem: player_hud::ELEM_SCRUB };
        }
        if player_hud::row_index(want.elem).is_some_and(|i| !self.slot.enabled(i)) {
            FocusKey { entry: self.entry, elem: player_hud::control_elem(self.slot.primary_btn()) }
        } else { want }
    }
    fn seat(&self, g: GroupId, _from: Placed, cx: &Cx<'_, H>) -> FocusKey<u32> {
        let elem = match g {
            GROUP_SCRUB => player_hud::ELEM_SCRUB,
            GROUP_ROW => player_hud::control_elem(self.slot.primary_btn()),
            // the tab the user last stood on (Down from the scrubber returns to it), held to the
            // tabs this item has
            GROUP_TABS => {
                let last = if plx_appkit::chapters_panel::has_chapters(H::metadata(cx)) { 1 } else { 0 };
                player_hud::ELEM_TAB_BASE + self.hud.nav.tab.clamp(0, last) as u32
            }
            GROUP_FAILURE => player_hud::ELEM_FAILURE_BASE,
            GROUP_REPAIR => REPAIR_CANCEL,
            _ => player_hud::ELEM_SCRUB,
        };
        let elem = if self.hidden_classic_control(cx, elem) { player_hud::ELEM_SCRUB } else { elem };
        FocusKey { entry: self.entry, elem }
    }
}

impl LogicalState for PlayerScreen {
    fn write(&self, c: &mut Canon) {
        c.u32(self.hud.nav.focus.0 as u32)
            .u32(self.hud.nav.btn as u32)
            .u32(self.hud.nav.tab as u32)
            .u32(self.hud.until)
            .u32(u32::from(self.hud.info_timeout().seconds()))
            .bool(self.hud.dismissed)
            .bool(self.hud.visible_at_press)
            .bool(self.hud.exit_armed)
            .bool(self.hud.back_leaves_at_press)
            .bool(self.hud.was_standin);
        c.option(self.hud.last_offer.as_ref(), |c, (kind, at)| {
            c.u32(*kind as u32).u64(*at as u64);
        });
        c.u32(self.scrub.key.dir as u32)
            .bool(self.scrub.key.engaged)
            .bool(self.scrub.reveal)
            .bool(self.scrub.drag)
            .u64(self.scrub.ns as u64);
        c.option(self.seek_burst.snapshot().as_ref(), |c, (target, at)| {
            c.u64(*target as u64).u32(*at);
        });
        c.bool(self.transport_tap);
        c.option(self.origin.as_ref(), |c, o| {
            c.u32(o.entry.0);
        });
        c.bool(self.repair_alert.is_open()).bool(self.repair_alert.choice() == plx_ui::decision_alert::Choice::Destructive).u32(self.repair_scroll);
        c.bool(self.reminder_sheet).bool(self.reminder_focus);
        c.option(self.reminder_prompt.as_ref(), |c, token| { c.str(&format!("{token:?}")); });
        c.option(self.reminder_notice_until.as_ref(), |c, until| { c.u32(*until); });
        c.bool(self.reminder_notice.is_open());
        c.option(self.volume_notice_until.as_ref(), |c, until| { c.u32(*until); });
        c.bool(self.volume_notice.is_open()).bool(self.number_notice).bool(self.channel_change_hud);
    }
    fn probe(&self, out: &mut String) {
        out.push_str(WORD);
    }
}

impl<H: PlayerLike + crate::registry::MetadataLike> Screen<H> for PlayerScreen {
    fn name(&self) -> &'static str {
        WORD
    }
    fn state(&self) -> &dyn LogicalState {
        self
    }
    fn crumb(&self, _cx: &Cx<'_, H>) -> Option<Cow<'_, str>> {
        None
    }
    /// **The HUD's travel between its three regions**, as declared links (§7.3 step 3): the
    /// scrubber's Up is the control row and its Down the tabs; Down from the row and Up from the
    /// tabs come back to the scrubber. Where a link lands is the destination group's `seat`. What
    /// no link covers is the group's edge rule: Up off the row and Down off the tabs are
    /// `EdgeRule::Screen`, which the dispatcher re-delivers to [`PlayerScreen::step`] as the key
    /// with `at_edge` set — [`PlayerScreen::answer_edge`] is the screen's answer.
    fn links(&self, out: &mut Vec<Link>) {
        // FocusEngine follows a link only if its destination occurs in the
        // current groups. Classic omits hidden row/tabs in groups_in above.
        out.extend([
            Link { from: GROUP_SCRUB, dir: Dir::Up, to: GROUP_ROW },
            Link { from: GROUP_SCRUB, dir: Dir::Down, to: GROUP_TABS },
            Link { from: GROUP_ROW, dir: Dir::Down, to: GROUP_SCRUB },
            Link { from: GROUP_TABS, dir: Dir::Up, to: GROUP_SCRUB },
        ]);
    }
    /// The transport's clocks, queued for the presenting side to rasterise before the draw's
    /// back-buffer wait (`player_hud::motion::ClockWarm`). Only while the playbar is drawn: the gate is
    /// `draw`'s own (`hud_drawn` and the transport's middle).
    fn prepare(&mut self, _b: &mut Budget, cx: &Cx<'_, H>) {
        let ps = H::session(cx);
        if self.transport && self.hud_drawn(ps, cx.tick.ms) {
            // `model` is the app's snapshot of the PREVIOUS frame, and a held scrub moves the
            // target by seconds a frame: the one field the page already holds fresher (its `Tick`
            // ran before this pass), so the prewarm and the draw agree on it.
            self.model.scrub_ns = self.published_scrub();
            self.clock_warm.queue(&self.model);
        }
    }
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, H>) {
        // The frame's publication of the playback session (spec §2.3) — see `AppViews::session`.
        let ps = H::session(f.cx);
        let now = f.cx.tick.ms;
        // Both subtitle paths lift clear of the transport for the same reason and by the same
        // test — an open track menu counts, since that is exactly when the user is reading the
        // bottom of the screen. `transport` is false while the Info card or Chapters strip owns
        // the middle, and the lift follows the panel rather than the transport there. The Timing
        // capsule is the exception: it HIDES the transport rather than sharing its read time, so
        // captions stay at their normal `SUB_BASE_Y` even while it is up (`Self::subs_lift`).
        let subs_lift = self.subs_lift(ps, now);
        // The band the layout occupies, a value from its geometry this frame, not a constant.
        let band = self.hud_geometry(f.cx).band();
        self.render.ass.draw(subs_lift, band, &self.model);
        if let Some(message) = self.render.ass.error() {
            plx_appkit::player_hud::draw_subtitle_message(message, subs_lift, band, &self.model, f.cx.measure);
        }
        // a sidecar that produced nothing says so for a few seconds; the server burning the
        // selection draws it already, so no second notice then
        if let Some(message) = plx_media::player::sidecar::notice()
            .filter(|_| !plx_media::route::subtitles_burned(ps))
        {
            plx_appkit::player_hud::draw_subtitle_message(message, subs_lift, band, &self.model, f.cx.measure);
        }
        self.draw_subtitle_bitmap(subs_lift, band); // PGS/VobSub image subs
        // What a pointer can hit is registered AFTER the paint, by `record_stops`, from this
        // screen's own `Focusable` — the rects D-pad focus uses, in the z-order `groups` states.
        // `Hover::Ignore` on the transport's stops: the old pointer path never followed the mouse
        // into a HOVER-driven focus change on this route, and a DRAG is `§7.5`'s own case (no
        // hover, no click) which the hit map answers whatever this says.
        //
        // `Activate::Direct` on them, and it is not the same statement `ElemKind` makes
        // elsewhere: the transport's control row DOES dip, and OK on it is the engine's own press
        // (`key_ok` leaves it alone, the commit is `row_pressed`). A pointer CLICK on it is still
        // the loop's (`PlayerReq::ArmControlRow` → `press.begin_ctl` → `activate_player_row` on the
        // spring-back), which the hit census pins; both converge on `perform_hud_command`. Direct
        // delivery, and the screen decides what the element means — including the scrubber, whose
        // meaning needs the click's `x` and so cannot be carried by a bare `ScreenEvent::Activate`
        // at all, and the tabs, which open a panel at once with no dip to wait for.
        if let Some(frame) = H::multiview_offer(f.cx) {
            let empty = plx_platform::i18n::msg::multiview_empty();
            let loading = plx_platform::i18n::msg::multiview_loading();
            let failed = plx_platform::i18n::msg::multiview_failed();
            frame.draw(f.painter, f.measure, &plx_appkit::multiview_chrome::Labels {
                empty, loading, failed,
            });
        }
        if self.permanent_clock_key().is_some() {
            let _ = self.render.osd_clock.draw(f.painter, &self.model, f.measure);
        }
        let hud_drawn = self.hud_drawn(ps, now);
        if hud_drawn {
            self.draw_hud_in_mode(ps, now, f.measure, H::metadata(f.cx), H::ui_mode(&f.cx.views));
        } else {
            self.row.reset_title();
        }
        // Failure notices have their own bounded lifetime; a loading spinner still
        // reflects active work. Neither notice pins the ordinary transport HUD.
        let readout = if player_hud::failure_row_drawn(self.busy) && !self.failure_banner_visible() {
            player_hud::Busy::None
        } else { self.busy };
        plx_appkit::player_hud::classic::draw_readout(ps, readout, now, 0, f.measure);
        if self.volume_notice.visible() && !self.repair_alert.visible() && !self.reminder_notice.visible() {
            self.volume_notice.draw(plx_platform::i18n::msg::settings_cancel_c(),
                plx_platform::i18n::msg::settings_cancel_c(), f.measure);
        }
        if self.repair_alert.visible() {
            self.repair_alert.draw_scrim();
            let confirm = if self.reminder_sheet { plx_platform::i18n::msg::ott_reminder_switch_c() }
                else { plx_platform::i18n::msg::widgets_repair_action_c() };
            self.repair_alert.draw(plx_platform::i18n::msg::settings_cancel_c(), confirm, f.measure);
        }
        if self.reminder_notice.visible() {
            self.reminder_notice.draw(plx_platform::i18n::msg::settings_cancel_c(),
                plx_platform::i18n::msg::settings_cancel_c(), f.measure);
        }
        self.record_stops(f, hud_drawn);
    }
    fn render(&self) -> RenderStrategy {
        RenderStrategy::VideoPlane
    }
    /// The PGS/VobSub display set is the only render this screen owns (§8.3 rule (c)): the picture
    /// is the hardware video plane, the HUD is drawn immediate-mode, and its glyphs and icons come
    /// from the shared caches. `SubtitleBitmaps` uploads one texture per rect of the active cue and
    /// deletes them itself, so it is the one thing on this screen whose bytes belong to this
    /// instance rather than to a pool.
    fn render_report(&self) -> plx_ui::frame::RenderReport {
        let bitmap = self.render.subs.render_report();
        let ass = self.render.ass.render_report();
        plx_ui::frame::RenderReport { textures: bitmap.textures + ass.textures, bytes: bitmap.bytes + ass.bytes }
    }
    fn focus_source(&self) -> FocusSource {
        FocusSource::Engine
    }
    fn hit_source(&self) -> HitSource {
        HitSource::Engine
    }
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

/// **The player page's clock-driven animators, one test per CLASS** (spec §8.3, §9, phase 9).
///
/// Until phase 9 the loop's gate read `idle::should_present(now) || fr.player`, so nothing drawn
/// on this route ever had to report motion: the ROUTE presented unconditionally. The gate is keyed
/// on the video plane being BOUND now, which leaves two windows where the player page is on screen
/// and the plane is not under it — the pre-bind spinner and everything after an unbind — and in
/// those windows an unreported animator is a FROZEN one. `Xfade::tick` and `Spinner::draw` each
/// shipped exactly that way before they were taught to report.
///
/// Each test below drives ONE class through the whole path the loop uses — the fingerprint, the
/// machine's comparison, the live gate — with the plane UNBOUND, and asserts the frame presents.
///
/// **Observed RED, simulated in every case** (the fix changes the signatures the old code called,
/// so these cannot be compiled against 88841d3e): deleting that class's `mix(...)` term from
/// `PlayerScreen::clock_fingerprint` — which is precisely "this animator does not report" — makes
/// the fingerprint identical across the change and the assertion fails at "…must present".
#[cfg(test)]
mod clock_animator_tests {
    use super::*;
    use plx_media::player::machine::Player;
    use plx_media::route::PlaybackSession;

    #[test]
    fn accepted_picture_change_invalidates_a_stationary_subtitle_frame() {
        let _serial = plx_base::testlock::serial();
        let ps = PlaybackSession::default();
        let mut screen = PlayerScreen::new(EntryId(8));
        let before = screen.clock_fingerprint(&ps, 1000);
        screen.model.video_presentation.x = -240;
        screen.model.video_presentation.width = 2400;
        let after = screen.clock_fingerprint(&ps, 1000);
        assert_ne!(before, after);
        assert_eq!(after, screen.clock_fingerprint(&ps, 1000));
    }

    /// One iteration of the loop's motion report, with the plane UNBOUND — the fingerprint and
    /// the machine's comparison, exactly as `app::run`'s prepare phase runs them. `true` = THIS
    /// path raised damage, i.e. this frame is owed a present.
    ///
    /// Graded on `idle::take_local_damage`, which counts what this THREAD reported, rather than on
    /// `should_present`, which reads the process-wide `DIRTY`/`DAMAGE_GEN` that every unlocked
    /// test repainting anything also moves — the flake `plx_machine::idle`'s own `LOCAL_DAMAGE` doc
    /// describes, and which cost this test two red runs in six before it was written this way.
    /// The gate's own half is asserted once, below, where it cannot be made false by another
    /// thread: a spurious `invalidate` elsewhere can only make `should_present` MORE true.
    fn reported(pl: &mut Player, screen: &PlayerScreen, ps: &PlaybackSession, now: u32) -> bool {
        let _ = plx_machine::idle::take_local_damage();
        if pl.note_clock(screen.clock_fingerprint(ps, now)) {
            plx_machine::idle::invalidate();
        }
        plx_machine::idle::take_local_damage() > 0
    }

    /// Run the page to a standstill, so that a report below can only be this test's own doing.
    fn settled(pl: &mut Player, screen: &PlayerScreen, ps: &PlaybackSession, now: u32) {
        plx_machine::idle::set_enabled(true);
        plx_machine::idle::frame_begin(1.0 / 60.0);
        for t in 0..4 {
            reported(pl, screen, ps, now + t);
        }
        assert!(
            !reported(pl, screen, ps, now + 4),
            "the fixture must start from a page that reports nothing, or what follows proves \
             nothing",
        );
    }

    /// The other half, asserted once per test: a report really does open the gate.
    fn presents(now: u32) -> bool {
        plx_machine::idle::should_present(now)
    }

    /// **The MEDIA CLOCK class** — the scrub bar, the playhead, both clocks, the hold-to-scrub
    /// ramp. Everything derived from `playpos_ns`, which LG's media thread writes and no spring
    /// can see.
    ///
    /// With the transport UP, which is the only state in which any of it is drawn: the fingerprint
    /// deliberately drops the playhead's readings while the HUD is hidden, since a report for a
    /// picture nobody draws is the gate turned off.
    #[test]
    fn a_moving_playhead_presents_a_frame_while_the_plane_is_unbound() {
        let _g = plx_base::testlock::serial();
        let ps = PlaybackSession::IDLE;
        let mut pl = Player::new();
        let mut screen = PlayerScreen::new(plx_machine::machine::EntryId(1));
        screen.hud.extend(1_000, 60_000);
        assert!(screen.hud_up(&ps, 1_005), "the fixture: the scrub bar is on screen");
        assert!(!pl.video_plane_bound, "the window this test is about");

        plx_media::player::SHARED.playpos_ns.store(0, std::sync::atomic::Ordering::Relaxed);
        settled(&mut pl, &screen, &ps, 1_000);
        plx_media::player::SHARED
            .playpos_ns
            .store(500_000_000, std::sync::atomic::Ordering::Relaxed);
        assert!(
            reported(&mut pl, &screen, &ps, 1_005),
            "the playhead moved half a second: the scrub bar and both clocks must report",
        );
        assert!(presents(1_005), "…and a report is a frame");
        plx_media::player::SHARED.playpos_ns.store(0, std::sync::atomic::Ordering::Relaxed);
    }

    /// **The DEADLINE class** — the HUD's auto-hide. At `now == until` the whole transport leaves
    /// the screen, from a countdown, with nothing else in the frame moving at all.
    #[test]
    fn the_huds_auto_hide_deadline_presents_the_frame_it_expires_on() {
        let _g = plx_base::testlock::serial();
        let ps = PlaybackSession::IDLE;
        let mut pl = Player::new();
        let mut screen = PlayerScreen::new(plx_machine::machine::EntryId(1));
        screen.hud.extend(2_000, 500); // on screen until 2_500

        assert!(screen.hud_up(&ps, 2_400), "the fixture: still up before the deadline");
        settled(&mut pl, &screen, &ps, 2_100);
        assert!(!screen.hud_up(&ps, 2_600), "…and gone after it");
        assert!(
            reported(&mut pl, &screen, &ps, 2_600),
            "the transport disappeared on this frame: it must report",
        );
        assert!(presents(2_600), "…and a report is a frame");
    }

    /// **The TRANSPORT MARK class** — the two-second `Play` glyph, whose own doc used to record
    /// that no report was owed for it because the player route presented unconditionally.
    #[test]
    fn the_play_marks_two_second_expiry_presents_a_frame() {
        let _g = plx_base::testlock::serial();
        let ps = PlaybackSession::IDLE;
        let mut pl = Player::new();
        let mut screen = PlayerScreen::new(plx_machine::machine::EntryId(1));
        // A resume edge at t=5s. Stamped directly rather than derived from `player::TX`, which
        // every playback test in the crate moves: what is under test is the CLOCK, not the
        // observer that reads the transport.
        screen.row.force_play_at_for_test(5_000);
        assert!(
            screen.row.since_play_ms(5_100).is_some_and(|d| d < plx_appkit::player_hud::PLAY_MARK_MS),
            "the fixture: the Play mark is on screen",
        );

        settled(&mut pl, &screen, &ps, 5_100);
        let after = 5_000 + plx_appkit::player_hud::PLAY_MARK_MS + 1;
        assert!(
            reported(&mut pl, &screen, &ps, after),
            "the Play mark stopped being drawn on this frame: it must report",
        );
        assert!(presents(after), "…and a report is a frame");
    }

    /// **The READ-OUT class** — `busy`, i.e. the centred status surface appearing and vanishing.
    /// The `Playing -> Error` edge with the HUD auto-hidden is the one that matters: nothing else
    /// in that frame moves, and what the viewer would otherwise keep looking at is a black screen.
    #[test]
    fn a_read_out_appearing_presents_a_frame_while_the_plane_is_unbound() {
        let _g = plx_base::testlock::serial();
        let ps = PlaybackSession::IDLE;
        let mut pl = Player::new();
        let mut screen = PlayerScreen::new(plx_machine::machine::EntryId(1));
        screen.busy = plx_appkit::player_hud::Busy::None;

        settled(&mut pl, &screen, &ps, 9_000);
        // The fingerprint reads `player_hud::busy(ps)` for itself rather than this field, so the
        // state has to move where the read-out reads it — the pipeline state the pump publishes.
        plx_media::player::SHARED.pb_state.store(
            plx_media::player::PlaybackState::Error as u8,
            std::sync::atomic::Ordering::Relaxed,
        );
        assert!(
            reported(&mut pl, &screen, &ps, 9_005),
            "the failure read-out took the screen on this frame: it must report",
        );
        assert!(presents(9_005), "…and a report is a frame");
        plx_media::player::SHARED.pb_state.store(
            plx_media::player::PlaybackState::Idle as u8,
            std::sync::atomic::Ordering::Relaxed,
        );
    }
}

#[cfg(test)]
mod render_residency_tests {
    use super::*;
    use plx_ui::frame::RenderReport;
    use plx_appkit::player_hud::SubtitleBitmaps;

    /// **The player's own render is its image-subtitle display set, and only that** (§8.3 rule
    /// (c)). The picture is the hardware video plane, which is not ours at all; the HUD is drawn
    /// immediate-mode from the shared glyph and icon caches. A screen that owns a texture has to
    /// SAY so, or the frame's byte term is a sum over an empty inventory — which is what
    /// `Screen::render_bytes`'s single `{ 0 }` default and zero overrides made it until phase 11.
    #[test]
    fn the_player_reports_its_image_subtitle_set_and_nothing_else() {
        let mut s = PlayerScreen::new(EntryId(1));
        assert_eq!(
            Screen::<crate::player::overlay_tests::TestHost>::render_report(&s),
            RenderReport::NONE,
            "no image cue up: this screen holds no render of its own",
        );
        s.render.subs = SubtitleBitmaps::stub(&[(720, 120)]);
        assert_eq!(
            Screen::<crate::player::overlay_tests::TestHost>::render_report(&s),
            RenderReport::one(720, 120),
            "the cue on screen is one texture and its pixels",
        );
    }
}

/// **The part of the dispatcher a direct `PlayerScreen::step` test needs** (§7.3): a real
/// `FocusEngine`, the screen's own `links()`, `Focusable` and edge rules, and the same order the
/// dispatcher keeps. A direction the page leaves unhandled is the engine's; a move arrives as
/// `FocusMoved` before anything else; a `Screen` edge comes back as the key with `at_edge` set,
/// behind the rest of the queue; an `Enter::Fresh` the page pushes (its re-seats) is entered and
/// reports its own `FocusMoved`. Nothing here decides travel for the page, so a test through it
/// grades the tables the page declares, not a copy of them.
#[cfg(test)]
mod travel_rig {
    use super::*;
    use crate::player::overlay_tests::TestHost;
    use crate::registry::AppFx;
    use plx_machine::machine::{Delivery, InputEvent, InputOwner, MachineId, Source, Tick};
    use plx_ui::fixture::FixtureMeasure;
    use plx_ui::focus::{FocusEngine, Outcome};
    use plx_ui::screen::{Enter, FocusTarget};
    use std::collections::VecDeque;

    const INST: InstanceId = InstanceId(3);

    pub(super) struct Rig {
        entry: EntryId,
        engine: FocusEngine<u32>,
        now: u32,
    }

    impl Rig {
        /// An engine holding focus where `page.hud.nav` says it is, by that group's own seat.
        pub(super) fn on(page: &PlayerScreen, entry: EntryId) -> Self {
            let mut rig = Self { entry, engine: FocusEngine::new(), now: 1_000 };
            let cx = rig.cx();
            rig.engine.enter(
                rig.owner(),
                page,
                FocusTarget::ContainerGroup(page.hud.nav.focus),
                None,
                &cx,
            );
            rig
        }

        fn owner(&self) -> InputOwner {
            InputOwner::Entry(self.entry)
        }

        /// What the app does after it writes `hud.nav.focus` itself: hand the engine a fresh seat on
        /// that group and tell the page where the engine went.
        pub(super) fn reseat(&mut self, page: &mut PlayerScreen, group: GroupId) {
            let cx = self.cx();
            if let Outcome::Moved { from, to, by } =
                self.engine.enter(self.owner(), &*page, FocusTarget::ContainerGroup(group), None, &cx)
            {
                self.deliver(page, ScreenEvent::FocusMoved { from, to, by });
            }
        }

        /// Seat the engine on one element of the control row, as a pointer click or a restore does,
        /// without delivering the move: the page's cursor is whatever the test left it.
        pub(super) fn seat_on(&mut self, elem: u32) {
            let key = FocusKey { entry: self.entry, elem };
            self.engine.set(self.owner(), key, Some(GROUP_ROW), plx_ui::screen::By::Restore);
        }

        /// The element the engine holds focus on.
        pub(super) fn current(&self) -> Option<u32> {
            self.engine.current(self.owner()).map(|k| k.elem)
        }

        pub(super) fn cx(&self) -> Cx<'static, TestHost> {
            Cx {
                views: (),
                tick: Tick { ms: self.now, dt_us: 0 },
                measure: &FixtureMeasure,
                focus: self.engine.read(self.owner()),
                press: Default::default(),
                owner: self.owner(),
            }
        }

        /// One key through the page and everything the dispatcher would do after it. Returns the
        /// KEY's own answer and every `PlayerReq` the whole exchange asked for.
        pub(super) fn key(
            &mut self,
            page: &mut PlayerScreen,
            sym: u32,
            wcode: u32,
            edge: Edge,
            ms: u32,
        ) -> (Handled, Vec<PlayerReq>) {
            self.now = ms;
            self.deliver(
                page,
                ScreenEvent::Input(InputEvent {
                    at: Tick { ms, dt_us: 0 },
                    source: Source::Sdl,
                    kind: InputKind::Key {
                        key: plx_machine::machine::Key::Other,
                        sym,
                        wcode,
                        edge,
                        at_edge: false,
                    },
                }),
            )
        }

        pub(super) fn deliver(
            &mut self,
            page: &mut PlayerScreen,
            ev: ScreenEvent<TestHost>,
        ) -> (Handled, Vec<PlayerReq>) {
            let mut reqs = Vec::new();
            let mut first = None;
            let mut queue = VecDeque::from([ev]);
            while let Some(ev) = queue.pop_front() {
                let cx = self.cx();
                let mut out = Vec::new();
                let mut present = plx_machine::present::Present::new();
                let handled = page.step(
                    &ev,
                    &cx,
                    &mut Effects::new(&mut out, MachineId::Instance(INST), &mut present),
                );
                first.get_or_insert(handled);
                for e in out {
                    match e.fx {
                        Fx::App(AppFx::Player(req)) => reqs.push(req),
                        Fx::Deliver(_, Delivery::Screen(ScreenEvent::Enter(Enter::Fresh { focus }))) => {
                            let cx = self.cx();
                            if let Outcome::Moved { from, to, by } =
                                self.engine.enter(self.owner(), &*page, focus, None, &cx)
                            {
                                queue.push_front(ScreenEvent::FocusMoved { from, to, by });
                            }
                        }
                        _ => {}
                    }
                }
                let ScreenEvent::Input(InputEvent { at, source, kind: InputKind::Key { key, sym, wcode, edge, at_edge } }) = &ev
                else { continue };
                let dir = match consts::classify_input(*key, *sym, *wcode) {
                    consts::Key::Up => Dir::Up,
                    consts::Key::Down => Dir::Down,
                    _ => continue,
                };
                if handled != Handled::No || *edge == Edge::Up {
                    continue;
                }
                let mut links = Vec::new();
                Screen::<TestHost>::links(&*page, &mut links);
                let cx = self.cx();
                match self.engine.move_dir(self.owner(), &*page, &links, dir, &cx) {
                    Outcome::Moved { from, to, by } => queue.push_front(ScreenEvent::FocusMoved { from, to, by }),
                    Outcome::Edge(EdgeRule::Screen) if !*at_edge => {
                        queue.push_back(ScreenEvent::Input(InputEvent {
                            at: *at,
                            source: *source,
                            kind: InputKind::Key { key: *key, sym: *sym, wcode: *wcode, edge: *edge, at_edge: true },
                        }));
                    }
                    _ => {}
                }
            }
            (first.expect("one event went in"), reqs)
        }
    }
}

/// **The transport ladder, ported onto `PlayerScreen::step`** (restructure phase 12, D2 Part C).
///
/// Each case here was watched RED first against `88841d3e` (the contract-freeze commit, whose
/// `Input` arm was a stub returning `Handled::No` unconditionally) before this package's `step`
/// body was written — the same shape `overlay_tests.rs`'s own suite already uses for this screen's
/// sibling, and the reason both suites share `overlay_tests::TestHost`/`cx` rather than each
/// inventing a fixture.
///
/// **Every case holds `testlock::serial()` since phase 12 (PX-PLAYER), and none did before.**
/// `handle_key`'s first test is now `player_hud::transport_hidden`, which reads the crate-global
/// playback state — so a case in another module driving that state to `Error` turns every key
/// here into the failure read-out's policy. It was not hypothetical: the failure-escape case in
/// `scrub_ownership_tests` below does exactly that, and two cases here failed on the first run
/// after it was written, in the shape `docs/agent-reference.md` describes — plausible wrong data
/// rather than a clean failure (`ok_elsewhere_toggles_play_pause` asked for the quality ladder).
/// A screen that reads a crate global from its input path owes the lock to every test that drives
/// that path.
#[cfg(test)]
mod step_ladder_tests {
    use super::*;
    use crate::player::overlay_tests::TestHost;
    use crate::registry::AppFx;
    use plx_ui::consts::{
        SDLK_DOWN, SDLK_RETURN, SDLK_UP, WCODE_BACK, WCODE_PAUSE, WCODE_PLAY, WCODE_PLAYPAUSE,
        WCODE_STOP,
    };
    use plx_ui::fixture::FixtureMeasure;
    use super::travel_rig::Rig;
    use plx_machine::machine::{
        Cx, Edge, Effects, EntryId, Fx, Handled, InputEvent, InputKind, InputOwner, MachineId,
        Source, Tick,
    };

    const ENTRY: EntryId = EntryId(9);
    const INST: InstanceId = InstanceId(3);

    fn cx() -> Cx<'static, TestHost> {
        Cx {
            views: (),
            tick: Tick::default(),
            measure: &FixtureMeasure,
            focus: Default::default(),
            press: Default::default(),
            owner: InputOwner::Entry(ENTRY),
        }
    }

    /// One key press through `PlayerScreen::step`, returning `(handled, the PlayerReqs it asked
    /// for)` — the same shape `overlay_tests.rs::press` returns for the sibling surface.
    fn press(page: &mut PlayerScreen, sym: u32, wcode: u32, edge: Edge) -> (Handled, Vec<PlayerReq>) {
        let mut out = Vec::new();
        let mut present = plx_machine::present::Present::new();
        let ev = ScreenEvent::<TestHost>::Input(InputEvent {
            kind: InputKind::Key {
                key: plx_machine::machine::Key::Other,
                sym,
                wcode,
                edge,
                at_edge: false,
            },
            at: Tick { ms: 1_000, dt_us: 0 },
            source: Source::Sdl,
        });
        let handled = page.step(
            &ev,
            &cx(),
            &mut Effects::new(&mut out, MachineId::Instance(INST), &mut present),
        );
        let reqs = out
            .into_iter()
            .filter_map(|e| match e.fx {
                Fx::App(AppFx::Player(req)) => Some(req),
                _ => None,
            })
            .collect();
        (handled, reqs)
    }

    fn click(page: &mut PlayerScreen, elem: u32) -> (Handled, Vec<PlayerReq>) {
        let mut out = Vec::new();
        let mut present = plx_machine::present::Present::new();
        let ev = ScreenEvent::<TestHost>::Input(InputEvent {
            kind: InputKind::Click { x: 0.0, y: 0.0, hit: Some(elem) },
            at: Tick { ms: 1_000, dt_us: 0 },
            source: Source::Sdl,
        });
        let handled = page.step(
            &ev,
            &cx(),
            &mut Effects::new(&mut out, MachineId::Instance(INST), &mut present),
        );
        let reqs = out
            .into_iter()
            .filter_map(|e| match e.fx {
                Fx::App(AppFx::Player(req)) => Some(req),
                _ => None,
            })
            .collect();
        (handled, reqs)
    }

    /// One frame's `ScreenEvent::Tick` through `PlayerScreen::step`: the per-frame half of the page,
    /// where nothing is pressed.
    fn tick(page: &mut PlayerScreen, ms: u32) {
        let mut out = Vec::new();
        let mut present = plx_machine::present::Present::new();
        page.step(
            &ScreenEvent::<TestHost>::Tick(Tick { ms, dt_us: 16_000 }),
            &cx(),
            &mut Effects::new(&mut out, MachineId::Instance(INST), &mut present),
        );
    }

    #[test]
    fn numeric_notice_and_volume_keep_presentation_order() {
        let _lock = plx_base::testlock::serial();
        let mut page = PlayerScreen::new(ENTRY);
        let number = Some(("2".into(), "Channel Two".into()));
        let volume = plx_media::player::volume::DesiredVolume::new(55, true).unwrap();
        page.set_number_notice(&number, 100);
        assert!(page.number_notice);
        assert!(page.volume_notice(volume, 101));
        page.set_number_notice(&None, 102);
        assert!(page.volume_notice.is_open(), "old number cancellation must not hide newer volume");
        assert!(!page.number_notice);
        assert_eq!(page.volume_notice.body_for_test()[0].as_ref(), plx_platform::i18n::msg::widgets_toggle_off());
        page.set_number_notice(&number, 103);
        assert!(page.number_notice);
        assert_eq!(page.volume_notice.body_for_test()[0].as_ref(), "2");
        tick(&mut page, 2_103);
        assert!(!page.volume_notice.is_open());
    }

    #[test]
    fn volume_notice_expires_after_last_change_without_revealing_the_hud() {
        let _guard = plx_base::testlock::serial();
        let mut page = PlayerScreen::new(ENTRY);
        page.hud.hide(false);
        page.hud.nav.focus = GROUP_SCRUB;
        let before = (page.hud.until, page.hud.nav.focus, page.hud.nav.btn, page.hud.dismissed);
        let volume = plx_media::player::volume::DesiredVolume::new(55, false).unwrap();
        assert!(page.volume_notice(volume, 100));
        assert_eq!(page.volume_notice.body_for_test()[0].as_ref(), "55%");
        tick(&mut page, 2_099);
        assert!(page.volume_notice.is_open());
        assert!(page.volume_notice(volume.with_muted(true), 2_099));
        assert_eq!(page.volume_notice.body_for_test()[0].as_ref(), plx_platform::i18n::msg::widgets_toggle_off());
        tick(&mut page, 4_098);
        assert!(page.volume_notice.is_open());
        plx_machine::idle::take_local_damage();
        tick(&mut page, 4_099);
        assert!(plx_machine::idle::take_local_damage() > 0, "expiry must wake a settled frame");
        assert!(!page.volume_notice.is_open());
        assert!(page.volume_notice_until.is_none());
        assert_eq!((page.hud.until, page.hud.nav.focus, page.hud.nav.btn, page.hud.dismissed), before);
        page.clear_reminders();
        assert!(!page.volume_notice.visible());
    }

    #[test]
    fn volume_notice_expiry_wraps_with_the_frame_clock() {
        let _guard = plx_base::testlock::serial();
        let mut page = PlayerScreen::new(ENTRY);
        let now = u32::MAX - 999;
        assert!(page.volume_notice(plx_media::player::volume::DesiredVolume::DEFAULT, now));
        tick(&mut page, 999);
        assert!(page.volume_notice.is_open());
        tick(&mut page, 1_000);
        assert!(!page.volume_notice.is_open());
        page.clear_reminders();
    }

    fn fresh(page: &mut PlayerScreen) {
        page.hud.visible_at_press = true;
    }

    /// One BOUND key press as the app delivers it: `note_global_press` samples the fresh-press
    /// state (`HudState::note_fresh_press`) and only then does the page's `key` arm run. [`fresh`]
    /// skips that sample, which is right for a test of one arm's body and wrong for anything about
    /// what the PREVIOUS press left behind — the two-press BACK reads exactly that.
    fn bound_press(page: &mut PlayerScreen, sym: u32, wcode: u32) -> (Handled, Vec<PlayerReq>) {
        page.hud.note_fresh_press(TestHost::session(&cx()), 1_000, false);
        press(page, sym, wcode, Edge::Down)
    }

    /// [`bound_press`] for UP and DOWN, which the page leaves to the focus engine: the press goes
    /// through the dispatcher's own order ([`travel_rig`]), so the travel under test is the page's
    /// declared links and edge rules and nothing a test wrote.
    fn bound_travel(rig: &mut Rig, page: &mut PlayerScreen, sym: u32) -> (Handled, Vec<PlayerReq>) {
        page.hud.note_fresh_press(TestHost::session(&cx()), 1_000, false);
        rig.key(page, sym, 0, Edge::Down, 1_000)
    }

    /// The census's other half: a control the frame does NOT draw registers no stop, so a click
    /// there reaches the picture (play/pause) rather than a blind action. A hidden HUD registers
    /// nothing; with an Info card or Chapters strip over the middle (`transport == false`) only
    /// the tabs remain.
    #[test]
    fn a_control_the_frame_does_not_draw_registers_no_stop() {
        use plx_appkit::player_hud::ELEM_TAB_BASE;
        use plx_ui::screen::DrawFrame;
        let _g = plx_base::testlock::serial();
        for (transport, hud_drawn, want) in [
            (true, false, vec![]),
            (false, false, vec![]),
            (false, true, vec![ELEM_TAB_BASE]),
        ] {
            let mut page = PlayerScreen::new(ENTRY);
            page.transport = transport;
            let cx = cx();
            let mut f = DrawFrame::new(&cx, plx_ui::Painter::root());
            page.record_stops(&mut f, hud_drawn);
            let got: Vec<u32> = f.into_stops().iter().map(|s| s.key.elem).collect();
            assert_eq!(got, want, "transport={transport} hud_drawn={hud_drawn}");
        }
    }

    /// **The failure notice expiring owes a frame.** Nothing else on the screen changes when the
    /// 6 s notice lapses (the cues are libass's, whose own fingerprint is unchanged), so
    /// `clock_fingerprint` must move on `sidecar::notice_shown()` alone — before it did, the
    /// notice's text stayed on the last presented frame until something else drew.
    #[test]
    fn classic_wallclock_changes_report_on_paused_video_but_not_hidden_or_modern() {
        use plx_machine::machine::UiMode;
        let _g = plx_base::testlock::serial();
        let mut page = PlayerScreen::new(ENTRY); let cx = cx(); let ps = TestHost::session(&cx);
        page.transport = true; page.hud.until = 10_000; page.hud.dismissed = false;
        page.model.paused = true; page.model.wall_now_s = Some(100);
        let old = page.clock_fingerprint_in_mode(ps, 1_000, UiMode::Classic, None);
        let modern = page.clock_fingerprint_in_mode(ps, 1_000, UiMode::Modern, None);
        page.model.wall_now_s = Some(101);
        assert_ne!(old, page.clock_fingerprint_in_mode(ps, 1_000, UiMode::Classic, None));
        assert_eq!(modern, page.clock_fingerprint_in_mode(ps, 1_000, UiMode::Modern, None));
        page.hud.until = 0; page.hud.dismissed = true;
        let hidden = page.clock_fingerprint_in_mode(ps, 1_000, UiMode::Classic, None);
        page.model.wall_now_s = Some(102);
        assert_eq!(hidden, page.clock_fingerprint_in_mode(ps, 1_000, UiMode::Classic, None));
        page.hud.until = 10_000; page.hud.dismissed = false; page.transport = false;
        let covered = page.clock_fingerprint_in_mode(ps, 1_000, UiMode::Classic, None);
        page.model.wall_now_s = Some(103);
        assert_eq!(covered, page.clock_fingerprint_in_mode(ps, 1_000, UiMode::Classic, None));
    }

    #[test]
    fn the_failure_notice_expiring_changes_the_clock_fingerprint() {
        let _g = plx_base::testlock::serial();
        let page = PlayerScreen::new(ENTRY);
        let cx = cx();
        let ps = TestHost::session(&cx);
        plx_media::player::sidecar::reset();
        let quiet = page.clock_fingerprint(ps, 1_000);
        plx_media::player::sidecar::fail_for_test(9, std::time::Duration::ZERO);
        assert!(plx_media::player::sidecar::notice_shown());
        let shown = page.clock_fingerprint(ps, 1_000);
        assert_ne!(shown, quiet, "the notice appearing is a change");
        plx_media::player::sidecar::fail_for_test(9, std::time::Duration::from_secs(7));
        assert!(!plx_media::player::sidecar::notice_shown());
        let expired = page.clock_fingerprint(ps, 1_000);
        assert_ne!(expired, shown, "the notice lapsing is a change");
        assert_eq!(expired, quiet, "and nothing else moved");
        plx_media::player::sidecar::reset();
    }

    /// A text cue appearing or clearing reaches the frame gate through libass's own fingerprint
    /// (`AssSubtitles::fingerprint`), the only subtitle term besides the notice and the bitmaps.
    #[test]
    fn a_libass_cue_change_changes_the_clock_fingerprint() {
        let _g = plx_base::testlock::serial();
        let mut page = PlayerScreen::new(ENTRY);
        let cx = cx();
        let ps = TestHost::session(&cx);
        let before = page.clock_fingerprint(ps, 1_000);
        assert_eq!(before, page.clock_fingerprint(ps, 1_000), "a quiet screen is stable");
        page.render.ass.set_frame_serial_for_test(7);
        assert_ne!(before, page.clock_fingerprint(ps, 1_000));
    }

    /// **`HudPolicy::Hidden` withholds everything a lift would draw** (plan
    /// `subtitle-menu-capsule` §4): no caption lift, no transport and no scrub/row/tab stop, even
    /// with the transport otherwise up. The policy is pushed BY HAND here — no frame loop runs in
    /// a host test, so `app/run.rs`'s per-frame `bridge::player_hud_policy` push never happens —
    /// the same pattern [`a_control_the_frame_does_not_draw_registers_no_stop`] uses for
    /// `hud_drawn`. `Lifted`, by contrast, draws and lifts.
    #[test]
    fn the_hidden_policy_withholds_the_transport_and_the_lift() {
        use plx_ui::screen::DrawFrame;
        let _g = plx_base::testlock::serial();
        let mut page = PlayerScreen::new(ENTRY);
        page.transport = true;
        let cx = cx();
        let ps = TestHost::session(&cx);
        page.set_hud_policy(HudPolicy::Lifted);
        assert!(page.subs_lift(ps, 1_000) && page.hud_drawn(ps, 1_000), "a panel read over the page lifts");
        page.set_hud_policy(HudPolicy::Hidden);
        assert!(!page.subs_lift(ps, 1_000), "captions must stay at SUB_BASE_Y under the capsule");
        assert!(!page.hud_drawn(ps, 1_000));
        let mut f = DrawFrame::new(&cx, plx_ui::Painter::root());
        page.record_stops(&mut f, page.hud_drawn(ps, 1_000));
        let got: Vec<u32> = f.into_stops().iter().map(|s| s.key.elem).collect();
        assert_eq!(got, Vec::<u32>::new(), "no scrub/row/tab stop registers under the capsule");
    }

    /// **A prepared player frame queues the transport's clocks, and only while the playbar is
    /// drawn.** The clocks are new strings once a second; left to the draw they were rasterised
    /// AFTER the frame's back-buffer wait (`clear:12.4 … textx2:1.1` on the TV), which is what
    /// took those frames past 20 ms under an open Tracks/More popover (`HudPolicy::Lifted`).
    #[test]
    fn prepare_queues_the_transport_clocks_only_while_the_playbar_is_drawn() {
        let _g = plx_base::testlock::serial();
        plx_gfx::text::reset_prewarm_for_test();
        let mut budget = Budget::new();
        let cx = cx();
        let mut page = PlayerScreen::new(ENTRY);
        page.transport = true;
        page.set_hud_policy(HudPolicy::Hidden);
        Screen::<TestHost>::prepare(&mut page, &mut budget, &cx);
        assert!(!plx_gfx::text::prewarm_pending(), "nothing is drawn under the capsule: nothing to warm");
        page.set_hud_policy(HudPolicy::Lifted);
        page.transport = false;
        Screen::<TestHost>::prepare(&mut page, &mut budget, &cx);
        assert!(!plx_gfx::text::prewarm_pending(), "an Info card owns the middle: no playbar, no clocks");
        page.transport = true;
        Screen::<TestHost>::prepare(&mut page, &mut budget, &cx);
        assert!(plx_gfx::text::prewarm_pending(), "the playbar's clocks were left for the draw to rasterise");
        plx_gfx::text::reset_prewarm_for_test();
    }

    /// **The prewarm reads the scrub this frame's draw reads.** `self.model` is the app's snapshot of
    /// the PREVIOUS frame, but a held scrub moves its target by seconds a frame, and the clocks warm
    /// only the displayed second and the next: a prewarm one frame behind the target misses every
    /// clock the draw then rasterises after the back-buffer wait. The page's own scrub, which the
    /// `Tick` before `prepare` has already advanced, is the value the draw publishes.
    #[test]
    fn prepare_warms_the_clocks_for_the_scrub_the_draw_will_show_not_last_frames() {
        let _g = plx_base::testlock::serial();
        plx_gfx::text::reset_prewarm_for_test();
        const S: i64 = 1_000_000_000;
        let mut budget = Budget::new();
        let cx = cx();
        let mut page = PlayerScreen::new(ENTRY);
        page.transport = true;
        page.set_hud_policy(HudPolicy::Lifted);
        page.model.playpos_ns = 10 * S;
        page.model.duration_ns = 7_200 * S;
        page.model.scrub_ns = -1;
        page.scrub.ns = 600 * S;
        Screen::<TestHost>::prepare(&mut page, &mut budget, &cx);
        assert_eq!(page.clock_warm.warmed_for_test(), Some((600, 6_600)), "the scrub target's second");
        page.scrub.ns = -1;
        Screen::<TestHost>::prepare(&mut page, &mut budget, &cx);
        assert_eq!(page.clock_warm.warmed_for_test(), Some((10, 7_190)), "no scrub: the playhead's");
        plx_gfx::text::reset_prewarm_for_test();
    }

    /// **However the capsule's surface closes, the transport stays down after it** (finding: a
    /// pointer miss closed it through `OnMiss::Dismiss` without the key ladder's old HideHud
    /// request). `set_hud_policy`'s falling edge is the one place that turns the close into a
    /// dismissed HUD; a later bound key raises it again as usual.
    #[test]
    fn the_capsule_leaving_leaves_the_transport_dismissed_however_it_closed() {
        let _g = plx_base::testlock::serial();
        let paused = plx_media::player::TX.paused.swap(true, std::sync::atomic::Ordering::Relaxed);
        let cx = cx();
        let ps = TestHost::session(&cx);
        let mut page = PlayerScreen::new(ENTRY);
        page.transport = true;
        assert!(page.hud_drawn(ps, 1_000), "paused: the transport is up before the capsule");
        page.set_hud_policy(HudPolicy::Hidden);
        assert!(!page.hud_drawn(ps, 1_000), "hidden under the capsule");
        page.set_hud_policy(HudPolicy::Normal); // the surface is gone — no key reached the overlay's ladder
        assert!(!page.hud_drawn(ps, 1_000), "and stays down once the capsule has left");
        page.hud.note_fresh_press(ps, 1_100, true);
        assert!(page.hud_drawn(ps, 1_100), "a bound key raises it again");
        plx_media::player::TX.paused.store(paused, std::sync::atomic::Ordering::Relaxed);
    }

    /// **A fresh LEFT on the capsule while paused keeps the transport hidden.** The capsule's own
    /// first press goes through `note_global_press` → `note_fresh_press`, which clears
    /// `hud.dismissed`; paused, that alone would put the transport back over the capsule. The
    /// per-frame `HudPolicy::Hidden` gate is what keeps it down, and this is the layer that draws it.
    #[test]
    fn a_fresh_press_on_the_capsule_while_paused_keeps_the_transport_hidden() {
        let _g = plx_base::testlock::serial();
        let paused = plx_media::player::TX.paused.swap(true, std::sync::atomic::Ordering::Relaxed);
        let cx = cx();
        let ps = TestHost::session(&cx);
        let mut page = PlayerScreen::new(ENTRY);
        page.transport = true;
        page.set_hud_policy(HudPolicy::Hidden);
        page.hud.dismissed = true;
        page.hud.note_fresh_press(ps, 1_000, true); // the LEFT that steps the capsule
        assert!(!page.hud.dismissed, "the press did un-dismiss the HUD");
        assert!(!page.hud_up(ps, 1_000), "yet the transport stays hidden under the capsule");
        assert!(!page.hud_drawn(ps, 1_000));
        assert!(!page.subs_lift(ps, 1_000), "and the captions do not lift for it");
        plx_media::player::TX.paused.store(paused, std::sync::atomic::Ordering::Relaxed);
    }

    /// **Issue #162's census: every control the D-pad can activate is clickable with the Magic
    /// Remote pointer, all over its drawn rect, and the click acts.**
    ///
    /// The element lists below are written out per state rather than read back from `groups`, so
    /// a control the D-pad reaches but the registration forgets is a failure here, not a shorter
    /// list. Each is clicked on a grid (`ui::hit::pointer_gaps`) against the map the real
    /// `record_stops` filled — which is what caught the scrubber's grab band registered ABOVE the
    /// control row (every disc's middle seeked) and the stand-ins' single floor-width region
    /// (*Watch Credits* unclickable, *Next Episode* activating item 0). The scrubber itself is
    /// judged at a point no control covers, since controls sit on top of its band by design.
    #[test]
    fn every_control_the_dpad_reaches_is_clickable_with_the_pointer_and_acts() {
        use plx_data::metadata::{Marker, MarkerKind};
        use plx_ui::hit::{pointer_gaps, HitMap, PointerKind};
        use plx_machine::machine::FocusKey;
        use plx_appkit::player_hud::{Busy, ControlSlot, ELEM_CONTROL_BASE, ELEM_SCRUB, ELEM_TAB_BASE};
        use plx_ui::screen::{At, DrawFrame};
        let _g = plx_base::testlock::serial();
        let marker = |kind| Marker { kind, start_ms: 1_000, end_ms: 2_000, final_seg: kind == MarkerKind::Credits };
        let skip = player_hud::slot_for(Some(marker(MarkerKind::Intro)), false, plx_media::route::NextEpisodeMode::Countdown);
        let up_next = player_hud::slot_for(Some(marker(MarkerKind::Credits)), true, plx_media::route::NextEpisodeMode::Countdown);
        assert!(matches!(skip, ControlSlot::Skip(_)) && matches!(up_next, ControlSlot::UpNext(_)));
        let failed = Busy::Readout(plx_ui::widgets::StatusKind::Failed, c"Playback failed");
        let row = |n: u32| (0..n).map(|i| ELEM_CONTROL_BASE + i).collect::<Vec<_>>();
        let cases: [(&str, ControlSlot, Busy, Vec<u32>); 5] = [
            ("discs", ControlSlot::Discs(player_hud::Controls::NONE), Busy::None, [(2..5).map(|i| ELEM_CONTROL_BASE + i).collect::<Vec<_>>(), vec![ELEM_TAB_BASE]].concat()),
            ("episode", ControlSlot::Discs(player_hud::Controls { previous: true, next: true, restart: false }),
                Busy::None, [row(5), vec![ELEM_TAB_BASE]].concat()),
            ("skip", skip, Busy::None, [row(1), vec![ELEM_TAB_BASE]].concat()),
            ("up next", up_next, Busy::None, [row(2), vec![ELEM_TAB_BASE]].concat()),
            ("failed", ControlSlot::Discs(player_hud::Controls::NONE), failed, Vec::new()),
        ];
        for (name, slot, busy, controls) in cases {
            let mut page = PlayerScreen::new(ENTRY);
            page.slot = slot;
            page.busy = busy;
            page.transport = true;
            // A finite, painted timeline: unknown-duration media deliberately has no pointer stop.
            page.model.duration_ns = 100_000_000_000;
            let cx = cx();
            let mut f = DrawFrame::new(&cx, plx_ui::Painter::root());
            page.record_stops(&mut f, true);
            let mut map = HitMap::new();
            map.fill(f.into_stops());
            map.swap();
            let placed: Vec<_> = controls
                .iter()
                .map(|&elem| {
                    let p = Focusable::<TestHost>::place(&page, &elem, &cx, At::Drawn)
                        .unwrap_or_else(|| panic!("{name}: {elem} is a control the D-pad reaches"));
                    (FocusKey { entry: ENTRY, elem }, p.rect)
                })
                .collect();
            let gaps = pointer_gaps(&mut map, ENTRY, &placed);
            assert!(gaps.is_empty(), "{name}: controls the pointer cannot click:\n{}", gaps.join("\n"));
            if !player_hud::failure_row_drawn(busy) {
                let geometry = page.hud_geometry(&cx);
                assert!(geometry.grab.contains(geometry.playbar.cx(), geometry.playbar.cy()));
                let hit = map.resolve(Some(ENTRY), PointerKind::Click,
                    geometry.playbar.cx(), geometry.playbar.cy(), None);
                assert_eq!(hit.activate.map(|(k, _)| k.elem), Some(ELEM_SCRUB), "{name}: the scrubber");
            }
            // …and a click on each one ACTS, never falling through to the picture's play/pause.
            for &elem in &controls {
                let (handled, reqs) = click(&mut page, elem);
                assert_eq!(handled, Handled::Yes, "{name}: {elem}");
                assert!(!reqs.is_empty(), "{name}: a click on {elem} asked for nothing");
                assert!(!reqs.iter().any(|r| matches!(r, PlayerReq::Transport(_))), "{name}: {elem} -> {reqs:?}");
                assert!(page.hud.until >= 1_000 + input::InfoTimeout::DEFAULT.milliseconds(), "{name}: a click re-arms the linger");
                if player_hud::row_index(elem).is_some() {
                    assert_eq!(page.hud.nav.btn, (elem - ELEM_CONTROL_BASE) as i32, "{name}: the CLICKED item arms");
                    assert_eq!(reqs, vec![PlayerReq::ArmControlRow]);
                }
                if elem == ELEM_TAB_BASE {
                    assert_eq!(reqs, vec![PlayerReq::OpenOverlay(overlay::OverlayKind::Info)], "{name}: Info");
                }
            }
        }
    }

    /// PAUSE/PLAY/PLAYPAUSE all reach the shared `Transport` request unchanged — the port of
    /// `key_pause`/`key_play`'s Route::Player arms.
    #[test]
    fn transport_keys_ask_for_the_shared_toggle() {
        let _g = plx_base::testlock::serial();
        for (wcode, want) in [
            (WCODE_PAUSE, Some(false)),
            (WCODE_PLAY, Some(true)),
            (WCODE_PLAYPAUSE, None),
        ] {
            let mut page = PlayerScreen::new(ENTRY);
            let (handled, reqs) = press(&mut page, 0, wcode, Edge::Down);
            assert_eq!(handled, Handled::Yes);
            assert_eq!(reqs, vec![PlayerReq::Transport(want)], "wcode {wcode}");
        }
    }

    /// STOP and a plain BACK both ask to leave the player — port of the Stop arm and `key_back`'s
    /// Route::Player arm (`exit_player`, unreachable from a host test directly since it pulls in
    /// the Starfish/ACB seam — this is the request it is now reached through).
    #[test]
    fn stop_and_back_both_ask_to_exit() {
        let _g = plx_base::testlock::serial();
        for wcode in [WCODE_STOP, WCODE_BACK] {
            let mut page = PlayerScreen::new(ENTRY);
            let (handled, reqs) = press(&mut page, 0, wcode, Edge::Down);
            assert_eq!(handled, Handled::Yes);
            assert_eq!(reqs, vec![PlayerReq::Exit], "wcode {wcode}");
        }
    }

    /// **BACK over the transport hides it; BACK over a bare picture leaves.** The first press finds
    /// the transport on screen (`visible_at_press`), so it dismisses it the way an UP-hide does —
    /// ring parked on the scrubber, scrub disengaged, no `Exit` — and the film keeps playing. The
    /// second finds nothing on screen and leaves. STOP is not part of this: it always leaves.
    #[test]
    fn back_over_the_transport_hides_it_and_a_second_back_leaves() {
        let _g = plx_base::testlock::serial();
        let mut page = PlayerScreen::new(ENTRY);
        fresh(&mut page);
        page.hud.nav.focus = GROUP_TABS; // parked on the tabs row, to see the ring come home
        let (handled, reqs) = press(&mut page, 0, WCODE_BACK, Edge::Down);
        assert_eq!(handled, Handled::Yes);
        assert!(reqs.is_empty(), "the first BACK only hides the transport: {reqs:?}");
        assert!(page.hud.dismissed, "the transport is dismissed");
        assert_eq!(page.hud.nav.focus, GROUP_SCRUB, "ring parked on the scrubber");
        assert_eq!(page.scrub.ns, -1, "no scrub target left behind");
        // A bound key un-dismisses; BACK itself is bound, so the second press must sample the
        // dismissed state BEFORE that, which is what `visible_at_press` is for.
        page.hud.note_fresh_press(TestHost::session(&cx()), 1_000, false);
        assert!(!page.hud.visible_at_press, "a hand-hidden transport is not on screen at the press");
        let (_, reqs) = press(&mut page, 0, WCODE_BACK, Edge::Down);
        assert_eq!(reqs, vec![PlayerReq::Exit], "the second BACK leaves");
    }

    /// **BACK leaves a stalled load, on the second press.** While the pipeline is busy the transport
    /// cannot be hidden (`hud_visible` keeps it up), so a first load that stalls with no picture and
    /// no error would be a trap if BACK could only ever hide: every press would find the transport
    /// up. The first BACK hides it (latched, and it ARMS the exit); the second leaves.
    #[test]
    fn back_leaves_a_stalled_load_on_the_second_press() {
        use plx_media::player::PlaybackState::{Buffering, Resolving};
        let _g = plx_base::testlock::serial();
        for busy in [Resolving, Buffering] {
            let was = plx_media::player::swap_state_for_test(busy);
            let mut page = PlayerScreen::new(ENTRY);
            let (handled, first) = bound_press(&mut page, 0, WCODE_BACK);
            let latched = (page.hud.dismissed, page.hud.exit_armed);
            let (_, second) = bound_press(&mut page, 0, WCODE_BACK);
            plx_media::player::restore_state_for_test(was);
            assert_eq!(handled, Handled::Yes);
            assert_eq!(first, vec![], "the first BACK over a {busy:?} load only hides the transport");
            assert_eq!(latched, (true, true), "{busy:?}: the hide is latched and arms the exit");
            assert_eq!(second, vec![PlayerReq::Exit], "the second BACK over a {busy:?} load leaves");
        }
    }

    /// A BACK during a mid-film seek (the scrub was committed, so no scrub is live) hides the
    /// transport on the first press; it must not end the film.
    #[test]
    fn back_during_a_seek_hides_rather_than_leaves() {
        let _g = plx_base::testlock::serial();
        let was = plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Seeking);
        let mut page = PlayerScreen::new(ENTRY);
        assert!(!page.scrub.is_live(), "the fixture: a committed seek leaves no live scrub");
        let (handled, reqs) = bound_press(&mut page, 0, WCODE_BACK);
        plx_media::player::restore_state_for_test(was);
        assert_eq!(handled, Handled::Yes);
        assert_eq!(reqs, vec![], "BACK during a seek must not leave the film");
        assert!(page.hud.dismissed, "BACK during a seek latches the hide");
    }

    /// …and the second BACK, with the seek still in flight and the transport still up, leaves: the
    /// viewer who asked twice meant it.
    #[test]
    fn a_second_back_during_a_seek_leaves() {
        let _g = plx_base::testlock::serial();
        let was = plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Seeking);
        let mut page = PlayerScreen::new(ENTRY);
        let (_, first) = bound_press(&mut page, 0, WCODE_BACK);
        let (_, second) = bound_press(&mut page, 0, WCODE_BACK);
        plx_media::player::restore_state_for_test(was);
        assert_eq!(first, vec![], "the first BACK only hides");
        assert_eq!(second, vec![PlayerReq::Exit], "the second BACK leaves");
    }

    /// A BACK-hide in normal play does NOT arm the exit: a rebuffer that pops the transport back up
    /// afterwards still costs two presses, because that viewer has not asked to leave.
    #[test]
    fn a_back_hide_in_play_does_not_arm_the_exit_for_a_later_rebuffer() {
        use plx_media::player::PlaybackState::{Buffering, Playing};
        let _g = plx_base::testlock::serial();
        let was = plx_media::player::swap_state_for_test(Playing);
        let mut page = PlayerScreen::new(ENTRY);
        page.hud.until = 5_000; // the transport is up and lingering
        let (_, hide) = bound_press(&mut page, 0, WCODE_BACK);
        let hidden = (page.hud.dismissed, page.hud.exit_armed);
        plx_media::player::swap_state_for_test(Buffering);
        let (_, one) = bound_press(&mut page, 0, WCODE_BACK);
        let (_, two) = bound_press(&mut page, 0, WCODE_BACK);
        plx_media::player::restore_state_for_test(was);
        assert_eq!(hide, vec![], "the first BACK hides the transport");
        assert_eq!(hidden, (true, false), "…and, with nothing busy, arms nothing");
        assert_eq!(one, vec![], "one BACK over the rebuffer hides again");
        assert_eq!(two, vec![PlayerReq::Exit], "the next one leaves");
    }

    /// The arming lasts only as long as the busy window it was made in. A BACK that hid the
    /// transport over a Buffering load armed the exit; the pipeline then settles and nothing is
    /// pressed, so a rebuffer much later is a rebuffer after a hide, which still costs two presses
    /// (the rule `a_back_hide_in_play_does_not_arm_the_exit_for_a_later_rebuffer` holds for a hide
    /// in play). The next Tick over a settled pipeline drops the arming; it leaves the hide itself
    /// alone.
    #[test]
    fn a_busy_hide_stops_arming_the_exit_once_the_pipeline_settles() {
        use plx_media::player::PlaybackState::{Buffering, Playing};
        let _g = plx_base::testlock::serial();
        let was = plx_media::player::swap_state_for_test(Buffering);
        let mut page = PlayerScreen::new(ENTRY);
        let (_, hide) = bound_press(&mut page, 0, WCODE_BACK);
        let armed = (page.hud.dismissed, page.hud.exit_armed);
        plx_media::player::swap_state_for_test(Playing);
        tick(&mut page, 2_000);
        let settled = (page.hud.dismissed, page.hud.exit_armed);
        plx_media::player::swap_state_for_test(Buffering);
        let (_, one) = bound_press(&mut page, 0, WCODE_BACK);
        let (_, two) = bound_press(&mut page, 0, WCODE_BACK);
        plx_media::player::restore_state_for_test(was);
        assert_eq!(hide, vec![], "the first BACK over a Buffering load only hides the transport");
        assert_eq!(armed, (true, true), "the fixture: that busy hide armed the exit");
        assert_eq!(settled, (true, false), "a Tick over a settled pipeline drops the arming, not the hide");
        assert_eq!(one, vec![], "one BACK over the later rebuffer does not leave");
        assert_eq!(two, vec![PlayerReq::Exit], "the next one leaves");
    }

    /// An UP-hide is a hide that arms nothing either: UP from the control row, then a rebuffer, and
    /// one BACK still only hides.
    #[test]
    fn an_up_hide_does_not_arm_the_exit_for_a_later_rebuffer() {
        use plx_media::player::PlaybackState::{Buffering, Playing};
        let _g = plx_base::testlock::serial();
        let was = plx_media::player::swap_state_for_test(Playing);
        let mut page = PlayerScreen::new(ENTRY);
        page.hud.until = 5_000;
        page.hud.nav.focus = GROUP_ROW; // on the control row, where UP hides
        let mut rig = Rig::on(&page, ENTRY);
        bound_travel(&mut rig, &mut page, SDLK_UP);
        let hidden = (page.hud.dismissed, page.hud.exit_armed);
        plx_media::player::swap_state_for_test(Buffering);
        let (_, one) = bound_press(&mut page, 0, WCODE_BACK);
        plx_media::player::restore_state_for_test(was);
        assert_eq!(hidden, (true, false), "the fixture: UP hid the transport and armed nothing");
        assert_eq!(one, vec![], "one BACK over the rebuffer does not leave");
    }

    /// …and an UP-hide done WHILE the pipeline is busy arms nothing either: only a BACK-hide is the
    /// viewer asking to leave. The test above hides in `Playing`, so it could not tell the UP-hide
    /// (`answer_edge`) passing `false` to `HudState::hide` from passing the live `loading()`; this
    /// one can.
    #[test]
    fn an_up_hide_while_busy_does_not_arm_the_exit() {
        use plx_media::player::PlaybackState::Buffering;
        let _g = plx_base::testlock::serial();
        let was = plx_media::player::swap_state_for_test(Buffering);
        let mut page = PlayerScreen::new(ENTRY);
        page.hud.nav.focus = GROUP_ROW; // on the control row, where UP hides
        let mut rig = Rig::on(&page, ENTRY);
        bound_travel(&mut rig, &mut page, SDLK_UP);
        let hidden = (page.hud.dismissed, page.hud.exit_armed);
        let (_, one) = bound_press(&mut page, 0, WCODE_BACK);
        plx_media::player::restore_state_for_test(was);
        assert_eq!(hidden, (true, false), "the fixture: UP hid the transport over a Buffering load and armed nothing");
        assert_eq!(one, vec![], "one BACK after an UP-hide over a busy load does not leave");
    }

    /// Any other bound press between the two BACKs spends the arming: BACK, DOWN, BACK is a hide, a
    /// move and another hide — not a request to leave.
    #[test]
    fn another_bound_press_between_the_backs_disarms_the_exit() {
        let _g = plx_base::testlock::serial();
        let was = plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Buffering);
        let mut page = PlayerScreen::new(ENTRY);
        let (_, first) = bound_press(&mut page, 0, WCODE_BACK);
        bound_press(&mut page, SDLK_DOWN, 0);
        // DOWN walks the ring; it must not have started a scrub, whose own BACK arm would then
        // swallow the next press and make this test pass without the disarm.
        let scrubbing = page.scrub.is_live();
        let (_, third) = bound_press(&mut page, 0, WCODE_BACK);
        let (_, fourth) = bound_press(&mut page, 0, WCODE_BACK);
        plx_media::player::restore_state_for_test(was);
        assert_eq!(first, vec![]);
        assert!(!scrubbing, "the fixture: DOWN started no scrub");
        assert_eq!(third, vec![], "a BACK after another key is a fresh first BACK");
        assert_eq!(fourth, vec![PlayerReq::Exit], "…and it re-arms the exit for the next one");
    }

    /// Anything that un-dismisses the transport between the two BACKs disarms the exit too: a
    /// segment offer raising it (`raise_for_offer`) leaves the next BACK a first BACK again.
    #[test]
    fn a_segment_offer_between_the_backs_disarms_the_exit() {
        let _g = plx_base::testlock::serial();
        let was = plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Buffering);
        let mut page = PlayerScreen::new(ENTRY);
        let (_, first) = bound_press(&mut page, 0, WCODE_BACK);
        page.hud.raise_for_offer(1_000, 1);
        let (_, second) = bound_press(&mut page, 0, WCODE_BACK);
        plx_media::player::restore_state_for_test(was);
        assert_eq!(first, vec![]);
        assert_eq!(second, vec![], "an offer put the transport back: this BACK is a first one");
    }

    /// STOP leaves even with the transport up, and BACK's key-up edge does nothing either way.
    #[test]
    fn stop_still_leaves_over_the_transport_and_back_acts_on_the_down_edge_only() {
        let _g = plx_base::testlock::serial();
        let mut page = PlayerScreen::new(ENTRY);
        fresh(&mut page);
        let (_, reqs) = press(&mut page, 0, WCODE_STOP, Edge::Down);
        assert_eq!(reqs, vec![PlayerReq::Exit], "STOP over the transport");
        let mut page = PlayerScreen::new(ENTRY);
        fresh(&mut page);
        let (_, reqs) = press(&mut page, 0, WCODE_BACK, Edge::Up);
        assert!(reqs.is_empty() && !page.hud.dismissed, "the release edge neither hides nor leaves");
    }

    /// Port of `key_player_updown`: from the scrubber, UP walks to the control row and DOWN to the
    /// tabs; from either, the return trip goes back to the scrubber.
    #[test]
    fn updown_walks_the_huds_three_rows() {
        let _g = plx_base::testlock::serial();
        let mut page = PlayerScreen::new(ENTRY);
        fresh(&mut page);
        let mut rig = Rig::on(&page, ENTRY);
        assert_eq!(page.hud.nav.focus, GROUP_SCRUB);
        rig.key(&mut page, SDLK_UP, 0, Edge::Down, 1_000);
        assert_eq!(page.hud.nav.focus, GROUP_ROW, "scrubber -> control row");
        rig.key(&mut page, SDLK_UP, 0, Edge::Down, 1_000);
        assert_eq!(page.hud.nav.focus, GROUP_SCRUB, "control row has nothing above: hides+reparks");
        assert_eq!(rig.current(), Some(player_hud::ELEM_SCRUB), "…and the engine is home too");
        rig.key(&mut page, SDLK_DOWN, 0, Edge::Down, 1_000);
        assert_eq!(page.hud.nav.focus, GROUP_TABS, "scrubber -> tabs");
        rig.key(&mut page, SDLK_DOWN, 0, Edge::Down, 1_000);
        assert_eq!(page.hud.nav.focus, GROUP_TABS, "tabs has nothing below: stays");
        rig.key(&mut page, SDLK_UP, 0, Edge::Down, 1_000);
        assert_eq!(page.hud.nav.focus, GROUP_SCRUB, "tabs -> scrubber");
    }

    /// OK on the control row is the ENGINE's press, not the page's: the key is left unhandled so the
    /// dispatcher arms the dip on the focused control, and the commit — delivered once the dip has
    /// played — asks the app for that control's command. The page asks for nothing at key-down.
    #[test]
    fn ok_on_the_control_row_leaves_the_press_to_the_engine() {
        let _g = plx_base::testlock::serial();
        let mut page = PlayerScreen::new(ENTRY);
        fresh(&mut page);
        page.hud.nav.focus = GROUP_ROW;
        let mut rig = Rig::on(&page, ENTRY);
        let (handled, reqs) = rig.key(&mut page, SDLK_RETURN, 0, Edge::Down, 1_000);
        assert_eq!(handled, Handled::No);
        assert!(reqs.is_empty(), "key-down asks for nothing: {reqs:?}");
        let (_, reqs) = rig.deliver(&mut page, ScreenEvent::PressCommit(plx_machine::machine::PressId(1)));
        let ordinal = rig.current().and_then(player_hud::row_index).expect("the engine is on a control");
        assert_eq!(page.hud.nav.btn, ordinal, "the commit seats the cursor where the engine is");
        assert_eq!(reqs, vec![PlayerReq::Command(page.slot.command(ordinal).expect("a disc"))]);
    }

    /// **Up Next's two keys are fixed control ids**: `watch_credits` and `play_next` are the
    /// row's elements 0 and 1, the engine presses either one, and a stand-in's press is the app's
    /// to interpret ([`PlayerReq::StandIn`]) with the cursor seated on the pressed key — which is
    /// what `countdown_may_run` reads.
    #[test]
    fn up_next_presses_its_two_fixed_ids_and_seats_the_cursor_on_the_pressed_one() {
        use plx_appkit::up_next;
        use plx_data::metadata::{Marker, MarkerKind};
        let _g = plx_base::testlock::serial();
        let credits = Marker { kind: MarkerKind::Credits, start_ms: 1_000, end_ms: 2_000, final_seg: true };
        for (id, ordinal) in [("watch_credits", up_next::BTN_CREDITS), ("play_next", up_next::BTN_NEXT)] {
            let mut page = PlayerScreen::new(ENTRY);
            fresh(&mut page);
            page.slot = player_hud::slot_for(Some(credits), true, plx_media::route::NextEpisodeMode::Countdown);
            assert!(matches!(page.slot, ControlSlot::UpNext(_)));
            page.hud.nav.focus = GROUP_ROW;
            page.hud.nav.btn = up_next::PRIMARY_BTN;
            let mut rig = Rig::on(&page, ENTRY);
            let group = page.hud.nav.focus;
            rig.reseat(&mut page, group);
            rig.seat_on(player_hud::control_elem(ordinal));
            let (handled, reqs) = rig.key(&mut page, SDLK_RETURN, 0, Edge::Down, 1_000);
            assert_eq!(handled, Handled::No, "{id}: OK is the engine's press");
            assert!(reqs.is_empty(), "{id}: nothing is asked at key-down");
            let (_, reqs) = rig.deliver(&mut page, ScreenEvent::PressCommit(plx_machine::machine::PressId(1)));
            assert_eq!(reqs, vec![PlayerReq::StandIn], "{id}");
            assert_eq!(page.hud.nav.btn, ordinal, "{id}: the cursor is on the pressed key");
        }
    }

    /// A commit that finds the engine no longer on an enabled control of the row asks for nothing:
    /// the row changed under the press (a disc that has since lost its stop).
    #[test]
    fn a_press_that_commits_after_its_control_went_away_asks_for_nothing() {
        let _g = plx_base::testlock::serial();
        let mut page = PlayerScreen::new(ENTRY);
        fresh(&mut page);
        page.slot = ControlSlot::Discs(player_hud::Controls::NONE);
        page.hud.nav.focus = GROUP_ROW;
        let mut rig = Rig::on(&page, ENTRY);
        rig.seat_on(player_hud::control_elem(player_hud::BTN_PREVIOUS));
        let (_, reqs) = rig.deliver(&mut page, ScreenEvent::PressCommit(plx_machine::machine::PressId(1)));
        assert!(reqs.is_empty(), "{reqs:?}");
        let (_, reqs) = rig.deliver(&mut page, ScreenEvent::PressCommit(plx_machine::machine::PressId(2)));
        assert!(reqs.is_empty(), "{reqs:?}");
    }

    /// OK on a seek control continues the relative-seek burst; every other key (and OK on any other
    /// control) ends it.
    #[test]
    fn only_ok_on_a_seek_control_continues_the_seek_burst() {
        use player_hud::command::{HudCommand, SeekBy, SeekDir};
        let seek = Some(HudCommand::Seek(SeekBy::Setting(SeekDir::Forward)));
        assert!(continues_seek_burst(consts::Key::Ok, seek));
        assert!(!continues_seek_burst(consts::Key::Up, seek));
        assert!(!continues_seek_burst(consts::Key::Ok, Some(HudCommand::Next)));
        assert!(!continues_seek_burst(consts::Key::Ok, None));
    }

    #[test]
    fn live_catalog_ok_yields_to_focused_audio_subtitles_and_more_controls() {
        use player_hud::{control_elem, Controls, ControlSlot, BTN_AUDIO, BTN_MORE, BTN_SUBTITLES};
        use crate::registry::OttPlayerAction;
        let slot = ControlSlot::Discs(Controls::NONE);
        for index in [BTN_AUDIO, BTN_SUBTITLES, BTN_MORE] {
            assert!(
                actionable_hud_target_focused(slot, Some(control_elem(index))),
                "focused actionable control {index} must retain OK"
            );
        }
        assert!(!actionable_hud_target_focused(slot, None));
        assert!(!actionable_hud_target_focused(slot, Some(player_hud::ELEM_SCRUB)));
        for tab in [player_hud::ELEM_TAB_BASE, player_hud::ELEM_TAB_BASE + 1] {
            let tab_focused = actionable_hud_target_focused(slot, Some(tab));
            assert!(tab_focused, "focused tab {tab} is actionable");
            assert!(!live_catalog_shortcut_on_ok(
                consts::Key::Ok, true, OttPlayerAction::OpenCatalog, true, tab_focused,
            ), "visible focused tab {tab} owns OK");
        }

        let focused_audio = actionable_hud_target_focused(slot, Some(control_elem(BTN_AUDIO)));
        assert!(!live_catalog_shortcut_on_ok(
            consts::Key::Ok, true, OttPlayerAction::OpenCatalog, true, focused_audio,
        ), "visible focused control owns OK");
        assert!(live_catalog_shortcut_on_ok(
            consts::Key::Ok, true, OttPlayerAction::OpenCatalog, false, focused_audio,
        ), "hidden HUD leaves OK available to open the catalogue");
        assert!(live_catalog_shortcut_on_ok(
            consts::Key::Ok, true, OttPlayerAction::OpenCatalog, true, false,
        ), "OK opens the catalogue without a focused actionable control");
    }

    /// A segment offer moves the ring (`PlayerScreen::raise_for_offer` writes `nav`, with no effect to
    /// carry), so the app that calls it re-seats the engine afterwards. Until it does the engine is
    /// still on the scrubber, and OK there toggles play/pause instead of arming the offered pill.
    #[test]
    fn a_segment_offer_hands_ok_to_the_engine_only_once_it_is_re_seated() {
        let _g = plx_base::testlock::serial();
        let mut page = PlayerScreen::new(ENTRY);
        fresh(&mut page);
        let mut rig = Rig::on(&page, ENTRY);
        page.raise_for_offer(1_100, 0);
        assert_eq!(page.hud.nav.focus, GROUP_ROW, "the offer raised the ring onto the row");
        assert_eq!(rig.current(), Some(player_hud::ELEM_SCRUB), "the engine has not followed by itself");
        let group = page.hud.nav.focus;
        rig.reseat(&mut page, group);
        assert_eq!(
            rig.current(),
            Some(player_hud::ELEM_CONTROL_BASE + page.slot.primary_btn() as u32),
            "the app's re-seat lands it on the row's primary button"
        );
        let (handled, _) = rig.key(&mut page, SDLK_RETURN, 0, Edge::Down, 1_200);
        assert_eq!(handled, Handled::No, "OK on the row is the engine's press, not a play/pause toggle");
    }

    /// OK on the tabs row opens the named overlay AT ONCE — port of `key_ok`'s `focus == 2` arm,
    /// now read off the element the engine holds.
    #[test]
    fn ok_on_the_tabs_row_opens_the_named_overlay() {
        let _g = plx_base::testlock::serial();
        let mut page = PlayerScreen::new(ENTRY);
        fresh(&mut page);
        page.hud.nav.focus = GROUP_TABS;
        page.hud.nav.tab = 0;
        let mut rig = Rig::on(&page, ENTRY);
        let (_, reqs) = rig.key(&mut page, SDLK_RETURN, 0, Edge::Down, 1_000);
        assert_eq!(
            reqs,
            vec![PlayerReq::OpenOverlay(overlay::OverlayKind::Info)]
        );
    }

    /// OK anywhere else (the scrubber, or a hidden transport) is a plain play/pause toggle —
    /// port of `key_ok`'s final `else` arm.
    #[test]
    fn ok_elsewhere_toggles_play_pause() {
        let _g = plx_base::testlock::serial();
        let mut page = PlayerScreen::new(ENTRY);
        fresh(&mut page);
        page.hud.nav.focus = GROUP_SCRUB;
        let mut rig = Rig::on(&page, ENTRY);
        let (_, reqs) = rig.key(&mut page, SDLK_RETURN, 0, Edge::Down, 1_000);
        assert_eq!(reqs, vec![PlayerReq::Transport(None)]);
    }

    /// A click on a registered control-row stop seats the ring on that button and asks the same
    /// deferred press OK would — the pointer twin of `ok_on_the_control_row_leaves_the_press_to_the_engine`.
    #[test]
    fn a_click_on_a_disc_seats_the_ring_and_arms_the_press() {
        let _g = plx_base::testlock::serial();
        let mut page = PlayerScreen::new(ENTRY);
        let (handled, reqs) = click(&mut page, player_hud::ELEM_CONTROL_BASE + player_hud::BTN_AUDIO as u32);
        assert_eq!(handled, Handled::Yes);
        assert_eq!(page.hud.nav.focus, GROUP_ROW);
        assert_eq!(page.hud.nav.btn, player_hud::BTN_AUDIO);
        assert_eq!(reqs, vec![PlayerReq::ArmControlRow]);
    }

    /// The scrubber's registered `Stop` is the exact band `scrub_hit` tests, so a click resolved
    /// by the dispatcher's hit map lands on the same geometry the old bare pointer path did.
    #[test]
    fn the_scrubber_group_places_at_the_hit_testers_own_band() {
        let _g = plx_base::testlock::serial();
        let page = PlayerScreen::new(ENTRY);
        let placed = Focusable::<TestHost>::place(
            &page,
            &player_hud::ELEM_SCRUB,
            &cx(),
            At::Drawn,
        )
        .expect("the scrubber always places");
        let want = player_hud::classic::frame_geometry().grab;
        assert_eq!((placed.rect.x, placed.rect.y, placed.rect.w, placed.rect.h), (want.x, want.y, want.w, want.h));
    }

    /// Every group this screen declares resolves back through `group_of` to the group it came
    /// from — the property `Focusable`'s two queries must agree on for the engine's own
    /// bookkeeping to be internally consistent.
    #[test]
    fn every_declared_group_round_trips_through_group_of() {
        let _g = plx_base::testlock::serial();
        let page = PlayerScreen::new(ENTRY);
        let mut groups = Vec::new();
        Focusable::<TestHost>::groups(&page, &cx(), &mut groups);
        assert!(groups.len() >= 3, "scrubber, control row, tabs at least");
        for g in &groups {
            let seated = Focusable::<TestHost>::seat(
                &page,
                g.id,
                Placed { rect: Rect::FULL, rest_rect: Rect::FULL, clip: Rect::FULL, index: None },
                &cx(),
            );
            assert_eq!(
                Focusable::<TestHost>::group_of(&page, &seated.elem, &cx()),
                Some(g.id),
                "group {:?}'s own seat must resolve back to it",
                g.id
            );
        }
    }
}

/// **The scrub gesture has exactly ONE owner** (restructure phase 12, PX-PLAYER).
///
/// Phase 12's first swarm made `PlayerScreen` answer `FocusSource::Engine`/`HitSource::Engine`
/// without deleting the loop's own player input path, so two implementations drove one piece of
/// state — `PlayerScreen::scrub` — at the same time. Each case below is one of the three
/// regressions that produced, watched RED against `2790f47a` before the retirement commit:
///
/// 1. [`one_right_tap_moves_the_target_and_ok_is_the_one_seek`] — the step's `Jump` arm committed
///    on the PRESS while `app/input.rs`'s key-up arm armed a tap commit on the same gesture and
///    `app/run.rs`'s per-frame block committed it a second time. (Both committers are gone with
///    click-to-confirm: the screen commits a key target only when OK takes it.)
/// 2. [`a_scrub_seek_taken_while_paused_holds_the_pause`] — the screen asked for
///    [`PlayerReq::SeekTo`], whose loop arm resumes a paused film; a pointer drag's commit must not.
/// 3. [`a_click_on_the_scrubber_seeks_and_a_drag_previews_before_it`] — the scrubber's registered
///    stop landed on the play/pause arm, and the drag preview had lost its producer entirely.
///
/// **Where the loop is part of the defect it is simulated, minimally and by name.** Case 1's
/// second commit came from `app/run.rs`:1413-1426, which no host test can call; what is graded
/// here instead is the contract that removes it — the screen is the only thing that commits a key
/// target, and it commits it when OK takes it, not on the press, the release or a timer. A second
/// committer on that field can then only be a second seek, which is the symptom.
#[cfg(test)]
mod scrub_ownership_tests {
    use super::travel_rig::Rig;
    use super::*;
    use crate::player::overlay_tests::TestHost;
    use crate::registry::AppFx;
    use plx_ui::consts::{SDLK_RETURN, SDLK_RIGHT, WCODE_BACK, WCODE_EXIT};
    use plx_ui::fixture::FixtureMeasure;
    use plx_machine::machine::{
        Cx, Edge, Effects, EntryId, Fx, Handled, InputEvent, InputKind, InputOwner, MachineId,
        Source, Tick,
    };

    const ENTRY: EntryId = EntryId(9);
    const INST: InstanceId = InstanceId(3);
    /// 100 s, so the 3 s tail cap (`duration - 3s`) is far from every position these cases name.
    const DUR: i64 = 100_000_000_000;
    /// What one press hops at the default Skip interval — a literal, so the default stays pinned
    /// at the 10 s every build before the preference made.
    const DEFAULT_HOP: i64 = 10_000_000_000;

    fn cx() -> Cx<'static, TestHost> {
        Cx {
            views: (),
            tick: Tick::default(),
            measure: &FixtureMeasure,
            focus: Default::default(),
            press: Default::default(),
            owner: InputOwner::Entry(ENTRY),
        }
    }

    /// The playback globals these cases read, restored on the way out. Held together with
    /// `testlock::serial()` by every caller: `duration_ns`/`playpos_ns`/`paused` are crate-wide.
    struct Fixture(i64, i64, bool);
    impl Fixture {
        fn new(paused: bool) -> Self {
            use std::sync::atomic::Ordering::Relaxed;
            let was = Fixture(
                plx_media::player::SHARED.duration_ns.load(Relaxed),
                plx_media::player::SHARED.playpos_ns.load(Relaxed),
                plx_media::player::TX.paused.load(Relaxed),
            );
            plx_media::player::SHARED.duration_ns.store(DUR, Relaxed);
            plx_media::player::SHARED.playpos_ns.store(0, Relaxed);
            plx_media::player::TX.paused.store(paused, Relaxed);
            was
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            use std::sync::atomic::Ordering::Relaxed;
            plx_media::player::SHARED.duration_ns.store(self.0, Relaxed);
            plx_media::player::SHARED.playpos_ns.store(self.1, Relaxed);
            plx_media::player::TX.paused.store(self.2, Relaxed);
        }
    }

    /// A page with the transport ON SCREEN and the ring on the scrubber — the state every gesture
    /// below begins from.
    fn page_on_the_bar() -> PlayerScreen {
        let mut page = PlayerScreen::new(ENTRY);
        page.hud.visible_at_press = true;
        page.hud.nav.focus = GROUP_SCRUB;
        // The app supplies this per-frame snapshot before rendering/input. Setting only the
        // decoder globals leaves the painted timeline unknown and correctly unclickable.
        page.model = HudModel {
            duration_ns: plx_media::player::duration_ns(),
            playpos_ns: plx_media::player::playpos_ns(),
            paused: plx_media::player::TX.paused.load(std::sync::atomic::Ordering::Relaxed),
            ..HudModel::default()
        };
        assert_eq!(page.model.duration_ns, DUR);
        page
    }

    fn run(page: &mut PlayerScreen, ev: ScreenEvent<TestHost>) -> (Handled, Vec<PlayerReq>) {
        let mut out = Vec::new();
        let mut present = plx_machine::present::Present::new();
        let handled = page.step(
            &ev,
            &cx(),
            &mut Effects::new(&mut out, MachineId::Instance(INST), &mut present),
        );
        let reqs = out
            .into_iter()
            .filter_map(|e| match e.fx {
                Fx::App(AppFx::Player(req)) => Some(req),
                _ => None,
            })
            .collect();
        (handled, reqs)
    }

    fn key(page: &mut PlayerScreen, sym: u32, edge: Edge, ms: u32) -> (Handled, Vec<PlayerReq>) {
        key_w(page, sym, 0, edge, ms)
    }

    /// …and the same with a `wcode`, which is the only field BACK, STOP and EXIT arrive in.
    fn key_w(
        page: &mut PlayerScreen,
        sym: u32,
        wcode: u32,
        edge: Edge,
        ms: u32,
    ) -> (Handled, Vec<PlayerReq>) {
        run(
            page,
            ScreenEvent::Input(InputEvent {
                kind: InputKind::Key {
                    key: plx_machine::machine::Key::Other,
                    sym,
                    wcode,
                    edge,
                    at_edge: false,
                },
                at: Tick { ms, dt_us: 0 },
                source: Source::Sdl,
            }),
        )
    }

    fn pointer(
        page: &mut PlayerScreen,
        kind: InputKind<u32>,
        ms: u32,
    ) -> (Handled, Vec<PlayerReq>) {
        run(
            page,
            ScreenEvent::Input(InputEvent { kind, at: Tick { ms, dt_us: 0 }, source: Source::Sdl }),
        )
    }

    fn tick(page: &mut PlayerScreen, ms: u32) -> (Handled, Vec<PlayerReq>) {
        run(page, ScreenEvent::Tick(Tick { ms, dt_us: 16_000 }))
    }

    /// Every request that MOVES THE PLAYHEAD, whichever variant carries it — so a case counts
    /// seeks rather than asserting on one spelling of them.
    fn seeks(reqs: Vec<PlayerReq>) -> Vec<PlayerReq> {
        reqs.into_iter()
            .filter(|r| matches!(r, PlayerReq::SeekTo(_) | PlayerReq::CommitSeek(_)))
            .collect()
    }

    /// A point on the current painted rail, proved clickable by the real stop registration.
    fn on_the_bar(page: &PlayerScreen, frac: f32) -> (f32, f32) {
        use plx_ui::hit::{HitMap, PointerKind};
        use plx_ui::screen::DrawFrame;
        let cx = cx();
        let geometry = page.hud_geometry(&cx);
        let point = (geometry.playbar.x + geometry.playbar.w * frac, geometry.playbar.cy());
        assert!(geometry.grab.contains(point.0, point.1));
        let mut frame = DrawFrame::new(&cx, plx_ui::Painter::root());
        page.record_stops(&mut frame, true);
        let mut map = HitMap::new();
        map.fill(frame.into_stops());
        map.swap();
        let hit = map.resolve(Some(ENTRY), PointerKind::Click, point.0, point.1, None);
        assert_eq!(hit.activate.map(|(key, _)| key.elem), Some(player_hud::ELEM_SCRUB),
            "the gesture must begin on a real, presented scrubber");
        point
    }

    #[test]
    fn pointer_requires_presented_duration_even_when_decoder_duration_is_known() {
        use plx_ui::screen::DrawFrame;
        let _g = plx_base::testlock::serial();
        let _f = Fixture::new(false);
        let mut page = page_on_the_bar();
        page.model.duration_ns = 0;
        let cx = cx();
        let mut frame = DrawFrame::new(&cx, plx_ui::Painter::root());
        page.record_stops(&mut frame, true);
        assert!(!frame.into_stops().iter().any(|stop| stop.key.elem == player_hud::ELEM_SCRUB));
        // Even a stale hit from the prior frame must not admit a seek on an unknown timeline.
        let x = page.hud_geometry(&cx).playbar.cx();
        let (_, reqs) = pointer(&mut page,
            InputKind::Click { x, y: 0.0, hit: Some(player_hud::ELEM_SCRUB) }, 1_000);
        assert!(reqs.is_empty());
        assert!(!page.scrub.drag && page.scrub.ns == -1);
        assert!(seeks(key(&mut page, SDLK_RETURN, Edge::Up, 1_080).1).is_empty());
    }

    /// **(a) A tap only MOVES the target; OK is the ONE seek.**
    ///
    /// Click-to-confirm: releasing a key — and the old ~450 ms tap debounce, and every frame after
    /// it — commits nothing, on a playing film exactly as on a paused one. The target stays
    /// pending until OK takes it, so a tap burst is one seek because the viewer said so, not
    /// because a timer coalesced it.
    #[test]
    fn one_right_tap_moves_the_target_and_ok_is_the_one_seek() {
        let _g = plx_base::testlock::serial();
        let _f = Fixture::new(false);
        let mut page = page_on_the_bar();

        let (handled, reqs) = key(&mut page, SDLK_RIGHT, Edge::Down, 1_000);
        assert_eq!(handled, Handled::Yes);
        assert!(seeks(reqs).is_empty(), "the press moves the TARGET and commits nothing");
        assert_eq!(page.scrub.ns, DEFAULT_HOP, "…and the target hopped 10 s");

        let (_, reqs) = key(&mut page, SDLK_RIGHT, Edge::Up, 1_020);
        assert!(seeks(reqs).is_empty(), "the release does not commit");

        let mut all = Vec::new();
        for t in [1_020 + plx_appkit::player_hud::TAP_COMMIT_MS + 1, 3_000, 60_000] {
            all.extend(seeks(tick(&mut page, t).1));
        }
        assert!(all.is_empty(), "no timer commits a key target: {all:?}");
        assert_eq!(page.scrub.ns, DEFAULT_HOP, "the target is still pending");

        let (_, reqs) = key(&mut page, SDLK_RETURN, Edge::Down, 61_000);
        assert_eq!(seeks(reqs), vec![PlayerReq::SeekTo(DEFAULT_HOP)], "OK takes the target: the one seek");
        assert_eq!(page.scrub.ns, -1, "…and the gesture is over");

        let (_, reqs) = tick(&mut page, 62_000);
        assert!(seeks(reqs).is_empty(), "…and it is not repeated on every frame after it");
    }

    /// **A press hops the chosen Skip interval, in both directions, and clamps like any scrub.**
    /// From 50 s of a 100 s film: a 60 s hop forward lands on the 3 s tail cap and one back on 0.
    #[test]
    fn a_press_hops_the_chosen_skip_interval_in_both_directions_and_clamps() {
        use plx_plex::plex::session::SkipInterval;
        use plx_ui::consts::SDLK_LEFT;
        use std::sync::atomic::Ordering::Relaxed;
        let _g = plx_base::testlock::serial();
        let _f = Fixture::new(false);
        const S: i64 = 1_000_000_000;
        plx_media::player::SHARED.playpos_ns.store(50 * S, Relaxed);
        for (interval, fwd, back) in [
            (SkipInterval::Seconds5, 55 * S, 45 * S),
            (SkipInterval::Seconds10, 60 * S, 40 * S),
            (SkipInterval::Seconds15, 65 * S, 35 * S),
            (SkipInterval::Seconds30, 80 * S, 20 * S),
            (SkipInterval::Seconds60, DUR - 3 * S, 0),
        ] {
            let _i = player_hud::SkipIntervalGuard::set(interval);
            for (sym, want) in [(SDLK_RIGHT, fwd), (SDLK_LEFT, back)] {
                let mut page = page_on_the_bar();
                let (handled, reqs) = key(&mut page, sym, Edge::Down, 1_000);
                assert_eq!(handled, Handled::Yes);
                assert!(seeks(reqs).is_empty(), "{interval:?}: the press commits nothing");
                assert_eq!(page.scrub.ns, want, "{interval:?} sym {sym}");
                assert_eq!(
                    want,
                    player_hud::scrub_clamp_target(50 * S + if sym == SDLK_RIGHT { 1 } else { -1 } * interval.ns(), DUR),
                    "{interval:?}: the expectation is the shared clamp of ±interval",
                );
            }
        }
    }

    /// **(b) A pointer drag taken while PAUSED leaves the film paused.**
    ///
    /// [`PlayerReq::SeekTo`] is the Chapters strip's request — seek and RESUME — and it is the
    /// wrong one for a drag: the loop's arm for it calls `resume_if_paused`, so paused scrubbing
    /// started the film. The drag's commit request is [`PlayerReq::CommitSeek`], whose arm is
    /// `app::playback::commit_seek` — a bare seek; the pump opens the seek-preroll override
    /// that decodes the landed frame without publishing a viewer Resume, and re-pauses on landing.
    ///
    /// What commits by itself is the POINTER (a drag's release): the keys only move the target
    /// until OK or PLAY takes it — with `SeekTo`, which plays on from it
    /// (`ok_or_play_commits_the_target_and_the_film_plays_on_from_it`).
    #[test]
    fn a_scrub_seek_taken_while_paused_holds_the_pause() {
        let _g = plx_base::testlock::serial();
        let _f = Fixture::new(true);
        let mut page = page_on_the_bar();

        let (mx, my) = on_the_bar(&page, 0.75);
        pointer(&mut page, InputKind::Click { x: mx, y: my, hit: Some(player_hud::ELEM_SCRUB) }, 1_000);
        let target = page.scrub.ns;
        let (_, reqs) = key(&mut page, SDLK_RETURN, Edge::Up, 1_080);
        assert_eq!(
            seeks(reqs),
            vec![PlayerReq::CommitSeek(target)],
            "a scrub commit must be the request that HOLDS the pause, not the one that resumes",
        );
    }

    /// **A HELD direction runs the ramp; its release — and the `SCRUB_LOST_MS` net when the release
    /// never arrives — ends the KEY gesture and leaves the target pending.** Nothing seeks until
    /// OK takes it.
    ///
    /// This remote emits a held key as auto-repeat keydowns followed by ONE keyup, so a dropped
    /// keyup leaves the `HeldKey` engaged with the preview slewing at up to `SCRUB_MAX` for as long as the
    /// page is up. The net was `app/run.rs`'s and phase 9 did not reproduce it; it is the screen's
    /// `Tick` now, beside the ramp it guards.
    #[test]
    fn a_held_direction_leaves_its_target_pending_on_release_and_on_the_lost_keyup_net() {
        let _g = plx_base::testlock::serial();
        let _f = Fixture::new(false);

        // …the release that does arrive.
        let mut page = page_on_the_bar();
        key(&mut page, SDLK_RIGHT, Edge::Down, 1_000);
        key(&mut page, SDLK_RIGHT, Edge::Repeat, 1_050);
        assert!(page.scrub.key.engaged, "a hardware repeat engages the continuous scrub");
        tick(&mut page, 1_400); // the ramp travels
        let travelled = page.scrub.ns;
        assert!(travelled > DEFAULT_HOP, "the ramp advanced past the press's own hop");
        let (_, reqs) = key(&mut page, SDLK_RIGHT, Edge::Up, 1_420);
        assert!(seeks(reqs).is_empty(), "the release waits out the key-bounce window first");
        let mut all = Vec::new();
        for t in [1_420 + plx_appkit::player_hud::SCRUB_RELEASE_DEBOUNCE_MS + 1, 5_000, 60_000] {
            all.extend(seeks(tick(&mut page, t).1));
        }
        assert!(all.is_empty(), "a hold's release does not seek, now or ever: {all:?}");
        assert_eq!(page.scrub.ns, travelled, "the target waits for OK");
        assert_eq!(page.scrub.key.dir, 0, "the key gesture is over");

        // …and the release that never does.
        let mut page = page_on_the_bar();
        key(&mut page, SDLK_RIGHT, Edge::Down, 1_000);
        key(&mut page, SDLK_RIGHT, Edge::Repeat, 1_050);
        let (_, reqs) = tick(&mut page, 1_050 + plx_appkit::player_hud::SCRUB_LOST_MS / 2);
        assert!(seeks(reqs).is_empty(), "inside the net's window the ramp just runs");
        let (_, reqs) = tick(&mut page, 1_050 + plx_appkit::player_hud::SCRUB_LOST_MS + 1);
        assert!(seeks(reqs).is_empty(), "the repeats stopped without a keyup: let go, seek nothing");
        assert_eq!(page.scrub.key.dir, 0, "…and the gesture is over, not slewing forever");
        assert!(page.scrub.ns >= 0, "…with the target kept");
    }

    /// **A key-up straight followed by a key-down is the SAME hold.** Measured on the television
    /// 2026-10-13: mid-hold the key driver delivered `Up` and then a fresh `Down` 0-17 ms later (a
    /// remote that bounces), the player took the `Up` for the release, committed the seek and
    /// loaded the video while the thumb was still down, and the next repeats only came 500 ms
    /// later. The release is therefore held back for `SCRUB_RELEASE_DEBOUNCE_MS`; a same-direction
    /// press inside it continues the hold — no commit, no hop, the ramp is not restarted — and the
    /// lost-keyup net waits out the repeat delay the re-press restarts.
    #[test]
    fn a_key_bounce_mid_hold_continues_the_hold_instead_of_committing() {
        let _g = plx_base::testlock::serial();
        let _f = Fixture::new(false);
        let mut page = page_on_the_bar();

        key(&mut page, SDLK_RIGHT, Edge::Down, 1_000);
        key(&mut page, SDLK_RIGHT, Edge::Repeat, 1_500);
        let since = page.scrub.key.since;
        key(&mut page, SDLK_RIGHT, Edge::Repeat, 1_950);
        tick(&mut page, 2_000);
        let before = page.scrub.ns;

        // …the bounce: up, then down 1 ms later.
        let (_, reqs) = key(&mut page, SDLK_RIGHT, Edge::Up, 2_050);
        assert!(seeks(reqs).is_empty());
        let (_, reqs) = key(&mut page, SDLK_RIGHT, Edge::Down, 2_051);
        assert!(seeks(reqs).is_empty(), "the re-press is not a hop and not a commit");
        assert!(page.scrub.key.engaged && page.scrub.key.dir == 1, "the hold goes on");
        assert_eq!(page.scrub.key.since, since, "…and the ramp keeps its speed");

        // The repeats do not resume for ~500 ms. The preview keeps moving, and the lost-keyup net
        // (400 ms) must not fire inside the restarted repeat delay.
        let (_, reqs) = tick(&mut page, 2_051 + plx_appkit::player_hud::SCRUB_LOST_MS + 50);
        assert!(seeks(reqs).is_empty(), "the net waits out the restarted repeat delay");
        assert!(page.scrub.ns > before, "the preview kept slewing through the gap");
        key(&mut page, SDLK_RIGHT, Edge::Repeat, 2_560);
        key(&mut page, SDLK_RIGHT, Edge::Repeat, 2_960);
        tick(&mut page, 3_000);
        let travelled = page.scrub.ns;

        // …and the real release, once, after the window: the hold is over and the target waits.
        key(&mut page, SDLK_RIGHT, Edge::Up, 3_020);
        let (_, reqs) = tick(&mut page, 3_020 + plx_appkit::player_hud::SCRUB_RELEASE_DEBOUNCE_MS + 1);
        assert!(seeks(reqs).is_empty(), "the release seeks nothing, however many times the key bounced");
        assert_eq!(page.scrub.key.dir, 0, "the hold is over");
        assert_eq!(page.scrub.ns, travelled, "…and the target is where the hold left it");
        let (_, reqs) = key(&mut page, SDLK_RETURN, Edge::Down, 4_000);
        assert_eq!(seeks(reqs), vec![PlayerReq::SeekTo(travelled)], "OK: ONE seek for the whole hold");
        assert!(travelled > before);
    }

    /// The bounce window is not a licence to wait forever: a re-press that never gets a repeat
    /// still hits the lost-keyup net, just after the restarted repeat delay instead of before it.
    #[test]
    fn a_bounced_hold_whose_repeats_never_resume_still_hits_the_lost_keyup_net() {
        let _g = plx_base::testlock::serial();
        let _f = Fixture::new(false);
        let mut page = page_on_the_bar();

        key(&mut page, SDLK_RIGHT, Edge::Down, 1_000);
        key(&mut page, SDLK_RIGHT, Edge::Repeat, 1_500);
        key(&mut page, SDLK_RIGHT, Edge::Repeat, 1_950);
        tick(&mut page, 2_000);
        key(&mut page, SDLK_RIGHT, Edge::Up, 2_050);
        key(&mut page, SDLK_RIGHT, Edge::Down, 2_051);
        let (_, reqs) = tick(&mut page, 2_051 + plx_appkit::player_hud::SCRUB_REPEAT_RESTART_MS + 1);
        assert!(seeks(reqs).is_empty(), "no repeat ever came: let go, seeking nothing");
        assert_eq!(page.scrub.key.dir, 0);
        assert!(page.scrub.ns >= 0, "the target is kept");
    }

    /// A press in the OTHER direction inside the window is not a bounce of the same key.
    #[test]
    fn an_opposite_press_inside_the_window_is_not_a_bounce() {
        let _g = plx_base::testlock::serial();
        let _f = Fixture::new(false);
        let mut page = page_on_the_bar();

        key(&mut page, SDLK_RIGHT, Edge::Down, 1_000);
        key(&mut page, SDLK_RIGHT, Edge::Repeat, 1_500);
        key(&mut page, SDLK_RIGHT, Edge::Repeat, 1_950);
        tick(&mut page, 2_000);
        key(&mut page, SDLK_RIGHT, Edge::Up, 2_050);
        let before = page.scrub.ns;
        let (_, reqs) = key(&mut page, plx_ui::consts::SDLK_LEFT, Edge::Down, 2_051);
        assert!(seeks(reqs).is_empty(), "the RIGHT hold's release is real, but it seeks nothing");
        assert_eq!(page.scrub.key.dir, -1, "the new direction owns the gesture");
        assert!(!page.scrub.key.engaged, "…as a fresh press, not a continuation");
        assert_eq!(page.scrub.ns, before - DEFAULT_HOP, "…hopping on from the held target");
    }

    /// A key gesture SUPERSEDES a pointer one and throws its preview away rather than hopping from
    /// it — `key_scrub`'s own first act back when the flag was `app::input::Pointer::drag`. The
    /// pointer's release will never reach this gesture, so a preview left standing would be
    /// committed later by a debounce that knows nothing about where it came from.
    #[test]
    fn a_direction_press_during_a_drag_discards_the_drags_preview() {
        let _g = plx_base::testlock::serial();
        let _f = Fixture::new(false);
        let mut page = page_on_the_bar();

        let (mx, my) = on_the_bar(&page, 0.75);
        pointer(
            &mut page,
            InputKind::Click { x: mx, y: my, hit: Some(player_hud::ELEM_SCRUB) },
            1_000,
        );
        assert!(page.scrub.drag && page.scrub.ns > DUR / 2, "the fixture: a drag is under way");

        let (_, reqs) = key(&mut page, SDLK_RIGHT, Edge::Down, 1_100);
        assert!(seeks(reqs).is_empty());
        assert!(!page.scrub.drag, "the pointer no longer owns the gesture");
        assert_eq!(
            page.scrub.ns, DEFAULT_HOP,
            "…and the hop is seeded from the PLAYHEAD, not from the abandoned drag preview",
        );
    }

    /// **A click that lands on NO control toggles play/pause** — the `else` arm of the old pointer
    /// block, and the reason clicking the picture works at all: over full-screen video most of the
    /// frame is not a control. It is the one part of that block that was never about geometry, and
    /// the easiest to lose in a port that dispatches on a resolved element.
    #[test]
    fn a_click_on_the_picture_toggles_play_pause_but_not_over_a_failure() {
        use std::sync::atomic::Ordering::Relaxed;
        let _g = plx_base::testlock::serial();
        let _f = Fixture::new(false);
        let mut page = page_on_the_bar();

        let (handled, reqs) = pointer(
            &mut page,
            InputKind::Click { x: 960.0, y: 400.0, hit: None },
            1_000,
        );
        assert_eq!(handled, Handled::Yes);
        assert_eq!(reqs, vec![PlayerReq::Transport(None)]);

        // …and not while a failure owns the frame: there is nothing to toggle, and the read-out's
        // one drawn target is its own escape.
        let was = plx_media::player::SHARED.pb_state.load(Relaxed);
        plx_media::player::SHARED
            .pb_state
            .store(plx_media::player::PlaybackState::Error as u8, Relaxed);
        let mut page = page_on_the_bar();
        let (handled, reqs) = pointer(
            &mut page,
            InputKind::Click { x: 960.0, y: 400.0, hit: None },
            1_000,
        );
        assert_eq!(handled, Handled::Yes, "swallowed, as the old block's early return did");
        assert!(reqs.is_empty());
        plx_media::player::SHARED.pb_state.store(was, Relaxed);
    }

    /// **The failure read-out's own two escapes, as the page's first test rather than a ladder
    /// arm's height.**
    ///
    /// `key_player_failed` expressed this as PRECEDENCE — its guard sat above every other player
    /// arm in `app/run.rs`'s chain and `return`ed — and by the time phase 12 found it, it had
    /// grown a `!matches!(app.route, Route::Player)` term beside a `Route::Player` test, so it
    /// could never run at all: OK on a failed playback reached `key_ok`'s final `else` and asked
    /// the pipeline to toggle a film that was not playing. `PlayerScreen::handle_key` asks
    /// `transport_hidden` FIRST, which is the same precedence expressed as one condition.
    #[test]
    fn failed_notice_never_owns_ok_and_explicit_back_still_exits() {
        use std::sync::atomic::Ordering::Relaxed;
        let _g = plx_base::testlock::serial();
        let _f = Fixture::new(false);
        let was = plx_media::player::SHARED.pb_state.load(Relaxed);
        plx_media::player::SHARED
            .pb_state
            .store(plx_media::player::PlaybackState::Error as u8, Relaxed);

        // The transient error has no default button. OK must never implicitly
        // leave the failed selected channel or toggle its dead transport.
        let mut page = page_on_the_bar();
        let (handled, reqs) = key(&mut page, SDLK_RETURN, Edge::Down, 1_000);
        assert_eq!(handled, Handled::Yes);
        assert!(reqs.is_empty(), "no implicit Back, retry or dead transport command");
        assert!(!page.repair_alert.is_open());

        let mut page = page_on_the_bar();
        let (handled, reqs) = key_w(&mut page, 0, WCODE_BACK, Edge::Down, 1_000);
        assert_eq!(handled, Handled::Yes);
        assert_eq!(reqs, vec![PlayerReq::Exit], "…and BACK is the way out of it");

        let mut page = page_on_the_bar();
        let (handled, reqs) = key_w(&mut page, SDLK_RIGHT, 0, Edge::Down, 1_000);
        assert_eq!(handled, Handled::Yes, "RIGHT cannot focus or activate a removed error button");
        assert!(reqs.is_empty());
        assert_eq!(page.scrub.ns, -1, "…and nothing that is not drawn was driven");

        // …but EXIT is the remote's own and ends the PROCESS, so it is not this screen's to
        // swallow: a failed playback would otherwise be the one screen an EXIT press cannot
        // leave (LG checklist item 38).
        let mut page = page_on_the_bar();
        let (handled, reqs) = key_w(&mut page, 0, WCODE_EXIT, Edge::Down, 1_000);
        assert_eq!(handled, Handled::No, "EXIT falls through to the loop's own arm");
        assert!(reqs.is_empty());

        plx_media::player::SHARED.pb_state.store(was, Relaxed);
    }

    /// **The failure table never offers more controls than the read-out's row has slots.** The
    /// table (`player::failure_actions`) is the player's and the row (`STATUS_ROW_MAX` slots, the
    /// `ELEM_FAILURE_BASE..ELEM_FAILURE_END` elements) is the UI's, so the one test that holds the
    /// two to each other lives where both can be named. Over every kind x every context, like the
    /// table's own property test.
    #[test]
    fn the_failure_table_never_outgrows_the_read_outs_row() {
        for (kind, cx, row) in plx_media::player::every_failure_row() {
            assert!(
                row.len() <= plx_ui::widgets::STATUS_ROW_MAX,
                "{kind:?} {cx:?}: {row:?}"
            );
        }
    }

    /// **The chrome a live scrub fades away takes no pointer.** The control row and the bottom tabs
    /// are not registered as stops from the moment a scrub starts until they are back (an invisible
    /// disc must neither highlight nor act under the card), while the scrubber's own stop stays.
    #[test]
    fn faded_chrome_is_not_hit_registered_during_a_scrub() {
        use std::sync::atomic::Ordering::Relaxed;
        use plx_ui::screen::DrawFrame;
        let _g = plx_base::testlock::serial();
        let _f = Fixture::new(false);
        let was = plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Playing);
        plx_media::player::TX.scrub_ns.store(-1, Relaxed);
        let elems = |page: &PlayerScreen| {
            let cx = cx();
            let mut f = DrawFrame::new(&cx, plx_ui::Painter::root());
            page.record_stops(&mut f, true);
            let mut v: Vec<u32> = f.into_stops().iter().map(|s| s.key.elem).collect();
            v.sort_unstable();
            v
        };
        let step = |page: &mut PlayerScreen, now: u32| page.row.step(ControlSlot::Discs(player_hud::Controls::NONE), GROUP_SCRUB, 0, 0.016, now);
        let mut page = page_on_the_bar();
        page.transport = true;
        step(&mut page, 1_000);
        let before = elems(&page);
        assert!(before.contains(&(player_hud::ELEM_CONTROL_BASE + player_hud::BTN_SUBTITLES as u32)) && before.contains(&player_hud::ELEM_TAB_BASE));
        assert!(before.contains(&player_hud::ELEM_SCRUB));

        plx_media::player::TX.scrub_ns.store(60_000_000_000, Relaxed);
        step(&mut page, 1_016);
        let during = elems(&page);
        assert_eq!(during, vec![player_hud::ELEM_SCRUB], "only the bar takes the pointer: {during:?}");

        plx_media::player::TX.scrub_ns.store(-1, Relaxed);
        step(&mut page, 2_000);
        assert_eq!(elems(&page), vec![player_hud::ELEM_SCRUB], "still hidden in the 150 ms wait");
        step(&mut page, 2_300);
        assert_eq!(elems(&page), before, "back, and clickable again");
        plx_media::player::restore_state_for_test(was);
    }

    /// **A reveal tap is not a scrub, so it does not blink the chrome.** LEFT/RIGHT on a HIDDEN
    /// transport only raises it (`ScrubPress::Reveal`): the gesture is armed with its preview on
    /// the playhead, but nothing has travelled, and what is published for the draw path
    /// (`TX.scrub_ns`) is the signal that fades the title, control row and tabs out for a live
    /// scrub. It must stay `-1` until the hold has really moved the preview.
    #[test]
    fn a_reveal_tap_does_not_publish_a_live_scrub() {
        use std::sync::atomic::Ordering::Relaxed;
        let _g = plx_base::testlock::serial();
        let _f = Fixture::new(false);
        let was = plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Playing);
        playhead(50 * S);
        let mut page = PlayerScreen::new(ENTRY);
        page.transport = true;
        assert!(!page.hud.visible_at_press, "the fixture: the transport is hidden");
        let step = |page: &mut PlayerScreen, now: u32| page.row.step(ControlSlot::Discs(player_hud::Controls::NONE), GROUP_SCRUB, 0, 0.016, now);

        key(&mut page, SDLK_RIGHT, Edge::Down, 1_000);
        page.publish();
        assert!(page.scrub.reveal && page.scrub.ns >= 0, "the fixture: a reveal is armed on the seed");
        assert_eq!(plx_media::player::TX.scrub_ns.load(Relaxed), -1, "…but no scrub is published");
        for t in [1_000, 1_016, 1_100, 1_300] {
            step(&mut page, t);
            assert!(page.row.chrome_interactive(), "{t} ms: the chrome stays up, unfaded, and takes the pointer");
        }

        // the key comes up: still nothing, and the gesture is retired without a seek
        let (_, reqs) = key(&mut page, SDLK_RIGHT, Edge::Up, 1_320);
        page.publish();
        assert!(seeks(reqs).is_empty());
        assert_eq!(plx_media::player::TX.scrub_ns.load(Relaxed), -1);

        // …whereas a hold that really TRAVELS is a scrub, and is published as one
        let mut page = PlayerScreen::new(ENTRY);
        page.transport = true;
        key(&mut page, SDLK_RIGHT, Edge::Down, 2_000);
        key(&mut page, SDLK_RIGHT, Edge::Repeat, 2_050);
        tick(&mut page, 2_400);
        page.publish();
        assert!(!page.scrub.reveal && page.scrub.ns > 50 * S, "the fixture: the hold travelled");
        assert_eq!(plx_media::player::TX.scrub_ns.load(Relaxed), page.scrub.ns);
        step(&mut page, 2_416);
        assert!(!page.row.chrome_interactive(), "a travelled hold is a scrub: the chrome is away");

        page.transport_reset(true);
        plx_media::player::restore_state_for_test(was);
    }

    /// **(c) The scrubber's stop is a SEEK target, and a drag previews across it.**
    ///
    /// The pointer half of the same gesture. A click on the bar puts the preview under the
    /// pointer; a drag carries it (`§7.5`: a drag drives its control with no hover and no click);
    /// the button coming up — which reaches a screen as `Ok`/`Edge::Up`, the one release the
    /// dispatcher's `InputKind` has — commits it. Before this, the registered stop fell through to
    /// `key_ok`'s final `else` and TOGGLED PLAY/PAUSE, and `app.ptr.drag` had no producer left at
    /// all, so pointer scrubbing was gone.
    #[test]
    fn session_eight_click_coordinates_need_a_presented_hud_not_a_pointer_gate_override() {
        use plx_ui::hit::{HitMap, PointerKind};
        use plx_ui::screen::DrawFrame;
        let _g = plx_base::testlock::serial();
        let _f = Fixture::new(false);
        let was = plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Playing);
        for y in [870.0, 890.0] {
            let mut page = page_on_the_bar();
            assert!(page.hud_geometry(&cx()).grab.contains(1400.0, y),
                "the recorded session coordinates must still name the actual grab band");
            page.hud.dismissed = true;
            let mut map = HitMap::new();
            let cx = cx();
            let mut f = DrawFrame::new(&cx, plx_ui::Painter::root());
            page.record_stops(&mut f, false);
            map.fill(f.into_stops());
            map.swap();
            map.note_dpad();

            // All four ck events enter before another frame can paint. Motion reveals the HUD
            // state, but click still resolves against the LAST presented frame's empty hit map.
            pointer(&mut page, InputKind::Pointer { x: 1400.0, y, hit: None }, 1_000);
            assert!(!page.hud.dismissed && page.hud_up(TestHost::session(&cx), 1_000));
            let hit = map.resolve(Some(ENTRY), PointerKind::Click, 1400.0, y, None);
            assert!(hit.hit.is_none());
            let (_, reqs) = pointer(&mut page, InputKind::Click { x: 1400.0, y, hit: None }, 1_000);
            assert_eq!(reqs, vec![PlayerReq::Transport(None)], "a hidden scrubber is the picture's click");
            assert!(!page.scrub.drag);

            // A separate pm token lets a frame present the HUD before pd/ck. Register precisely
            // the stops the production draw uses, then keep D-pad hover suppression armed at the
            // SAME point: even with zero pointer travel, a click on a drawn control must work.
            let mut f = DrawFrame::new(&cx, plx_ui::Painter::root());
            page.record_stops(&mut f, true);
            map.fill(f.into_stops());
            map.swap();
            map.note_dpad();
            let hit = map.resolve(Some(ENTRY), PointerKind::Click, 1400.0, y, None);
            assert!(map.dpad_mode, "the hover gate remains armed");
            assert_eq!(hit.hit.map(|key| key.elem), Some(player_hud::ELEM_SCRUB));
            let (_, reqs) = pointer(&mut page,
                InputKind::Click { x: 1400.0, y, hit: hit.hit.map(|key| key.elem) }, 1_040);
            assert!(reqs.is_empty());
            assert!(page.scrub.drag);
            let previewed = page.scrub.ns;
            assert_eq!(seeks(key(&mut page, SDLK_RETURN, Edge::Up, 1_080).1),
                vec![PlayerReq::CommitSeek(previewed)]);
        }
        plx_media::player::restore_state_for_test(was);
    }

    #[test]
    fn a_click_on_the_scrubber_seeks_and_a_drag_previews_before_it() {
        let _g = plx_base::testlock::serial();
        let _f = Fixture::new(false);
        let mut page = page_on_the_bar();
        let tol = DUR / 1_000; // 100 ms: the band's own f32 arithmetic, not a behaviour

        let (mx, my) = on_the_bar(&page, 0.5);
        let (handled, reqs) = pointer(
            &mut page,
            InputKind::Click { x: mx, y: my, hit: Some(player_hud::ELEM_SCRUB) },
            1_000,
        );
        assert_eq!(handled, Handled::Yes);
        assert!(
            reqs.is_empty(),
            "a click on the bar is a SEEK, never the transport toggle",
        );
        assert!(
            (page.scrub.ns - DUR / 2).abs() < tol,
            "the preview lands where the pointer did (got {})",
            page.scrub.ns,
        );

        let (dx, dy) = on_the_bar(&page, 0.25);
        let (_, reqs) = pointer(
            &mut page,
            InputKind::Drag { x: dx, y: dy, hit: Some(player_hud::ELEM_SCRUB) },
            1_040,
        );
        assert!(reqs.is_empty(), "a drag PREVIEWS: it commits on release, not per motion");
        assert!(
            (page.scrub.ns - DUR / 4).abs() < tol,
            "…and the preview follows it (got {})",
            page.scrub.ns,
        );

        let previewed = page.scrub.ns;
        let (_, reqs) = key(&mut page, SDLK_RETURN, Edge::Up, 1_080);
        assert_eq!(
            seeks(reqs),
            vec![PlayerReq::CommitSeek(previewed)],
            "the button coming up commits exactly what the drag was showing",
        );
    }

    // ---- click-to-confirm and BACK (docs: the `step` doc, and ux-scenario §6.6) -------------

    const S: i64 = 1_000_000_000;

    /// Where the film is, as the engine publishes it. A request in these tests is only collected,
    /// never performed, so the playhead moves when a test says it does — which is what lets "back
    /// to where the scrub started" tell the origin from zero, and one gesture's origin from the
    /// next one's.
    fn playhead(ns: i64) {
        plx_media::player::SHARED.playpos_ns.store(ns, std::sync::atomic::Ordering::Relaxed);
    }

    fn back(page: &mut PlayerScreen, ms: u32) -> (Handled, Vec<PlayerReq>) {
        key_w(page, 0, WCODE_BACK, Edge::Down, ms)
    }

    /// A RIGHT tap at `ms`, then OK 100 ms later; returns the instant of the OK and the seeks it
    /// made.
    fn tap_and_commit(page: &mut PlayerScreen, ms: u32) -> (u32, Vec<PlayerReq>) {
        key(page, SDLK_RIGHT, Edge::Down, ms);
        key(page, SDLK_RIGHT, Edge::Up, ms + 20);
        let at = ms + 120;
        let (_, reqs) = key(page, SDLK_RETURN, Edge::Down, at);
        (at, seeks(reqs))
    }

    /// **A.3 — BACK while a hold is in progress cancels it.** No seek, now or when the key comes up
    /// or the net would have fired; the preview and the gesture are gone, the transport stays, and
    /// BACK did not mean "hide the HUD" or "leave".
    #[test]
    fn back_cancels_a_hold_in_progress() {
        let _g = plx_base::testlock::serial();
        let _f = Fixture::new(false);
        playhead(50 * S);
        let mut page = page_on_the_bar();
        key(&mut page, SDLK_RIGHT, Edge::Down, 1_000);
        key(&mut page, SDLK_RIGHT, Edge::Repeat, 1_050);
        tick(&mut page, 1_400);
        assert!(page.scrub.ns > 60 * S, "the fixture: the hold has travelled");

        let (handled, reqs) = back(&mut page, 1_500);
        assert_eq!(handled, Handled::Yes);
        assert!(reqs.is_empty(), "BACK cancels: no seek, and not an Exit either: {reqs:?}");
        assert_eq!(page.scrub.ns, -1, "the preview is gone");
        assert_eq!(page.scrub.key.dir, 0, "…and so is the hold");
        assert!(!page.hud.dismissed, "BACK spent itself on the scrub; the transport stays");

        // the key coming up, and every frame after: still nothing
        let (_, reqs) = key(&mut page, SDLK_RIGHT, Edge::Up, 1_520);
        let mut all = seeks(reqs);
        for t in [1_600, 2_500, 9_000] {
            all.extend(seeks(tick(&mut page, t).1));
        }
        assert!(all.is_empty(), "a cancelled hold never commits: {all:?}");
    }

    /// **A.3 — BACK cancels a pending key target** — a tap's, whenever it is pressed: no seek, now
    /// or from any timer, and the knob goes home.
    #[test]
    fn back_cancels_a_pending_target() {
        let _g = plx_base::testlock::serial();
        let _f = Fixture::new(false);
        playhead(50 * S);
        for at in [1_200, 30_000] {
            let mut page = page_on_the_bar();
            key(&mut page, SDLK_RIGHT, Edge::Down, 1_000);
            key(&mut page, SDLK_RIGHT, Edge::Up, 1_020);
            let (_, reqs) = back(&mut page, at);
            assert!(reqs.is_empty(), "BACK at {at} ms: {reqs:?}");
            assert_eq!(page.scrub.ns, -1);
            assert!(!page.hud.dismissed, "BACK spent itself on the target; the transport stays");
            let (_, reqs) = tick(&mut page, at + input::InfoTimeout::DEFAULT.milliseconds() + 50);
            assert!(seeks(reqs).is_empty(), "nothing is left to commit");
        }
    }

    /// **A.3 — BACK during a pointer drag cancels it**, and the button coming up afterwards commits
    /// nothing.
    #[test]
    fn back_cancels_a_seated_drag() {
        let _g = plx_base::testlock::serial();
        let _f = Fixture::new(false);
        playhead(50 * S);
        let mut page = page_on_the_bar();
        let (mx, my) = on_the_bar(&page, 0.75);
        pointer(&mut page, InputKind::Click { x: mx, y: my, hit: Some(player_hud::ELEM_SCRUB) }, 1_000);
        assert!(page.scrub.drag && page.scrub.ns > 0, "the fixture: a drag is seated");

        let (_, reqs) = back(&mut page, 1_100);
        assert!(reqs.is_empty());
        assert!(!page.scrub.drag && page.scrub.ns == -1);
        let (_, reqs) = key(&mut page, SDLK_RETURN, Edge::Up, 1_150);
        assert!(seeks(reqs).is_empty(), "the release has nothing left to commit");
    }

    /// With the scrub cancelled, the NEXT BACK is the ordinary one: it hides the transport.
    #[test]
    fn a_second_back_after_a_cancel_has_its_normal_meaning() {
        let _g = plx_base::testlock::serial();
        let _f = Fixture::new(false);
        let mut page = page_on_the_bar();
        key(&mut page, SDLK_RIGHT, Edge::Down, 1_000);
        back(&mut page, 1_100);
        assert!(!page.hud.dismissed);
        let (_, reqs) = back(&mut page, 1_200);
        assert!(reqs.is_empty() && page.hud.dismissed, "hide the transport: {reqs:?}");
    }

    /// **A commit is final: BACK afterwards is BACK's ordinary meaning** — hide the transport —
    /// and never a seek back to where the scrub started. A key target's confirmation and a playing
    /// film's drag release are the two ways a scrub commits on a playing film.
    #[test]
    fn back_after_a_commit_has_its_normal_meaning_and_seeks_nowhere() {
        let _g = plx_base::testlock::serial();
        let _f = Fixture::new(false);
        playhead(50 * S);
        let mut page = page_on_the_bar();
        let (at, commit) = tap_and_commit(&mut page, 1_000);
        assert_eq!(commit, vec![PlayerReq::SeekTo(60 * S)]);
        playhead(60 * S); // the seek landed
        let (handled, reqs) = back(&mut page, at + 1_000);
        assert_eq!(handled, Handled::Yes);
        assert!(seeks(reqs.clone()).is_empty(), "no jump back after a key commit: {reqs:?}");
        assert!(page.hud.dismissed, "BACK hides the transport");

        // a HOLD's commit is no different
        playhead(20 * S);
        let mut page = page_on_the_bar();
        key(&mut page, SDLK_RIGHT, Edge::Down, 1_000);
        key(&mut page, SDLK_RIGHT, Edge::Repeat, 1_050);
        tick(&mut page, 1_400);
        key(&mut page, SDLK_RIGHT, Edge::Up, 1_420);
        tick(&mut page, 1_420 + plx_appkit::player_hud::SCRUB_RELEASE_DEBOUNCE_MS + 1);
        let at = 3_000;
        assert_eq!(seeks(key(&mut page, SDLK_RETURN, Edge::Down, at).1).len(), 1, "the fixture: OK committed the hold");
        playhead(30 * S);
        let (_, reqs) = back(&mut page, at + 1_000);
        assert!(seeks(reqs.clone()).is_empty(), "no jump back after a hold's commit: {reqs:?}");
        assert!(page.hud.dismissed);

        let mut page = page_on_the_bar();
        let (mx, my) = on_the_bar(&page, 0.9);
        pointer(&mut page, InputKind::Click { x: mx, y: my, hit: Some(player_hud::ELEM_SCRUB) }, 10_000);
        let target = page.scrub.ns;
        assert_eq!(seeks(key(&mut page, SDLK_RETURN, Edge::Up, 10_080).1), vec![PlayerReq::CommitSeek(target)]);
        playhead(target);
        let (_, reqs) = back(&mut page, 11_000);
        assert!(seeks(reqs.clone()).is_empty(), "no jump back after a drag's release: {reqs:?}");
        assert!(page.hud.dismissed, "BACK hides the transport");
    }

    /// …and a BACK that finds NO transport on screen leaves, as it always did.
    #[test]
    fn back_on_a_hidden_transport_after_a_commit_leaves() {
        let _g = plx_base::testlock::serial();
        let _f = Fixture::new(false);
        playhead(50 * S);
        let mut page = page_on_the_bar();
        let (at, _) = tap_and_commit(&mut page, 1_000);
        page.hud.visible_at_press = false; // the transport is not on screen at the press
        let (_, reqs) = back(&mut page, at + 500);
        assert_eq!(reqs, vec![PlayerReq::Exit], "an ordinary BACK over a bare picture: {reqs:?}");
    }

    /// **B.5 — taps move the target and commit NOTHING**, playing or paused: not on release and
    /// not after the 450 ms; a second tap moves the target on from where the first left it; the
    /// transport stays up for as long as the target is pending (the linger never dismisses it, and
    /// no timer commits it).
    #[test]
    fn taps_move_the_target_and_commit_nothing() {
        for paused in [false, true] {
            let _g = plx_base::testlock::serial();
            let _f = Fixture::new(paused);
            playhead(50 * S);
            let mut page = page_on_the_bar();
            key(&mut page, SDLK_RIGHT, Edge::Down, 1_000);
            key(&mut page, SDLK_RIGHT, Edge::Up, 1_020);
            let mut all = Vec::new();
            for t in [1_020 + plx_appkit::player_hud::TAP_COMMIT_MS + 1, 3_000, 20_000, 120_000] {
                all.extend(seeks(tick(&mut page, t).1));
            }
            assert!(all.is_empty(), "paused {paused}: no timer commits a target: {all:?}");
            assert_eq!(page.scrub.ns, 60 * S, "paused {paused}: the target is still pending");

            key(&mut page, SDLK_RIGHT, Edge::Down, 130_000);
            key(&mut page, SDLK_RIGHT, Edge::Up, 130_020);
            assert_eq!(page.scrub.ns, 70 * S, "the second tap goes on from the target, not the playhead");
            tick(&mut page, 200_000);
            assert_eq!(page.scrub.ns, 70 * S);
        }
    }

    /// **B.5 — the transport stays up for a pending target while the film PLAYS**: the 4.5 s
    /// linger would hide it and, with it, the only sign there is a target to confirm. Frame by
    /// frame for a minute, the HUD is never dropped, and the target is never committed by the
    /// hide.
    #[test]
    fn a_pending_target_keeps_the_transport_up_while_the_film_plays() {
        let _g = plx_base::testlock::serial();
        let _f = Fixture::new(false);
        playhead(50 * S);
        let mut page = page_on_the_bar();
        key(&mut page, SDLK_RIGHT, Edge::Down, 1_000);
        key(&mut page, SDLK_RIGHT, Edge::Up, 1_020);
        let cx = cx();
        let mut all = Vec::new();
        let mut t = 1_020;
        while t < 61_000 {
            t += 16;
            all.extend(seeks(tick(&mut page, t).1));
            assert!(
                page.hud.visible(TestHost::session(&cx), t, false),
                "{t} ms: the transport was dropped from under a pending target",
            );
        }
        assert!(all.is_empty(), "…and nothing seeked: {all:?}");
        assert_eq!(page.scrub.ns, 60 * S);

        // once the target is taken, the transport lingers and then goes, as it always did
        key(&mut page, SDLK_RETURN, Edge::Down, t);
        assert!(!page.hud.visible(TestHost::session(&cx), t + 20_000, false));
    }

    /// **B.5 — a hold moves the target, and its release commits nothing**, playing or paused, nor
    /// does the lost-keyup net. The target is left pending, the key gesture over.
    #[test]
    fn a_hold_leaves_its_target_pending_on_release() {
        for paused in [false, true] {
            let _g = plx_base::testlock::serial();
            let _f = Fixture::new(paused);
            playhead(50 * S);
            let mut page = page_on_the_bar();
            key(&mut page, SDLK_RIGHT, Edge::Down, 1_000);
            key(&mut page, SDLK_RIGHT, Edge::Repeat, 1_050);
            tick(&mut page, 1_400);
            let travelled = page.scrub.ns;
            assert!(travelled > 60 * S, "the fixture: the hold travelled");
            key(&mut page, SDLK_RIGHT, Edge::Up, 1_420);
            let mut all = Vec::new();
            for t in [1_420 + plx_appkit::player_hud::SCRUB_RELEASE_DEBOUNCE_MS + 1, 5_000, 60_000] {
                all.extend(seeks(tick(&mut page, t).1));
            }
            assert!(all.is_empty(), "paused {paused}: {all:?}");
            assert_eq!(page.scrub.ns, travelled, "the target waits for OK or PLAY");
            assert_eq!(page.scrub.key.dir, 0, "the key gesture is over");
        }
    }

    /// **B.6 — OK or PLAY (or the play/pause key) takes the target and leaves the film PLAYING from
    /// it** — `SeekTo`, the request that seeks and resumes a paused film (and is only a seek on a
    /// playing one), one request and nothing else; PAUSE does not take it.
    #[test]
    fn ok_or_play_commits_the_target_and_the_film_plays_on_from_it() {
        use plx_ui::consts::{WCODE_PAUSE, WCODE_PLAY, WCODE_PLAYPAUSE};
        for paused in [false, true] {
            let _g = plx_base::testlock::serial();
            let _f = Fixture::new(paused);
            playhead(50 * S);
            let pending = || {
                let mut page = page_on_the_bar();
                key(&mut page, SDLK_RIGHT, Edge::Down, 1_000);
                key(&mut page, SDLK_RIGHT, Edge::Up, 1_020);
                assert_eq!(page.scrub.ns, 60 * S);
                page
            };
            let mut page = pending();
            let (_, reqs) = key(&mut page, SDLK_RETURN, Edge::Down, 2_000);
            assert_eq!(reqs, vec![PlayerReq::SeekTo(60 * S)], "paused {paused}: OK: seek and resume, nothing else");
            assert_eq!(page.scrub.ns, -1);

            for wcode in [WCODE_PLAY, WCODE_PLAYPAUSE] {
                let mut page = pending();
                let (_, reqs) = key_w(&mut page, 0, wcode, Edge::Down, 2_000);
                assert_eq!(reqs, vec![PlayerReq::SeekTo(60 * S)], "paused {paused}: wcode {wcode}");
                assert_eq!(page.scrub.ns, -1);
            }

            // the key coming up afterwards, and every frame after, adds nothing
            let (_, reqs) = key(&mut page, SDLK_RETURN, Edge::Up, 2_050);
            let mut all = seeks(reqs);
            all.extend(seeks(tick(&mut page, 2_500).1));
            assert!(all.is_empty(), "{all:?}");

            let mut page = pending();
            key_w(&mut page, 0, WCODE_PAUSE, Edge::Down, 2_000);
            assert_eq!(page.scrub.ns, 60 * S, "paused {paused}: PAUSE leaves the target alone");
        }
    }

    /// **The `toggle_play` command and the play keys are one function** ([`PlayerScreen::toggle_play`]):
    /// whatever the keys answer for a pending target, the command answers the same, because the key
    /// arm and the app's performer both call it. PAUSE (`Some(false)`) never takes the target.
    #[test]
    fn the_toggle_play_command_takes_a_pending_target_exactly_as_the_play_keys_do() {
        let _g = plx_base::testlock::serial();
        let _f = Fixture::new(false);
        playhead(50 * S);
        let pending = || {
            let mut page = page_on_the_bar();
            key(&mut page, SDLK_RIGHT, Edge::Down, 1_000);
            key(&mut page, SDLK_RIGHT, Edge::Up, 1_020);
            assert_eq!(page.scrub.ns, 60 * S);
            page
        };
        let mut page = pending();
        assert_eq!(page.toggle_play(None, 2_000), PlayerReq::SeekTo(60 * S));
        assert_eq!(page.scrub.ns, -1, "the target is spent");
        assert_eq!(page.toggle_play(None, 2_100), PlayerReq::Transport(None), "with none pending it toggles");
        let mut page = pending();
        assert_eq!(page.toggle_play(Some(false), 2_000), PlayerReq::Transport(Some(false)));
        assert_eq!(page.scrub.ns, 60 * S, "PAUSE leaves the target alone");
        let mut page = pending();
        assert_eq!(page.toggle_play(Some(true), 2_000), PlayerReq::SeekTo(60 * S), "PLAY confirms it");
    }

    /// **B.7 — BACK returns the knob to the playhead**, playing or paused: no seek, the target is
    /// gone, the transport stays.
    #[test]
    fn back_drops_a_pending_target_without_a_seek() {
        for paused in [false, true] {
            let _g = plx_base::testlock::serial();
            let _f = Fixture::new(paused);
            playhead(50 * S);
            let mut page = page_on_the_bar();
            key(&mut page, SDLK_RIGHT, Edge::Down, 1_000);
            key(&mut page, SDLK_RIGHT, Edge::Up, 1_020);
            let (_, reqs) = back(&mut page, 5_000);
            assert!(reqs.is_empty(), "no seek: {reqs:?}");
            assert_eq!(page.scrub.ns, -1);
            assert!(!page.hud.dismissed);
        }
    }

    /// **B.8 — whatever else takes the page cancels the target; none of them commits it.** A panel
    /// opening or the capsule rising over the transport, leaving the scrubber with UP or DOWN, a
    /// Skip offer raising the control row, the control row's own press, and a teardown.
    #[test]
    fn whatever_else_takes_the_page_cancels_the_target_and_never_commits_it() {
        use plx_ui::consts::{SDLK_DOWN, SDLK_UP};
        let _g = plx_base::testlock::serial();
        let _f = Fixture::new(false);
        playhead(50 * S);
        let pending = || {
            let mut page = page_on_the_bar();
            key(&mut page, SDLK_RIGHT, Edge::Down, 1_000);
            key(&mut page, SDLK_RIGHT, Edge::Up, 1_020);
            assert_eq!(page.scrub.ns, 60 * S, "the fixture: a target is pending");
            page
        };
        let settled = |page: &mut PlayerScreen, what: &str| {
            assert_eq!(page.scrub.ns, -1, "{what}: the target is cancelled");
            let mut all = Vec::new();
            for t in [1_500, 3_000, 30_000] {
                all.extend(seeks(tick(page, t).1));
            }
            assert!(all.is_empty(), "{what}: never committed: {all:?}");
        };

        for policy in [HudPolicy::Lifted, HudPolicy::Hidden] {
            let mut page = pending();
            page.set_hud_policy(policy);
            settled(&mut page, &format!("{policy:?}"));
        }
        for sym in [SDLK_UP, SDLK_DOWN] {
            let mut page = pending();
            let mut rig = Rig::on(&page, ENTRY);
            let (_, reqs) = rig.key(&mut page, sym, 0, Edge::Down, 1_100);
            assert!(seeks(reqs).is_empty());
            assert_ne!(page.hud.nav.focus, GROUP_SCRUB, "the fixture: the key left the scrubber");
            settled(&mut page, "leaving the scrubber");
        }
        let mut page = pending();
        page.raise_for_offer(1_100, 0);
        settled(&mut page, "a Skip offer raising the row");
        let mut page = pending();
        {
            let mut out = Vec::new();
            let mut present = plx_machine::present::Present::new();
            page.arm_control_row(&mut Effects::<TestHost>::new(&mut out, MachineId::Instance(INST), &mut present));
        }
        settled(&mut page, "the control row's press");
        let mut page = pending();
        page.transport_reset(true);
        settled(&mut page, "a teardown / reload");
    }

    /// **B.9 — a Magic Remote DRAG still commits on release**, which is the pointer's own
    /// confirmation: a playing film plays on, a paused one stays paused (`CommitSeek` holds the
    /// pause).
    #[test]
    fn a_drag_release_commits_a_playing_film_plays_on_and_a_paused_one_stays_paused() {
        for paused in [false, true] {
            let _g = plx_base::testlock::serial();
            let _f = Fixture::new(paused);
            playhead(50 * S);
            let mut page = page_on_the_bar();
            let (mx, my) = on_the_bar(&page, 0.75);
            pointer(&mut page, InputKind::Click { x: mx, y: my, hit: Some(player_hud::ELEM_SCRUB) }, 1_000);
            let target = page.scrub.ns;
            let (_, reqs) = key(&mut page, SDLK_RETURN, Edge::Up, 1_080);
            assert_eq!(reqs, vec![PlayerReq::CommitSeek(target)], "paused {paused}: one request, not a resuming one");
        }
    }

    /// **C — paused: a drag's release commits and the film STAYS paused** (`CommitSeek` is the
    /// request that holds the pause; the pump re-pauses on the landing — "seek: paused frame
    /// restored"), and BACK during the drag cancels it.
    #[test]
    fn a_paused_drag_commits_and_stays_paused_and_back_cancels_it() {
        let _g = plx_base::testlock::serial();
        let _f = Fixture::new(true);
        playhead(50 * S);
        let mut page = page_on_the_bar();
        let (mx, my) = on_the_bar(&page, 0.75);
        pointer(&mut page, InputKind::Click { x: mx, y: my, hit: Some(player_hud::ELEM_SCRUB) }, 1_000);
        let target = page.scrub.ns;
        let (_, reqs) = key(&mut page, SDLK_RETURN, Edge::Up, 1_080);
        assert_eq!(reqs, vec![PlayerReq::CommitSeek(target)], "one request, and not a resuming one");

        let mut page = page_on_the_bar();
        pointer(&mut page, InputKind::Click { x: mx, y: my, hit: Some(player_hud::ELEM_SCRUB) }, 1_000);
        let (_, reqs) = back(&mut page, 1_040);
        assert!(reqs.is_empty() && page.scrub.ns == -1 && !page.scrub.drag);
    }

    /// A pending target does not depend on the transport's state: pausing or resuming the film by
    /// some other route (a click on the picture, the PAUSE key) leaves it where it is, waiting for
    /// OK or BACK — it is no longer a paused-only thing to be retired.
    #[test]
    fn a_pending_target_survives_the_film_being_paused_or_resumed_another_way() {
        use std::sync::atomic::Ordering::Relaxed;
        let _g = plx_base::testlock::serial();
        let _f = Fixture::new(true);
        playhead(50 * S);
        let mut page = page_on_the_bar();
        key(&mut page, SDLK_RIGHT, Edge::Down, 1_000);
        key(&mut page, SDLK_RIGHT, Edge::Up, 1_020);
        tick(&mut page, 2_000);
        assert_eq!(page.scrub.ns, 60 * S);
        plx_media::player::TX.paused.store(false, Relaxed);
        tick(&mut page, 2_100);
        assert_eq!(page.scrub.ns, 60 * S, "resumed: the target stays");
        plx_media::player::TX.paused.store(true, Relaxed);
        tick(&mut page, 2_200);
        assert_eq!(page.scrub.ns, 60 * S, "paused again: the target stays");
        let (_, reqs) = key(&mut page, SDLK_RETURN, Edge::Down, 2_300);
        assert_eq!(reqs, vec![PlayerReq::SeekTo(60 * S)]);
    }
}
#[cfg(test)]
mod repair_confirmation_tests {
    use super::*;
    use plx_machine::machine::{Host, InputEvent, InputOwner, MachineId, PressId, Source, Tick};
    struct TestHost;
    impl Host for TestHost {
        type Arg = plx_ui::fixture::FixtureArg;
        type Fx = AppFx;
        type Msg = crate::registry::AppMsg;
        type Elem = u32;
        type Views<'a> = &'a plx_media::route::PlaybackSession;
        type Init = plx_ui::fixture::FixtureArg;
        type Memory = crate::registry::PageMemory;
    }
    impl PlayerLike for TestHost {
        fn session<'a>(cx: &Cx<'a, Self>) -> &'a plx_media::route::PlaybackSession { cx.views }
    }
    thread_local! {
        static TEST_METADATA: std::cell::UnsafeCell<plx_data::stores::metadata::MetadataStore> =
            std::cell::UnsafeCell::new(plx_data::stores::metadata::MetadataStore::default());
    }
    fn test_store() -> &'static mut plx_data::stores::metadata::MetadataStore {
        TEST_METADATA.with(|cell| unsafe { &mut *cell.get() })
    }
    impl crate::registry::MetadataLike for TestHost {
        fn metadata<'a>(_cx: &Cx<'a, Self>) -> plx_data::metadata::MetadataView<'a> {
            test_store().view()
        }
    }
    fn context(ps: &plx_media::route::PlaybackSession, elem: Option<u32>) -> Cx<'_, TestHost> {
        let mut cx = Cx { views: ps, tick: Tick::default(), measure: &plx_ui::fixture::FixtureMeasure,
            focus: Default::default(), press: Default::default(), owner: InputOwner::Entry(EntryId(1)) };
        cx.focus.current = elem.map(|elem| FocusKey { entry: EntryId(1), elem });
        cx
    }
    fn deliver(page: &mut PlayerScreen, ps: &plx_media::route::PlaybackSession, elem: Option<u32>, ev: ScreenEvent<TestHost>) -> Vec<PlayerReq> {
        let mut out = Vec::new();
        let mut present = plx_machine::present::Present::new();
        page.step(&ev, &context(ps, elem), &mut Effects::new(&mut out, MachineId::Instance(InstanceId(1)), &mut present));
        for effect in &out {
            if let Fx::Deliver(MachineId::Instance(id), _) = &effect.fx {
                assert_eq!(*id, InstanceId(1), "focus requests must address this screen instance");
            }
        }
        out.into_iter().filter_map(|e| match e.fx { Fx::App(AppFx::Player(req)) => Some(req), _ => None }).collect()
    }
    fn key_event(sym: u32, wcode: u32) -> ScreenEvent<TestHost> {
        ScreenEvent::Input(InputEvent { kind: InputKind::Key { key: plx_machine::machine::Key::Other, sym, wcode, edge: Edge::Down, at_edge: false }, at: Tick::default(), source: Source::Sdl })
    }
    fn click_event(hit: Option<u32>) -> ScreenEvent<TestHost> {
        ScreenEvent::Input(InputEvent {
            kind: InputKind::Click { x: 0.0, y: 0.0, hit },
            at: Tick::default(),
            source: Source::Sdl,
        })
    }
    fn blocked() -> plx_media::route::PlaybackSession {
        let mut ps = plx_media::route::PlaybackSession::IDLE;
        ps.jail_load_blocked = true;
        ps
    }
    #[test]
    #[cfg(feature = "devtriggers")]
    fn jail_fixture_keeps_permission_refusal_without_offering_repair() {
        let _g = plx_base::testlock::serial();
        // The `failtest` trigger is armed in a PRIVATE trigger root, never in the shared runtime
        // root: that is the bare `/tmp` on a host test run, and another process's player tests
        // read `plxnative-failtest` there (`player_hud::busy`), so arming it for the length of this
        // test turned their click on the picture into a click on a failed playback.
        plx_base::devtrig::with_private_triggers(|| {
            let path = plx_base::devtrig::path("failtest");
            std::fs::write(&path, "jail").unwrap();
            let mut ps = plx_media::route::PlaybackSession::IDLE;
            plx_media::route::reset_player_control_for_test(&ps);
            let hardware_verdict = plx_platform::tv::sandbox::blocks_native_video();
            plx_media::player::failure_fixture(&mut ps);
            let mut page = PlayerScreen::new(EntryId(1));
            assert_eq!(player_hud::FailureReadout::now(&ps).actions(), &[plx_media::player::FailureAction::Back]);
            assert_eq!(deliver(&mut page, &ps, None, key_event(0, consts::WCODE_BACK)), vec![PlayerReq::Exit]);
            assert!(!page.repair_alert.is_open(), "a permission refusal must not offer device modification");
            assert!(deliver(&mut page, &ps, Some(REPAIR_CANCEL), ScreenEvent::Tick(Tick { ms: 16, dt_us: 16_000 })).is_empty());
            assert!(!page.repair_alert.is_open(), "Tick must not resurrect the removed repair action");
            assert_eq!(player_hud::FailureReadout::now(&ps).actions(), &[plx_media::player::FailureAction::Back]);
            assert!(ps.jail_load_blocked);
            assert_eq!(plx_platform::tv::sandbox::blocks_native_video(), hardware_verdict, "the fixture must not alter the cached hardware verdict");
            ps.jail_load_blocked = false;
            deliver(&mut page, &ps, None, ScreenEvent::Tick(Tick { ms: 32, dt_us: 16_000 }));
            assert!(!page.repair_alert.is_open(), "retiring the refusal must not open a confirmation");
            std::fs::write(&path, "tv").unwrap();
            plx_media::player::failure_fixture(&mut ps);
            assert!(!ps.jail_load_blocked, "other failure fixtures must not claim a jail refusal");
        });
    }

    #[test]
    fn permission_refusal_offers_only_exit_for_keyboard_and_pointer() {
        let _g = plx_base::testlock::serial();
        let ps = blocked();
        plx_media::route::reset_player_control_for_test(&ps);
        let mut page = PlayerScreen::new(EntryId(1));
        page.busy = player_hud::busy(&ps);
        assert_eq!(player_hud::FailureReadout::now(&ps).actions(), &[plx_media::player::FailureAction::Back]);
        assert!(!page.group_drawn(GROUP_FAILURE, true), "a text notice owns no focus group");
        assert!(!page.group_drawn(GROUP_REPAIR, true));
        assert_eq!(deliver(&mut page, &ps, None, key_event(0, consts::WCODE_BACK)), vec![PlayerReq::Exit]);
        assert!(deliver(&mut page, &ps, None, click_event(Some(player_hud::ELEM_FAILURE_BASE))).is_empty(), "retired banner buttons do nothing");
        assert!(deliver(&mut page, &ps, None, key_event(consts::SDLK_RETURN, 0)).is_empty(), "OK is not an implicit Back");
        assert!(!page.repair_alert.is_open());
    }
    #[test]
    fn stale_repair_controls_cannot_modify_device_or_block_exit() {
        let _g = plx_base::testlock::serial();
        let ps = blocked();
        plx_media::route::reset_player_control_for_test(&ps);
        let mut page = PlayerScreen::new(EntryId(1));
        for elem in [REPAIR_CANCEL, REPAIR_CONFIRM] {
            assert!(deliver(&mut page, &ps, Some(elem), ScreenEvent::PressCommit(PressId(1))).is_empty());
            assert!(deliver(&mut page, &ps, None, click_event(Some(elem))).is_empty());
        }
        assert!(deliver(&mut page, &ps, None, click_event(None)).is_empty(), "the failed picture must not toggle playback");
        assert_eq!(deliver(&mut page, &ps, Some(REPAIR_CONFIRM), key_event(0, consts::WCODE_BACK)), vec![PlayerReq::Exit]);
        assert!(!page.repair_alert.is_open());
        assert!(deliver(&mut page, &ps, Some(REPAIR_CONFIRM), ScreenEvent::PressCommit(PressId(1))).is_empty());
        assert!(deliver(&mut page, &ps, None, ScreenEvent::Activate(player_hud::ELEM_FAILURE_BASE)).is_empty());
        assert!(!page.repair_alert.is_open());
    }
    /// Moved up from `player::machine`'s tests: it builds a replacement `PlayerScreen`, and
    /// `player` may not name `screens`. The attempt is the `Player` machine's, not the screen's.
    #[test]
    fn repair_survives_screen_and_session_recreation_and_rejects_stale_completions() {
        use plx_platform::tv::sandbox::{Failure, State};
        let mut player = plx_media::player::machine::Player::new();
        let token = player.repair.begin(true).unwrap();
        assert!(!player.repair.complete(token + 1, Ok(())));
        player.session = plx_media::route::PlaybackSession::IDLE;
        let _replacement = PlayerScreen::new(EntryId(9));
        assert_eq!(player.repair.state(), State::Running);
        assert_eq!(player.repair.begin(true), None);
        assert!(player.repair.complete(token, Err(Failure::Timeout)));
        assert!(!player.repair.complete(token, Ok(())));
        assert_eq!(player.repair.state(), State::Failed(Failure::Timeout));
        assert_eq!(player.repair.begin(true), None);
    }
    #[test]
    fn accepted_attempt_hides_forward_action_after_screen_recreation() {
        let _g = plx_base::testlock::serial();
        for state in [plx_platform::tv::sandbox::State::Running, plx_platform::tv::sandbox::State::Repaired, plx_platform::tv::sandbox::State::Failed(plx_platform::tv::sandbox::Failure::Timeout)] {
            let mut ps = blocked(); ps.repair_status = state;
            plx_media::route::reset_player_control_for_test(&ps);
            let row = plx_appkit::player_hud::FailureReadout::now(&ps).actions().to_vec();
            assert!(!row.contains(&plx_media::player::FailureAction::Repair), "{state:?}: {row:?}");
            let mut page = PlayerScreen::new(EntryId(1));
            // OK performs the row's first control, which is no longer the repair question: with
            // Repair gone the row is only Back
            assert_eq!(deliver(&mut page, &ps, None, key_event(0, consts::WCODE_BACK)), vec![PlayerReq::Exit]);
            assert!(!page.repair_alert.is_open());
        }
    }
}

#[cfg(test)]
mod reminder_tests {
    use super::*;
    use crate::player::overlay_tests::TestHost;
    use plx_data::ott::reminders::{Book, Context, Document, Fx as ReminderFx, PromptToken};
    use plx_machine::machine::MachineId;

    fn prompt() -> (ReminderFx, PromptToken) {
        let document = Document::parse(br#"{"version":1,"records":[{"key":{"source_id":"source","channel_id":"channel","programme_id":"programme","start_s":200},"end_s":800,"title":"Programme"}]}"#).unwrap();
        let mut book = Book::new(document, 100, 1).unwrap();
        let effect = book.poll(200, Context { source_id: "source", revision: 1, active: true }, 0, |_| true)
            .unwrap().pop().unwrap();
        let ReminderFx::PromptSwitch { token, .. } = effect else { panic!("expected prompt") };
        (effect, token)
    }

    #[test]
    fn reminder_answer_emits_current_token_once_and_never_repairs_sandbox() {
        let _guard = plx_base::testlock::serial();
        let (effect, token) = prompt();
        let mut page = PlayerScreen::new(EntryId(1));
        assert!(page.reminder_effect(effect, 10));
        let mut out = Vec::new();
        let mut present = plx_machine::present::Present::new();
        let mut fx = Effects::<TestHost>::new(&mut out, MachineId::Instance(InstanceId(7)), &mut present);
        page.repair_answer(true, &mut fx);
        page.repair_answer(true, &mut fx);
        drop(fx);
        let replies: Vec<_> = out.iter().filter_map(|effect| match &effect.fx {
            Fx::App(AppFx::Ott(crate::registry::OttOut::Action { reply, action: crate::registry::OttAction::ReminderAnswer { token, accept } })) => Some((*reply, *token, *accept)),
            _ => None,
        }).collect();
        assert_eq!(replies, vec![(InstanceId(7), token, true)]);
        assert!(!out.iter().any(|effect| matches!(&effect.fx, Fx::App(AppFx::Player(PlayerReq::RepairSandbox)))));
        assert!(page.reminder_sheet); // Keep the same labels through its exit motion.
        page.clear_reminders();
    }

    #[test]
    fn lead_card_keeps_footer_focus_and_sandbox_confirmation_untouched() {
        let _guard = plx_base::testlock::serial();
        let (effect, _) = prompt();
        let ReminderFx::PromptSwitch { record, .. } = effect else { unreachable!() };
        let mut page = PlayerScreen::new(EntryId(1));
        let before = (page.hud.until, page.hud.nav.focus, page.hud.nav.btn);
        assert!(page.reminder_effect(ReminderFx::LeadNotice { record, minutes: 1 }, 100));
        assert_eq!((page.hud.until, page.hud.nav.focus, page.hud.nav.btn), before);
        assert!(!page.repair_alert.visible());
        assert!(page.reminder_notice.is_open());
        assert_eq!(page.reminder_notice_until, Some(100 + input::HUD_LINGER_MS));
        page.clear_reminders();
        assert!(!page.reminder_notice.visible());
    }

    #[test]
    fn close_effect_and_teardown_remove_only_the_current_reminder() {
        let _guard = plx_base::testlock::serial();
        let (effect, token) = prompt();
        let mut page = PlayerScreen::new(EntryId(1));
        page.reminder_effect(effect, 100);
        page.reminder_effect(ReminderFx::ClosePrompt(token), 100);
        assert!(page.reminder_prompt.is_none());
        assert!(!page.repair_alert.visible());
        assert!(!page.reminder_focus);
        page.repair_alert.open(c"Existing repair");
        page.clear_reminders();
        assert!(page.repair_alert.is_open());
        page.repair_alert.close();
    }

    #[test]
    fn an_existing_sandbox_dialog_cannot_be_replaced_by_a_programme() {
        let _guard = plx_base::testlock::serial();
        let (effect, _) = prompt();
        let mut page = PlayerScreen::new(EntryId(1));
        page.repair_alert.open(c"Existing repair");
        assert!(!page.reminder_effect(effect, 100));
        assert!(page.reminder_prompt.is_none());
        assert!(!page.reminder_sheet);
        page.repair_alert.close();
    }

    #[test]
    fn clearing_an_absent_volume_notice_does_not_invalidate_repeatedly() {
        let _guard = plx_base::testlock::serial();
        let mut page = PlayerScreen::new(EntryId(1));
        plx_machine::idle::take_local_damage();
        for _ in 0..3 { page.clear_volume_notice(); }
        assert_eq!(plx_machine::idle::take_local_damage(), 0);
        assert!(page.volume_notice(plx_media::player::volume::DesiredVolume::DEFAULT, 100));
        page.clear_volume_notice();
        assert!(plx_machine::idle::take_local_damage() > 0);
        for _ in 0..3 { page.clear_volume_notice(); }
        assert_eq!(plx_machine::idle::take_local_damage(), 0);
    }

    #[test]
    fn volume_notice_yields_to_reminders_favorites_and_confirmations() {
        let _guard = plx_base::testlock::serial();
        let volume = plx_media::player::volume::DesiredVolume::DEFAULT;
        let mut page = PlayerScreen::new(EntryId(1));
        assert!(page.volume_notice(volume, 10));
        assert!(page.favorite_notice("Added to Favorites", 20));
        assert!(!page.volume_notice.visible());
        assert!(!page.volume_notice(volume, 30));
        assert_eq!(page.reminder_notice.body_for_test()[0].as_ref(), "Added to Favorites");
        page.clear_reminders();
        assert!(page.volume_notice(volume, 40));
        let (effect, _) = prompt();
        let ReminderFx::PromptSwitch { record, .. } = effect else { unreachable!() };
        assert!(page.reminder_effect(ReminderFx::LeadNotice { record, minutes: 1 }, 50));
        assert!(!page.volume_notice.visible());
        assert!(!page.volume_notice(volume, 60));
        page.clear_reminders();
        assert!(page.volume_notice(volume, 70));
        let (effect, _) = prompt();
        assert!(page.reminder_effect(effect, 80));
        assert!(!page.volume_notice.visible());
        assert!(!page.volume_notice(volume, 90));
        assert!(page.repair_alert.is_open());
        page.clear_reminders();
        page.repair_alert.open(c"Existing repair");
        assert!(!page.volume_notice(volume, 100));
        assert!(page.repair_alert.is_open());
        page.repair_alert.close();
    }

    #[test]
    fn favorite_notice_does_not_open_or_extend_the_transport_footer() {
        let _guard = plx_base::testlock::serial();
        let mut page = PlayerScreen::new(EntryId(1));
        let before = (page.hud.until, page.hud.nav.focus, page.hud.nav.btn);
        assert!(page.favorite_notice("Added to Favorites", 100));
        assert_eq!((page.hud.until, page.hud.nav.focus, page.hud.nav.btn), before);
        assert!(page.reminder_notice.is_open());
        assert_eq!(page.reminder_notice_until, Some(100 + input::HUD_LINGER_MS));
        page.clear_reminders();
        assert!(!page.reminder_notice.visible());
    }

    #[test]
    fn favorite_notice_cannot_replace_an_existing_confirmation() {
        let _guard = plx_base::testlock::serial();
        let mut page = PlayerScreen::new(EntryId(1));
        page.repair_alert.open(c"Existing repair");
        assert!(!page.favorite_notice("Removed from Favorites", 100));
        assert!(page.repair_alert.is_open());
        assert!(!page.reminder_notice.is_open());
        assert!(page.reminder_notice_until.is_none());
        page.repair_alert.close();
    }
}

#[cfg(test)]
mod classic_geometry_stop_tests {
    use super::*;
    use plx_machine::machine::{Host, FontScale, TypographyMeasure, UiMode, InputOwner};
    struct ModeHost;
    impl Host for ModeHost {
        type Arg = plx_ui::fixture::FixtureArg;
        type Fx = crate::registry::AppFx;
        type Msg = crate::registry::AppMsg;
        type Elem = u32;
        type Views<'a> = (UiMode, &'a plx_media::route::PlaybackSession, plx_data::metadata::MetadataView<'a>);
        type Init = plx_ui::fixture::FixtureArg;
        type Memory = crate::registry::PageMemory;
        fn ui_mode(views: &Self::Views<'_>) -> UiMode { views.0 }
    }
    impl PlayerLike for ModeHost {
        fn session<'a>(cx: &Cx<'a, Self>) -> &'a plx_media::route::PlaybackSession { cx.views.1 }
    }
    impl crate::registry::MetadataLike for ModeHost {
        fn metadata<'a>(cx: &Cx<'a, Self>) -> plx_data::metadata::MetadataView<'a> { cx.views.2 }
    }
    #[test]
    fn volume_notice_keeps_classic_and_modern_pointer_and_focus_stops() {
        let _guard = plx_base::testlock::serial();
        let store = plx_data::stores::metadata::MetadataStore::default();
        for mode in [UiMode::Classic, UiMode::Modern] {
            let cx = Cx::<ModeHost> { views: (mode, plx_media::route::idle_session_for_test(), store.view()),
                tick: Default::default(), measure: &plx_ui::fixture::FixtureMeasure,
                press: Default::default(), focus: Default::default(), owner: InputOwner::Entry(EntryId(1)) };
            let mut page = PlayerScreen::new(EntryId(1));
            let mut before = DrawFrame::new(&cx, plx_ui::Painter::root());
            page.record_stops(&mut before, false);
            assert!(before.into_stops().is_empty());
            let mut groups_before = Vec::new();
            page.groups(&cx, &mut groups_before);
            let modal_before = plx_ui::popover::any_open();
            assert!(page.volume_notice(plx_media::player::volume::DesiredVolume::DEFAULT, 100));
            assert_eq!(plx_ui::popover::any_open(), modal_before, "notice is not an input owner");
            let mut after = DrawFrame::new(&cx, plx_ui::Painter::root());
            page.record_stops(&mut after, false);
            assert!(after.into_stops().is_empty(), "notice cannot add pointer targets");
            let mut groups_after = Vec::new();
            page.groups(&cx, &mut groups_after);
            assert_eq!(groups_before.iter().map(|g| (g.id, g.len)).collect::<Vec<_>>(),
                groups_after.iter().map(|g| (g.id, g.len)).collect::<Vec<_>>());
            page.clear_reminders();
        }
    }

    #[test]
    fn classic_pointer_stops_and_focus_use_the_reported_shifted_geometry() {
        let _guard = plx_base::testlock::serial();
        let store = plx_data::stores::metadata::MetadataStore::default();
        for scale in [20, 100, 200] {
            let mut measure = TypographyMeasure::new(plx_ui::fixture::FixtureMeasure);
            measure.set_font_scale(FontScale::from_percent(scale).unwrap());
            for mode in [UiMode::Classic, UiMode::Modern] {
                let cx = Cx::<ModeHost> { views: (mode, plx_media::route::idle_session_for_test(), store.view()),
                    tick: Default::default(), measure: &measure, press: Default::default(),
                    focus: Default::default(), owner: InputOwner::Entry(EntryId(1)) };
                let mut page = PlayerScreen::new(EntryId(1)); page.transport = true;
                if mode == UiMode::Modern { page.model.duration_ns = 120_000_000_000; }
                let geometry = page.hud_geometry(&cx);
                // Text prewarming deliberately does not publish pointer stops.
                let mut prewarm = DrawFrame::new(&cx, plx_ui::Painter::recording());
                page.record_stops(&mut prewarm, true);
                assert!(prewarm.into_stops().is_empty());
                // Exercise the same visible-pass stop registration as the hit map.
                let mut frame = DrawFrame::new(&cx, plx_ui::Painter::root());
                page.record_stops(&mut frame, true);
                let stops = frame.into_stops();
                let scrub = stops.iter().find(|stop| stop.key.elem == player_hud::ELEM_SCRUB).unwrap();
                assert_eq!(scrub.rect, geometry.grab);
                assert!(scrub.rect.contains(geometry.playbar.cx(), geometry.playbar.cy()));
                let placed = Focusable::<ModeHost>::place(&page, &player_hud::ELEM_SCRUB, &cx, At::Drawn).unwrap();
                assert_eq!(placed.rect, scrub.rect);
                for stop in stops.iter().filter(|stop| player_hud::row_index(stop.key.elem).is_some()) {
                    let i = player_hud::row_index(stop.key.elem).unwrap() as usize;
                    assert_eq!(stop.rect, geometry.items[i].unwrap());
                }
            }
        }
    }
    #[test]
    fn live_dvr_first_click_uses_painted_decoder_domain_and_readonly_guide_cannot_seek() {
        use plx_data::metadata::{NowPlaying, OttNowPlaying, OttLivePlaying, OttNowNext, OttGuideProgramme};
        use plx_machine::machine::{Effects, MachineId};
        use std::sync::atomic::Ordering::Relaxed;
        let _guard = plx_base::testlock::serial();
        struct RestorePlayerState { duration_ns: i64, hud_until: u32, scrub_ns: i64 }
        impl Drop for RestorePlayerState {
            fn drop(&mut self) {
                plx_media::player::SHARED.duration_ns.store(self.duration_ns, Relaxed);
                plx_media::player::TX.hud_until.store(self.hud_until, Relaxed);
                plx_media::player::TX.scrub_ns.store(self.scrub_ns, Relaxed);
            }
        }
        let _restore = RestorePlayerState {
            duration_ns: plx_media::player::duration_ns(),
            hud_until: plx_media::player::TX.hud_until.load(Relaxed),
            scrub_ns: plx_media::player::TX.scrub_ns.load(Relaxed),
        };
        const DECODER: i64 = 120_000_000_000;
        plx_media::player::SHARED.duration_ns.store(DECODER, Relaxed);
        let mut store = plx_data::stores::metadata::MetadataStore::default();
        store.run(plx_data::stores::metadata::MetadataCmd::SetNowPlaying(Some(NowPlaying {
            ott: Some(OttNowPlaying { archive: None, collection_label: "Channel A".into(), confirmed_start_unix_ms: None,
                next: None, art: None, live: Some(OttLivePlaying { channel_title: "Channel A".into(), ordinal: Some(7),
                    guide: Some(OttNowNext { now_s: 150, current: Some(OttGuideProgramme { title: "One hour programme".into(),
                        description: String::new(), start_s: 100, end_s: 3700 }), next: None }) }) }),
            is_episode: false, is_real_episode: false, title: "Channel A".into(), ep_title: String::new(),
            season: 0, index: 0, summary: String::new(), year: 0, dur_ms: 0,
            rating: String::new(), thumb: String::new(), detail_rk: String::new(),
        })));
        let measure = plx_ui::fixture::FixtureMeasure;
        let cx = Cx::<ModeHost> { views: (UiMode::Classic, plx_media::route::idle_session_for_test(), store.view()),
            tick: Default::default(), measure: &measure, press: Default::default(), focus: Default::default(),
            owner: InputOwner::Entry(EntryId(1)) };
        let mut page = PlayerScreen::new(EntryId(1)); page.transport = true;
        page.model.duration_ns = DECODER; page.model.playpos_ns = DECODER / 4; page.model.wall_now_s = Some(150);
        let geometry = page.hud_geometry(&cx);
        let commands = plx_ui::draw_census::capture(|| plx_ui::record_walk(||
            page.draw_hud_in_mode(cx.views.1, 1_000, &measure, store.view(), UiMode::Classic)));
        assert!(commands.iter().any(|(_, rect)| (rect.x - geometry.playbar.x).abs() < 0.01
            && (rect.y - geometry.playbar.y).abs() < 0.01
            && (rect.w - geometry.playbar.w * 0.25).abs() < 0.01
            && (rect.h - geometry.playbar.h).abs() < 0.01), "paint must use 30/120 decoder domain, not 50/3600 guide domain");
        assert_eq!(page.pointer_seek_duration(&cx), DECODER);
        let mut out = Vec::new(); let mut present = plx_machine::present::Present::new();
        let mut fx = Effects::<ModeHost>::new(&mut out, MachineId::Instance(InstanceId(1)), &mut present);
        page.handle_click(&cx, Some(player_hud::ELEM_SCRUB), geometry.playbar.cx(), 1_000, &mut fx);
        assert!(page.scrub.drag); assert_eq!(page.scrub.ns, DECODER / 2);
        page.scrub.cancel();
        page.publish();
        page.model.duration_ns = 0; page.model.scrub_ns = -1;
        assert!(!page.scrub.is_live());
        assert_eq!(plx_media::player::TX.scrub_ns.load(Relaxed), -1);
        assert_eq!(page.pointer_seek_duration(&cx), 0);
        page.handle_click(&cx, Some(player_hud::ELEM_SCRUB), geometry.playbar.cx(), 1_000, &mut fx);
        assert!(!page.scrub.drag, "stale seek hit cannot turn the read-only guide into a seek");
        let mut frame = DrawFrame::new(&cx, plx_ui::Painter::recording()); page.record_stops(&mut frame, true);
        assert!(!frame.into_stops().iter().any(|stop| stop.key.elem == player_hud::ELEM_SCRUB));
    }

    #[test]
    fn modern_focused_caption_keeps_pointer_and_focus_on_the_same_visible_rail() {
        let _guard = plx_base::testlock::serial();
        let store = plx_data::stores::metadata::MetadataStore::default();
        for scale in [100, 150, 200] {
            let mut measure = TypographyMeasure::new(plx_ui::fixture::FixtureMeasure);
            measure.set_font_scale(FontScale::from_percent(scale).unwrap());
            for active in [player_hud::BTN_SUBTITLES, player_hud::BTN_AUDIO, player_hud::BTN_MORE] {
                let cx = Cx::<ModeHost> { views: (UiMode::Modern, plx_media::route::idle_session_for_test(), store.view()),
                    tick: Default::default(), measure: &measure, press: Default::default(),
                    focus: Default::default(), owner: InputOwner::Entry(EntryId(1)) };
                let mut page = PlayerScreen::new(EntryId(1)); page.transport = true;
                page.hud.nav.focus = GROUP_ROW; page.hud.nav.btn = active;
                let geometry = page.hud_geometry(&cx);
                let focused = geometry.items[active as usize].unwrap();
                let other = if active == player_hud::BTN_AUDIO { player_hud::BTN_MORE } else { player_hud::BTN_AUDIO };
                assert!(focused.w > geometry.items[other as usize].unwrap().w, "the focused caption must have measured room");
                assert!(focused.x >= geometry.row.x && focused.x + focused.w <= geometry.row.x + geometry.row.w + 0.01);
                let key = player_hud::control_elem(active);
                let placed = Focusable::<ModeHost>::place(&page, &key, &cx, At::Drawn).unwrap();
                assert_eq!(placed.rect, focused);
                assert_eq!(placed.clip, geometry.row);
                let mut frame = DrawFrame::new(&cx, plx_ui::Painter::root());
                page.record_stops(&mut frame, true);
                let stops = frame.into_stops();
                let stop = stops.iter().find(|stop| stop.key.elem == key).unwrap();
                assert_eq!(stop.rect, focused);
                // Recording intersects the clip and recomputes its extent in f32.
                // Keep that rounding tolerance below one thousandth of a layout pixel.
                assert!(
                    (stop.clip.x - geometry.row.x).abs() < 0.001
                        && (stop.clip.y - geometry.row.y).abs() < 0.001
                        && (stop.clip.w - geometry.row.w).abs() < 0.001
                        && (stop.clip.h - geometry.row.h).abs() < 0.001,
                    "recorded clip {:?} must match rail {:?}", stop.clip, geometry.row
                );
            }
        }
    }

    #[test]
    fn modern_live_boundary_invalidates_only_its_visible_information_band() {
        use plx_data::metadata::{NowPlaying, OttNowPlaying, OttLivePlaying, OttNowNext, OttGuideProgramme};
        let _guard = plx_base::testlock::serial();
        let playing = NowPlaying {
            ott: Some(OttNowPlaying { live: Some(OttLivePlaying { channel_title: "Fixture channel".into(), ordinal: None,
                guide: Some(OttNowNext { now_s: 100,
                    current: Some(OttGuideProgramme { title: "Current".into(), description: String::new(), start_s: 90, end_s: 101 }),
                    next: Some(OttGuideProgramme { title: "Next".into(), description: String::new(), start_s: 101, end_s: 200 }) }) }),
                archive: None, collection_label: String::new(), confirmed_start_unix_ms: None, next: None, art: None }),
            is_episode: false, is_real_episode: false, title: "Fixture channel".into(), ep_title: String::new(),
            season: 0, index: 0, summary: String::new(), year: 0, dur_ms: 0,
            rating: String::new(), thumb: String::new(), detail_rk: String::new(),
        };
        let ps = plx_media::route::idle_session_for_test();
        let mut page = PlayerScreen::new(EntryId(1));
        page.transport = true; page.hud.until = 10_000; page.hud.dismissed = false; page.model.paused = true;
        page.model.wall_now_s = Some(100);
        let before = page.clock_fingerprint_in_mode(ps, 1_000, UiMode::Modern, Some(&playing));
        page.model.wall_now_s = Some(101);
        assert_ne!(before, page.clock_fingerprint_in_mode(ps, 1_000, UiMode::Modern, Some(&playing)), "paused decoder must not freeze the programme boundary");
        for (hidden, covered) in [(true, false), (false, true)] {
            page.hud.until = if hidden { 0 } else { 10_000 }; page.hud.dismissed = hidden; page.transport = !covered;
            let before = page.clock_fingerprint_in_mode(ps, 1_000, UiMode::Modern, Some(&playing));
            page.model.wall_now_s = Some(102);
            assert_eq!(before, page.clock_fingerprint_in_mode(ps, 1_000, UiMode::Modern, Some(&playing)));
            page.model.wall_now_s = Some(101);
        }
    }

    #[test]
    fn modern_unknown_duration_has_no_invisible_seek_hit_target() {
        let _guard = plx_base::testlock::serial();
        let store = plx_data::stores::metadata::MetadataStore::default();
        let measure = plx_ui::fixture::FixtureMeasure;
        let cx = Cx::<ModeHost> { views: (UiMode::Modern, plx_media::route::idle_session_for_test(), store.view()),
            tick: Default::default(), measure: &measure, press: Default::default(),
            focus: Default::default(), owner: InputOwner::Entry(EntryId(1)) };
        let mut page = PlayerScreen::new(EntryId(1)); page.transport = true;
        page.model.duration_ns = 0;
        let mut frame = DrawFrame::new(&cx, plx_ui::Painter::root());
        page.record_stops(&mut frame, true);
        assert!(!frame.into_stops().iter().any(|stop| stop.key.elem == player_hud::ELEM_SCRUB));
        assert_eq!(page.pointer_seek_duration(&cx), 0);
        page.model.duration_ns = 120_000_000_000;
        let mut frame = DrawFrame::new(&cx, plx_ui::Painter::root());
        page.record_stops(&mut frame, true);
        assert!(frame.into_stops().iter().any(|stop| stop.key.elem == player_hud::ELEM_SCRUB));
        assert_eq!(page.pointer_seek_duration(&cx), page.model.duration_ns);
    }

}

#[cfg(test)]
#[path = "classic_hidden_focus_tests.rs"]
mod classic_hidden_focus_tests;

#[cfg(test)]
#[path = "classic_menu_pointer_tests.rs"]
mod classic_menu_pointer_tests;

#[cfg(test)]
mod compact_failure_notice_tests {
    use super::*;
    #[test]
    fn compact_failure_auto_hides_and_new_attempt_or_input_redisplays_it() {
        let mut screen = PlayerScreen::new(plx_machine::machine::EntryId(1));
        screen.busy = player_hud::Busy::Readout(plx_ui::widgets::StatusKind::Failed, c"Playback failed");
        let first = Some((None, plx_media::player::FailureKind::MediaSource));
        screen.settle_failure_banner(u32::MAX - 2_000, first.clone());
        assert!(screen.failure_banner_visible());
        screen.settle_failure_banner(2_998, first.clone());
        assert!(screen.failure_banner_visible());
        screen.settle_failure_banner(2_999, first.clone());
        assert!(!screen.failure_banner_visible());
        screen.refresh_failure_banner(3_000);
        assert!(screen.failure_banner_visible());
        screen.settle_failure_banner(8_000, first);
        assert!(!screen.failure_banner_visible());
        screen.settle_failure_banner(8_001, Some((None, plx_media::player::FailureKind::TvPipeline)));
        assert!(screen.failure_banner_visible(), "a different failure is a new notice even without an idle frame");
        assert!(!player_hud::readout_owns_frame(screen.busy));
    }
    #[test]
    fn transient_error_never_registers_buttons_or_retains_failure_focus_after_hide() {
        use crate::player::overlay_tests::TestHost;
        use plx_ui::screen::{At, Focusable};
        let mut screen = PlayerScreen::new(plx_machine::machine::EntryId(1));
        screen.busy = player_hud::Busy::Readout(plx_ui::widgets::StatusKind::Failed, c"Playback failed");
        let attempt = Some((None, plx_media::player::FailureKind::MediaSource));
        screen.settle_failure_banner(1_000, attempt.clone());
        assert!(screen.failure_banner_visible());
        let _lock = plx_base::testlock::serial();
        let cx = Cx::<TestHost> { views: (), tick: Default::default(), measure: &plx_ui::fixture::FixtureMeasure,
            focus: Default::default(), press: Default::default(), owner: plx_machine::machine::InputOwner::Entry(screen.entry) };
        for key in player_hud::ELEM_FAILURE_BASE..player_hud::ELEM_FAILURE_END {
            assert!(Focusable::<TestHost>::group_of(&screen, &key, &cx).is_none());
            assert!(Focusable::<TestHost>::place(&screen, &key, &cx, At::Drawn).is_none());
        }
        assert!(!screen.group_drawn(GROUP_FAILURE, true));
        screen.settle_failure_banner(6_000, attempt);
        assert!(!screen.failure_banner_visible());
        assert!(!screen.group_drawn(GROUP_FAILURE, true));
    }

}
