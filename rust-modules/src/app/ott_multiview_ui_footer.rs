//! Each footer belongs to an admitted source ticket, independent of focus and audio.
use super::*;
use plx_data::metadata::{NowPlaying, OttLivePlaying, OttNextPlaying, OttNowNext, OttNowPlaying};
use plx_data::ott::guide::{Context, Query, Request};
use plx_data::ott::multiview::{ResolveTicket, SlotId};
use std::sync::mpsc::{self, Receiver, TryRecvError};

#[derive(Default)]
pub(super) struct Footers {
    slots: [Option<Footer>; 9],
    guide: Option<GuideJob>,
}

struct Footer {
    ticket: ResolveTicket,
    playing: NowPlaying,
    decoder_ns: Option<(i64, i64)>,
    wall_s: Option<i64>,
    guide_due_s: i64,
}

struct GuideJob {
    ticket: ResolveTicket,
    cancel: Arc<AtomicBool>,
    reply: Receiver<Option<OttNowNext>>,
}

impl Drop for GuideJob {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

fn time_ns(value: u64, units: i64) -> Option<i64> {
    i64::try_from(value).ok()?.checked_mul(units)
}

impl Footers {
    pub(super) fn refresh(
        &mut self,
        runtime: &Runtime,
        scene: Option<&multiview::SceneStatus>,
    ) -> bool {
        let wall_s = runtime.wall_now_s();
        let start_ms = wall_s.and_then(|_| i64::try_from(runtime.wall_clock_ms).ok());
        let local = runtime.multiview_local.active();
        let mut inputs = [None; 9];
        if let Some(scene) = scene {
            for slot in &scene.slots {
                if !matches!(slot.status, "playing" | "paused" | "stalled") {
                    continue;
                }
                let sample = if local {
                    runtime
                        .multiview_local
                        .footer_input(runtime, slot.slot)
                        .map(|(input, pos_us, length_ms)| {
                            (
                                input,
                                u64::try_from(pos_us)
                                    .ok()
                                    .and_then(|pos| time_ns(pos, 1_000)),
                                length_ms,
                            )
                        })
                } else {
                    runtime
                        .multiview_driver
                        .footer_input(runtime, slot.slot)
                        .map(|(input, pos_ms, length_ms)| {
                            (
                                input,
                                pos_ms.and_then(|pos| time_ns(pos, 1_000_000)),
                                length_ms,
                            )
                        })
                };
                inputs[slot.slot.index()] = sample;
            }
        }

        let mut changed = false;
        for (index, (footer, sample)) in self
            .slots
            .iter_mut()
            .zip(inputs.iter().copied())
            .enumerate()
        {
            let Some((input, position, duration_ms)) = sample else {
                changed |= footer.take().is_some();
                continue;
            };
            let duration_ms = duration_ms.filter(|ms| *ms > 0).or(input.entry.duration_ms);
            if footer
                .as_ref()
                .is_none_or(|footer| footer.ticket != input.ticket)
            {
                let next = local
                    .then(|| next_source(runtime, input.ticket.slot()))
                    .flatten();
                *footer = Some(Footer {
                    ticket: input.ticket,
                    playing: source_playing(
                        runtime,
                        input,
                        local.then_some(start_ms).flatten(),
                        duration_ms,
                        next,
                    ),
                    decoder_ns: None,
                    wall_s: None,
                    guide_due_s: 0,
                });
                changed = true;
            }
            let footer = footer.as_mut().expect("admitted footer");
            let info = footer.playing.ott.as_mut().expect("OTT source projection");
            let dur_ms = if info.live.is_some() {
                0
            } else {
                duration_ms
                    .and_then(|length| i64::try_from(length).ok())
                    .unwrap_or(0)
            };
            let decoder = (dur_ms > 0)
                .then(|| position.zip(time_ns(dur_ms as u64, 1_000_000)))
                .flatten();
            // Refresh display seconds without spending a fresh input-offer serial.
            let clock_key =
                |clock: Option<(i64, i64)>| clock.map(|(pos, len)| (pos / 1_000_000_000, len));
            if footer.wall_s != wall_s || clock_key(footer.decoder_ns) != clock_key(decoder) {
                footer.wall_s = wall_s;
                footer.decoder_ns = decoder;
                changed = true;
            }
            if footer.playing.dur_ms != dur_ms {
                footer.playing.dur_ms = dur_ms;
                if let Some(next) = &mut info.next {
                    next.estimated_start_unix_ms =
                        estimate_next(info.confirmed_start_unix_ms, dur_ms);
                }
                changed = true;
            }
            debug_assert_eq!(footer.ticket.slot().index(), index);
        }
        changed
            | self.refresh_guide(
                inputs.map(|input| input.map(|(input, ..)| input)),
                runtime,
                wall_s,
            )
    }

    fn refresh_guide(
        &mut self,
        inputs: [Option<&multiview_sources::AuthorizedInput>; 9],
        runtime: &Runtime,
        wall_s: Option<i64>,
    ) -> bool {
        let mut changed = false;
        if let Some(job) = &self.guide {
            let current = self.slots[job.ticket.slot().index()]
                .as_mut()
                .filter(|footer| footer.ticket == job.ticket);
            if current.is_none() {
                job.cancel.store(true, Ordering::Release);
            }
            match job.reply.try_recv() {
                Ok(reply) => {
                    if let (Some(footer), Some(facts)) = (current, reply) {
                        changed = install_guide(footer, job.ticket, facts);
                    }
                    self.guide = None;
                }
                Err(TryRecvError::Disconnected) => self.guide = None,
                Err(TryRecvError::Empty) => return changed,
            }
        }
        let Some(now) = wall_s else {
            return changed;
        };
        let candidate = self
            .slots
            .iter_mut()
            .filter_map(Option::as_mut)
            .filter(|footer| {
                footer.guide_due_s <= now
                    && footer
                        .playing
                        .ott
                        .as_ref()
                        .is_some_and(|info| info.live.is_some())
            })
            .min_by_key(|footer| footer.guide_due_s);
        if let Some(footer) = candidate {
            if let Some(input) = inputs[footer.ticket.slot().index()] {
                footer.guide_due_s = now.saturating_add(60);
                self.guide = start_guide(runtime, input, now);
            }
        }
        changed
    }

    pub(super) fn project(&self, frame: &mut plx_appkit::multiview_chrome::ChromeFrame) {
        for footer in self.slots.iter().flatten() {
            let slot = footer.ticket.slot().index() as u8;
            frame.set_footer(slot, footer.playing.clone());
            frame.set_footer_identity(slot, footer.ticket.source(), footer.ticket.media());
            frame.set_footer_clock(slot, footer.decoder_ns, footer.wall_s);
        }
    }
}

fn estimate_next(start_ms: Option<i64>, dur_ms: i64) -> Option<i64> {
    start_ms.filter(|_| dur_ms > 0)?.checked_add(dur_ms)
}

fn next_source(runtime: &Runtime, slot: SlotId) -> Option<String> {
    let next = runtime.multiview.local_step_source(slot, None, 1)?;
    let previous = runtime.multiview.local_step_source(slot, None, 0)?;
    if next == previous {
        return None;
    }
    runtime
        .multiview
        .source_rows()
        .into_iter()
        .find(|row| row.handle == next)
        .map(|row| row.title.clone())
}

fn source_playing(
    runtime: &Runtime,
    input: &multiview_sources::AuthorizedInput,
    started_ms: Option<i64>,
    duration_ms: Option<u64>,
    next: Option<String>,
) -> NowPlaying {
    let entry = &input.entry;
    let profile = input.profile();
    let live = live_witness::live_source(profile, entry).then(|| OttLivePlaying {
        channel_title: entry.title.clone(),
        ordinal: input.ordinal,
        guide: None,
    });
    let dur_ms = if live.is_some() {
        0
    } else {
        duration_ms
            .and_then(|duration| i64::try_from(duration).ok())
            .unwrap_or(0)
    };
    let collection_label = if live.is_some() {
        &entry.title
    } else {
        input.footer_collection(runtime)
    };
    let next = live
        .is_none()
        .then_some(next)
        .flatten()
        .map(|title| OttNextPlaying {
            title,
            estimated_start_unix_ms: estimate_next(started_ms, dur_ms),
        });
    NowPlaying {
        ott: Some(OttNowPlaying {
            live,
            archive: None,
            collection_label: collection_label.into(),
            confirmed_start_unix_ms: started_ms,
            next,
            art: entry_poster(entry),
        }),
        is_episode: false,
        is_real_episode: false,
        title: entry.title.clone(),
        ep_title: String::new(),
        season: 0,
        index: 0,
        summary: entry.description.clone(),
        year: 0,
        dur_ms,
        rating: String::new(),
        thumb: String::new(),
        detail_rk: String::new(),
    }
}

fn start_guide(
    runtime: &Runtime,
    input: &multiview_sources::AuthorizedInput,
    now: i64,
) -> Option<GuideJob> {
    let profile = input.profile().clone();
    let entry = input.entry.clone();
    let hosted = runtime.hosted_epg_config.clone();
    let request = Request {
        context: Context {
            generation: runtime.generation,
            profile_id: profile.id.clone(),
            entry_id: entry.id.clone(),
        },
        query: Query::CurrentNext { now }.validate().ok()?,
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    let (tx, reply) = mpsc::channel();
    plx_base::task::spawn_off_frame("ott-multiview-guide", move |worker| {
        let result = super::super::ott_provider_http::guide_configured_cancellable(
            worker,
            &profile,
            &entry,
            &request,
            hosted.as_ref(),
            &worker_cancel,
        )
        .ok()
        .filter(|result| result.can_publish(&request, worker_cancel.load(Ordering::Acquire)))
        .and_then(|result| guide_projection::current_next(&result, now));
        if tx.send(result).is_ok() {
            plx_machine::idle::invalidate();
        }
    })
    .then_some(GuideJob {
        ticket: input.ticket,
        cancel,
        reply,
    })
}

fn install_guide(footer: &mut Footer, ticket: ResolveTicket, facts: OttNowNext) -> bool {
    if footer.ticket != ticket {
        return false;
    }
    let Some(live) = footer
        .playing
        .ott
        .as_mut()
        .and_then(|info| info.live.as_mut())
    else {
        return false;
    };
    if live.guide.as_ref() == Some(&facts) {
        return false;
    }
    live.guide = Some(facts);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use plx_data::metadata::OttGuideProgramme;
    use plx_data::ott::multiview::{Layout, MediaRef, Scene, SceneId, SourceRef};
    use std::num::NonZeroU64;

    fn footer(ticket: ResolveTicket) -> Footer {
        Footer {
            ticket,
            playing: NowPlaying {
                ott: Some(OttNowPlaying {
                    live: Some(OttLivePlaying {
                        channel_title: "News".into(),
                        ordinal: Some(106),
                        guide: None,
                    }),
                    archive: None,
                    collection_label: "News".into(),
                    confirmed_start_unix_ms: None,
                    next: None,
                    art: None,
                }),
                is_episode: false,
                is_real_episode: false,
                title: "News".into(),
                ep_title: String::new(),
                season: 0,
                index: 0,
                summary: String::new(),
                year: 0,
                dur_ms: 0,
                rating: String::new(),
                thumb: String::new(),
                detail_rk: String::new(),
            },
            decoder_ns: None,
            wall_s: None,
            guide_due_s: 0,
        }
    }

    #[test]
    fn guide_cannot_cross_windows_or_reused_source_tickets() {
        let token = |value| NonZeroU64::new(value).unwrap();
        let source = SourceRef::from_opaque(token(17));
        let media = MediaRef::from_opaque(token(18));
        let mut scene = Scene::new(SceneId::from_opaque(token(19)));
        scene.set_layout(Layout::Grid4).unwrap();
        for slot in [SlotId::SLOT_0, SlotId::SLOT_1] {
            scene.bind_source(slot, source).unwrap();
        }
        let a = scene
            .begin_resolution(SlotId::SLOT_0, source, media)
            .unwrap();
        let b = scene
            .begin_resolution(SlotId::SLOT_1, source, media)
            .unwrap();
        let new_a = scene
            .begin_resolution(SlotId::SLOT_0, source, media)
            .unwrap();
        let facts = OttNowNext {
            now_s: 120,
            current: Some(OttGuideProgramme {
                title: "Morning news".into(),
                description: String::new(),
                start_s: 100,
                end_s: 140,
            }),
            next: Some(OttGuideProgramme {
                title: "Weather".into(),
                description: String::new(),
                start_s: 140,
                end_s: 180,
            }),
        };
        let mut actual = footer(new_a);
        assert!(!install_guide(&mut actual, a, facts.clone()));
        assert!(!install_guide(&mut actual, b, facts.clone()));
        assert!(install_guide(&mut actual, new_a, facts.clone()));
        assert!(!install_guide(&mut actual, new_a, facts.clone()));
        let live = actual.playing.ott.unwrap().live.unwrap();
        assert_eq!(live.ordinal, Some(106));
        assert_eq!(live.guide, Some(facts));
    }

    #[test]
    fn per_slot_clocks_require_real_duration_and_do_not_overflow() {
        assert_eq!(time_ns(2_000_001, 1_000), Some(2_000_001_000));
        assert_eq!(time_ns(90_001, 1_000_000), Some(90_001_000_000));
        assert_eq!(time_ns(u64::MAX, 1_000_000), None);
        assert_eq!(estimate_next(Some(3_000_000), 90_001), Some(3_090_001));
        assert_eq!(estimate_next(None, 90_001), None);
        assert_eq!(estimate_next(Some(3_000_000), 0), None);
        assert_eq!(estimate_next(Some(i64::MAX), 1), None);
    }
}
