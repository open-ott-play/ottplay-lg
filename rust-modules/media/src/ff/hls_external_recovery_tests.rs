//! Exercise the actual external interval feeder, bounded FIFOs and native Play.
use super::*;
use crate::player::{engine,ffi_host,TX};
use plx_base::task::MainThread;
use plx_platform::tv::sink::VideoSink;
use std::time::{Duration,Instant};
fn interval()->HlsSegmentOutput {
 let mut output=HlsSegmentOutput {resident:None,transport_timing:None,webvtt_clock:None,observed:None,
  aus:[(1,1_600_000_000),(2,1_600_000_000),(1,1_640_000_000),(2,1_640_000_000)].into_iter()
   .map(|(es,pts_ns)|HlsAu{data:vec![0;64],pts_ns,key:1,es}).collect(),
  transfer:SegmentTransfer{bytes:256,active_us:100_000,total_us:100_000,audio_expected:true},
  video_width:160,video_height:90,video_tail_ns:1_680_000_000,audio_tail_ns:Some(1_680_000_000)};
 fixture_resident(&mut output,40_000_000);output
}
// These mock encoded samples have an explicit fixture duration. Production
// obtains durations from packets/codec frames; timestamp spacing is not proof.
fn fixture_resident(output:&mut HlsSegmentOutput,duration_ns:i64){
 let lane=|es| {
  let mut samples:Vec<_>=output.aus.iter().filter(|au|au.es==es&&!au.data.is_empty())
   .map(|au|(au.pts_ns,au.pts_ns+duration_ns)).collect();samples.sort_unstable();
  let mut ranges:Vec<(i64,i64)>=Vec::new();
  for (first,last) in samples {if let Some(previous)=ranges.last_mut(){if first<=previous.1 {previous.1=previous.1.max(last);continue;}}ranges.push((first,last));}
  std::sync::Arc::from(ranges)
 };
 output.resident=Some(crate::player::ExternalHlsResident{video:lane(1),audio:lane(2)});
}
fn fixture(held:bool)->(engine::Engine,MainThread,u64){
 SHARED.reset_session();TX.reset();
 ffi_host::force_clocksink_for_test(true);
 let native=SHARED.begin_native_session().unwrap();assert!(SHARED.mark_native_load_returned(native,None).is_some());
 let mut eng=engine::engine_after_reload_for_test();eng.native_epoch=native;
 for slot in [&mut eng.aq_video,&mut eng.aq_audio]{
  if let Some(mut q)=slot.take(){crate::aq::aq_destroy(&mut *q);}*slot=Some(crate::aq::aq_new(64));
 }
 let mt=unsafe{MainThread::assume()};let generation=SHARED.hls_candidate_generation.load(Ordering::Acquire);
 let(token,_)=SHARED.reserve_hls_prime_play(SHARED.hls_prime_kind().expect("initial hold"),generation,SHARED.hls_recovery()).unwrap();
 let played=unsafe{ffi_host::HostSink.play(&mt)};
 let _=SHARED.complete_hls_prime_play(token,played!=0);
 if held { pause(&mt); }
 eng.prime_play=true;eng.max_fed_video_pts=1_000_000_000;eng.max_fed_audio_pts=1_000_000_000;
 SHARED.hls_video_tail_ns.store(1_000_000_000,Ordering::Release);SHARED.hls_audio_tail_ns.store(1_000_000_000,Ordering::Release);
 SHARED.hls_audio_expected.store(true,Ordering::Release);SHARED.seek_to_ns.store(-1,Ordering::Release);
 (eng,mt,generation)
}
fn pause(mt:&MainThread){
 let token=SHARED.prepare_hls_rebuffer_pause().unwrap();let paused=unsafe{ffi_host::HostSink.pause(mt)};
 let _=SHARED.complete_hls_rebuffer_pause(token,paused!=0);
 assert!(SHARED.hls_rebuffering.load(Ordering::Acquire));
}
fn wait_retained(){let until=Instant::now()+Duration::from_secs(2);
 while !SHARED.has_retained_external_hls_interval_for_test() && Instant::now()<until {std::thread::sleep(Duration::from_millis(1));}
 assert!(SHARED.has_retained_external_hls_interval_for_test(),"actual feeder reached the full FIFO");
}
fn pointers(eng:&mut engine::Engine)->(*mut AuQueue,*mut AuQueue){
 (&mut **eng.aq_video.as_mut().unwrap(),&mut **eng.aq_audio.as_mut().unwrap())
}
fn clear(eng:&mut engine::Engine){
 for mut q in [eng.aq_video.take(),eng.aq_audio.take()].into_iter().flatten(){
  crate::aq::aq_abort(&mut *q);
  loop{let n=crate::aq::aq_pop(&mut *q,std::ptr::null_mut());if n.is_null(){break;}unsafe{libc::free(n.cast())};}
  crate::aq::aq_destroy(&mut *q);
 }ffi_host::force_clocksink_for_test(false);
 SHARED.retire_native_session(eng.native_epoch);SHARED.reset_session();TX.reset();
}
struct Producer{join:Option<std::thread::JoinHandle<Result<bool,HlsExit>>>,video:usize,audio:usize}
impl Drop for Producer{fn drop(&mut self){
 crate::aq::aq_abort(self.video as *mut AuQueue);crate::aq::aq_abort(self.audio as *mut AuQueue);
 if let Some(j)=self.join.take(){let _=j.join();}
}}
fn producer(v:*mut AuQueue,a:*mut AuQueue,g:u64,acquisition_us:u64)->Producer{
 let(v,a)=(v as usize,a as usize);
 Producer{video:v,audio:a,join:Some(std::thread::spawn(move||{
  let mut output=interval();output.transfer.total_us=acquisition_us;
  hls_feed_external_segment(&output,Duration::from_secs(2),g,v as *mut AuQueue,a as *mut AuQueue)}))}
}
fn wait_credit(){let until=Instant::now()+Duration::from_secs(2);
 while SHARED.hls_recovery().completed==0 && Instant::now()<until{std::thread::sleep(Duration::from_millis(1));}
 assert_eq!(SHARED.hls_recovery().completed,1,"real producer must credit before its bounded push parks");
}
#[test]
fn retained_external_segment_restarts_held_clock_before_full_fifo_can_finish(){
 let _serial=plx_base::testlock::serial();
 for held_before_feeding in [true,false] {
  let(mut eng,mt,g)=fixture(held_before_feeding);let(v,a)=pointers(&mut eng);
  let bytes=[0u8;64];for pts in [1_400_000_000,1_500_000_000]{assert_eq!(crate::aq::aq_push(v,bytes.as_ptr(),64,pts,1,1),0);}
  SHARED.hls_video_tail_ns.store(1_500_000_000,Ordering::Release);
  assert!(crate::aq::aq_bytes(v)>crate::aq::aq_cap(v));let mut work=producer(v,a,g,100_000);wait_retained();
  assert!(!work.join.as_ref().unwrap().is_finished(),"FIFO genuinely blocks the producer");
  if !held_before_feeding {
   assert!(!SHARED.hls_rebuffering.load(Ordering::Acquire));
   assert_eq!(SHARED.hls_recovery().completed,0,"retention while running is not an active recovery");
   pause(&mt); // The producer was already parked BEFORE the native pause.
  }
  wait_credit();let mut volume=crate::player::volume::VolumeState::new();
  engine::try_prime(&mt,&mut eng,&mut volume);
  assert!(!eng.prime_play,"actual native Play must release the clock");assert!(!SHARED.hls_rebuffering.load(Ordering::Acquire));
  let until=Instant::now()+Duration::from_secs(2);
  while !work.join.as_ref().unwrap().is_finished() && Instant::now()<until{
   engine::feed_both_lanes(&mt,&mut eng,&mut volume);std::thread::sleep(Duration::from_millis(1));
  }
  assert!(work.join.as_ref().unwrap().is_finished(),"both bounded lanes drain after Play");
  assert!(matches!(work.join.take().unwrap().join().unwrap(),Ok(true)));drop(work);
  assert!(!SHARED.has_retained_external_hls_interval_for_test(),"finished feeder releases its proof");
  pause(&mt);assert_eq!(SHARED.hls_recovery().completed,0,"completed historical intervals cannot seed a later hold");
  clear(&mut eng);
 }
}
#[test]
fn aborted_seek_stale_or_incomplete_external_interval_cannot_credit_recovery(){
 let _serial=plx_base::testlock::serial();
 for kind in ["abort","seek","stale"]{
  let(mut eng,_mt,g)=fixture(true);let(v,a)=pointers(&mut eng);let output=interval();
  if kind=="abort"{crate::aq::aq_abort(v);}if kind=="seek"{SHARED.seek_to_ns.store(1,Ordering::Release);}
  let owner=if kind=="stale"{g.wrapping_sub(2)}else{g};
  let result=hls_feed_external_segment(&output,Duration::from_secs(2),owner,v,a);
  assert!(!matches!(result,Ok(true)),"{kind} interval cannot enter active queues");
  assert_eq!(SHARED.hls_recovery().completed,0,"{kind} cannot authorize Play");
  assert!(SHARED.hls_rebuffering.load(Ordering::Acquire));clear(&mut eng);
 }
}
#[test]
fn retained_credit_cannot_invent_queued_or_fed_playable_media(){
 let _serial=plx_base::testlock::serial();let(mut eng,mt,g)=fixture(true);let(v,a)=pointers(&mut eng);
 // Both native lanes already satisfy decoder prime (1 s), but not the 1.2 s
 // acquisition runway. Publishing the retained future tails would wrongly Play.
 SHARED.hls_video_tail_ns.store(1_040_000_000,Ordering::Release);let bytes=[0u8;64];
 for pts in [1_000_000_000,1_040_000_000]{assert_eq!(crate::aq::aq_push(v,bytes.as_ptr(),64,pts,1,1),0);}
 let work=producer(v,a,g,1_200_000);wait_credit();
 engine::try_prime(&mt,&mut eng,&mut crate::player::volume::VolumeState::new());
 assert!(eng.prime_play,"retained output is not accepted decoder or queued runway");
 assert!(SHARED.hls_rebuffering.load(Ordering::Acquire));drop(work);clear(&mut eng);
}

#[test]
fn muxed_interval_without_audio_keeps_feeding_without_claiming_joined_recovery(){
 let _serial=plx_base::testlock::serial();
 for held in [false,true]{
  let(mut eng,_mt,g)=fixture(held);let(v,a)=pointers(&mut eng);let mut output=interval();
  output.aus.retain(|au|au.es==1);output.audio_tail_ns=None;
  assert!(matches!(hls_feed_external_segment(&output,Duration::from_secs(2),g,v,a),Ok(true)));
  assert_eq!(SHARED.hls_recovery().completed,0,"missing declared lane is not joined evidence");
  assert!(!SHARED.has_retained_external_hls_interval_for_test());
  assert!(SHARED.hls_audio_expected.load(Ordering::Acquire));
  clear(&mut eng);
 }
}

#[test]
fn fresh_external_hls_starts_from_real_queued_runway_when_paused_decoder_is_full() {
 let _serial=plx_base::testlock::serial();
 SHARED.reset_session();TX.reset();ffi_host::force_clocksink_for_test(true);
 let native=SHARED.begin_native_session().unwrap();assert!(SHARED.mark_native_load_returned(native,None).is_some());
 let mut eng=engine::engine_after_reload_for_test();eng.native_epoch=native;
 for slot in [&mut eng.aq_video,&mut eng.aq_audio]{
  if let Some(mut q)=slot.take(){crate::aq::aq_destroy(&mut *q);}*slot=Some(crate::aq::aq_new(960));
 }
 let cap=ffi_host::held_feed_capacity_for_test(1_000_000_000);
 let mt=unsafe{MainThread::assume()};let g=SHARED.hls_candidate_generation.load(Ordering::Acquire);
 assert!(SHARED.hls_prime_kind().is_some());
 let(v,a)=pointers(&mut eng);let(vp,ap)=(v as usize,a as usize);
 let mut work=Producer{video:vp,audio:ap,join:Some(std::thread::spawn(move||{
  let mut output=interval();output.transfer.total_us=2_000_000;
  output.aus=(0..33).flat_map(|n|[1,2].map(move|es|HlsAu{data:vec![0;64],pts_ns:n*100_000_000,key:1,es})).collect();
  output.video_tail_ns=3_300_000_000;output.audio_tail_ns=Some(3_300_000_000);
  fixture_resident(&mut output,100_000_000);
  hls_feed_external_segment(&output,Duration::from_millis(3300),g,vp as *mut AuQueue,ap as *mut AuQueue)
 }))};
 let plays=ffi_host::play_calls_for_test();let mut volume=crate::player::volume::VolumeState::new();
 let until=Instant::now()+Duration::from_secs(2);
 while Instant::now()<until {
  engine::feed_both_lanes(&mt,&mut eng,&mut volume);
  if !eng.prime_play || (ffi_host::held_feed_full_for_test()>2
   && SHARED.hls_video_tail_ns.load(Ordering::Acquire)>=2_000_000_000
   && SHARED.hls_audio_tail_ns.load(Ordering::Acquire)>=2_000_000_000){break;}
  std::thread::sleep(Duration::from_millis(1));
 }
 let accepted=(eng.max_fed_video_pts,eng.max_fed_audio_pts);
 let tails=(SHARED.hls_video_tail_ns.load(Ordering::Acquire),SHARED.hls_audio_tail_ns.load(Ordering::Acquire));
 let full=ffi_host::held_feed_full_for_test();let played=ffi_host::play_calls_for_test()>plays;
 let retained=SHARED.has_retained_external_hls_interval_for_test();
 if played {
  let until=Instant::now()+Duration::from_secs(2);
  while !work.join.as_ref().unwrap().is_finished()&&Instant::now()<until {engine::feed_both_lanes(&mt,&mut eng,&mut volume);std::thread::sleep(Duration::from_millis(1));}
 }
 let producer_drained=work.join.as_ref().unwrap().is_finished();
 if producer_drained {let _=work.join.take().unwrap().join();}
 drop(work);drop(cap);clear(&mut eng);
 assert!(full>0,"real Feed must encounter paused native BufferFull");
 assert!(accepted.0>=700_000_000&&accepted.1>=300_000_000,"decoder prime accepted: {accepted:?}");
 assert!(tails.0>0&&tails.1>0,"real queued prefix: {tails:?}");
 assert!(accepted.0<2_000_000_000&&accepted.1<2_000_000_000,"native capacity alone cannot cover acquisition");
 assert!(played,"Fresh held forever at accepted {accepted:?}, queued {tails:?}, BufferFull={full}, retained={retained}");
 assert!(producer_drained,"Play must let the bounded producer drain");
}

#[test]
fn fresh_external_queued_runway_cannot_bypass_source_or_decoder_guards() {
 let _serial=plx_base::testlock::serial();
 for case in ["no-lease","stale","seek","reset","audio-starved","abort","short-runway","viewer-pause","false-metadata"] {
  SHARED.reset_session();TX.reset();ffi_host::force_clocksink_for_test(true);
  let native=SHARED.begin_native_session().unwrap();assert!(SHARED.mark_native_load_returned(native,None).is_some());
  let mut eng=engine::engine_after_reload_for_test();eng.native_epoch=native;
  for slot in [&mut eng.aq_video,&mut eng.aq_audio]{
   if let Some(mut q)=slot.take(){crate::aq::aq_destroy(&mut *q);}*slot=Some(crate::aq::aq_new(if case=="short-runway"{64}else{960}));
  }
  let cap=ffi_host::held_feed_capacity_for_lanes_for_test(1_000_000_000,if case=="audio-starved"{100_000_000}else{1_000_000_000});
  let mt=unsafe{MainThread::assume()};let g=SHARED.hls_candidate_generation.load(Ordering::Acquire);
  // Fill accepted decoder prefixes and real queues without letting the test's
  // setup race production prime. This is the existing trial gate, not a clock.
  SHARED.hls_trial_reserve_ms.store(0,Ordering::Release);
  let(v,a)=pointers(&mut eng);let(vp,ap)=(v as usize,a as usize);let uncredited=case=="no-lease";let short=matches!(case,"short-runway"|"false-metadata");
  let work=Producer{video:vp,audio:ap,join:Some(std::thread::spawn(move||{
   let mut output=interval();output.transfer.total_us=2_000_000;
   output.aus=(0..33).flat_map(|n|[1,2].map(move|es|HlsAu{data:vec![0;64],pts_ns:n*100_000_000,key:1,es})).collect();
   output.video_tail_ns=3_300_000_000;output.audio_tail_ns=Some(3_300_000_000);
  fixture_resident(&mut output,100_000_000);
   if short { output.aus.retain(|au|au.pts_ns<=1_100_000_000); }
   if short { output.video_tail_ns=60_000_000_000;output.audio_tail_ns=Some(60_000_000_000); }
   fixture_resident(&mut output,100_000_000);
   if uncredited { hls_feed_segment_interruptible(&output,vp as *mut AuQueue,ap as *mut AuQueue,true) }
   else { hls_feed_external_segment(&output,Duration::from_millis(3300),g,vp as *mut AuQueue,ap as *mut AuQueue) }
  }))};
  let mut volume=crate::player::volume::VolumeState::new();let until=Instant::now()+Duration::from_secs(2);
  while Instant::now()<until {
   engine::feed_both_lanes(&mt,&mut eng,&mut volume);
   if ffi_host::held_feed_full_for_test()>4 {break;}
   std::thread::sleep(Duration::from_millis(1));
  }
  let full=ffi_host::held_feed_full_for_test();let accepted=(eng.max_fed_video_pts,eng.max_fed_audio_pts);
  match case {
   "stale"=>{SHARED.hls_candidate_generation.fetch_add(2,Ordering::AcqRel);},
   "seek"=>SHARED.publish_seek_target(2_000_000_000),
   "reset"=>{SHARED.reset_hls_clock_for_test();SHARED.ensure_initial_clock_hold_for_test();},
   "abort"=>crate::aq::aq_abort(v),
   "viewer-pause"=>{let _=SHARED.prepare_hls_user_pause();},
   _=>{}
  }
  SHARED.hls_trial_reserve_ms.store(-1,Ordering::Release);
  let plays=ffi_host::play_calls_for_test();
  for _ in 0..5 {engine::feed_both_lanes(&mt,&mut eng,&mut volume);}
  let played=ffi_host::play_calls_for_test()!=plays;
  drop(work);drop(cap);clear(&mut eng);
  assert!(full>0,"{case}: genuine native BufferFull");
  assert!(accepted.0>=700_000_000,"{case}: real video decoder prefix {accepted:?}");
  if case=="audio-starved" {assert!(accepted.1<300_000_000,"real audio stays below decoder prime");}
  else {assert!(accepted.1>=300_000_000,"{case}: real audio decoder prefix {accepted:?}");}
  assert!(!played,"{case}: unauthorized queued prefix started native Play");
 }
}

#[test]
fn completed_finite_external_segment_can_prime_after_its_producer_has_returned() {
 let _serial=plx_base::testlock::serial();SHARED.reset_session();TX.reset();ffi_host::force_clocksink_for_test(true);
 let native=SHARED.begin_native_session().unwrap();assert!(SHARED.mark_native_load_returned(native,None).is_some());
 let mut eng=engine::engine_after_reload_for_test();eng.native_epoch=native;
 let cap=ffi_host::held_feed_capacity_for_test(1_000_000_000);
 let mt=unsafe{MainThread::assume()};let g=SHARED.hls_candidate_generation.load(Ordering::Acquire);let(v,a)=pointers(&mut eng);
 let mut output=interval();output.transfer.total_us=2_000_000;
 output.aus=(0..33).flat_map(|n|[1,2].map(move|es|HlsAu{data:vec![0;64],pts_ns:n*100_000_000,key:1,es})).collect();
 output.video_tail_ns=3_300_000_000;output.audio_tail_ns=Some(3_300_000_000);
  fixture_resident(&mut output,100_000_000);
 assert!(matches!(hls_feed_external_segment(&output,Duration::from_millis(3300),g,v,a),Ok(true)));
 assert!(!SHARED.has_retained_external_hls_interval_for_test(),"producer already returned");
 assert!(SHARED.external_hls_fresh_proof(g),"the complete push transferred evidence to real queues");
 crate::aq::aq_set_eof(v);crate::aq::aq_set_eof(a);
 let plays=ffi_host::play_calls_for_test();engine::feed_both_lanes(&mt,&mut eng,&mut crate::player::volume::VolumeState::new());
 let played=ffi_host::play_calls_for_test()>plays;let full=ffi_host::held_feed_full_for_test();
 let evidence=SHARED.external_hls_fresh_proof(g);let pending=eng.prime_play;
 if played {pause(&mt);assert_eq!(SHARED.hls_recovery().completed,0,"completed historical proof cannot seed Rebuffer");}
 drop(cap);clear(&mut eng);
 assert!(full>0,"real paused native decoder was full");
 assert!(played&&!pending,"finished finite producer must not strand Fresh at native capacity");
 assert!(!evidence,"accepted first Play consumed completed Fresh evidence");
}

#[test]
fn external_resident_segment_larger_than_native_and_queues_can_start_all_external_holds() {
 let _serial=plx_base::testlock::serial();
 let mut stalled=Vec::new();
 for hold in ["fresh","rebuffer","resume"] {
 SHARED.reset_session();TX.reset();ffi_host::force_clocksink_for_test(true);
 let native=SHARED.begin_native_session().unwrap();assert!(SHARED.mark_native_load_returned(native,None).is_some());
 let mut eng=engine::engine_after_reload_for_test();eng.native_epoch=native;
 for slot in [&mut eng.aq_video,&mut eng.aq_audio]{
  if let Some(mut q)=slot.take(){crate::aq::aq_destroy(&mut *q);}*slot=Some(crate::aq::aq_new(960));
 }
 let mt=unsafe{MainThread::assume()};let g=SHARED.hls_candidate_generation.load(Ordering::Acquire);
 if hold!="fresh" {
  let(play,_)=SHARED.reserve_hls_prime_play(SHARED.hls_prime_kind().unwrap(),g,SHARED.hls_recovery()).unwrap();
  assert_ne!(unsafe{ffi_host::HostSink.play(&mt)},0);
  let _=SHARED.complete_hls_prime_play(play,true);
  if hold=="rebuffer" {pause(&mt);} else {
   let token=SHARED.prepare_hls_user_pause().expect("user pause");
   // Use the existing main-thread protocol helper's Debug-safe issue token.
   if let crate::player::HlsUserPause::Issue(token)=token {
    assert_ne!(unsafe{ffi_host::HostSink.pause(&mt)},0);
    let _=SHARED.complete_hls_user_pause(token,true);
   }
   assert!(SHARED.prepare_hls_user_resume(true).is_some());
  }
 }
 let cap=ffi_host::held_feed_capacity_for_test(1_000_000_000);
 assert!(SHARED.hls_prime_kind().is_some());
 let(v,a)=pointers(&mut eng);let(vp,ap)=(v as usize,a as usize);
 let mut work=Producer{video:vp,audio:ap,join:Some(std::thread::spawn(move||{
  let mut output=interval();output.transfer.total_us=4_000_000;
  output.aus=(0..63).flat_map(|n|[1,2].map(move|es|HlsAu{data:vec![0;64],pts_ns:n*100_000_000,key:1,es})).collect();
  output.video_tail_ns=6_300_000_000;output.audio_tail_ns=Some(6_300_000_000);
  fixture_resident(&mut output,100_000_000);
  hls_feed_external_segment(&output,Duration::from_millis(6300),g,vp as *mut AuQueue,ap as *mut AuQueue)
 }))};
 let plays=ffi_host::play_calls_for_test();let mut volume=crate::player::volume::VolumeState::new();
 let until=Instant::now()+Duration::from_secs(2);
 while Instant::now()<until {
  engine::feed_both_lanes(&mt,&mut eng,&mut volume);
  if !eng.prime_play || (ffi_host::held_feed_full_for_test()>2
   && SHARED.hls_video_tail_ns.load(Ordering::Acquire)>=2_000_000_000
   && SHARED.hls_audio_tail_ns.load(Ordering::Acquire)>=2_000_000_000){break;}
  std::thread::sleep(Duration::from_millis(1));
 }
 let accepted=(eng.max_fed_video_pts,eng.max_fed_audio_pts);
 let tails=(SHARED.hls_video_tail_ns.load(Ordering::Acquire),SHARED.hls_audio_tail_ns.load(Ordering::Acquire));
 let full=ffi_host::held_feed_full_for_test();let played=ffi_host::play_calls_for_test()>plays;
 let retained=SHARED.has_retained_external_hls_interval_for_test();
 let producer_drained=work.join.as_ref().unwrap().is_finished();
 if producer_drained {let _=work.join.take().unwrap().join();}
 drop(work);drop(cap);clear(&mut eng);
 assert!(full>0,"real Feed must encounter paused native BufferFull");
 assert!(accepted.0>=700_000_000&&accepted.1>=300_000_000,"decoder prime accepted: {accepted:?}");
 assert!(tails.0>0&&tails.1>0&&tails.0<4_000_000_000&&tails.1<4_000_000_000,"real FIFO prefix remains below acquisition runway: {tails:?}");
 assert!(accepted.0<4_000_000_000&&accepted.1<4_000_000_000,"native capacity alone cannot cover acquisition");
 if !played {stalled.push(format!("{hold}: accepted {accepted:?}, queued {tails:?}, BufferFull={full}, retained={retained}"));}

 }
 assert!(stalled.is_empty(),"held clocks did not start: {stalled:?}");
}

#[test]
fn disjoint_empty_or_internally_gapped_resident_aus_cannot_manufacture_fresh_runway() {
 let _serial=plx_base::testlock::serial();
 for case in ["disjoint","empty-audio","internal-gap"] {
  SHARED.reset_session();TX.reset();ffi_host::force_clocksink_for_test(true);
  let native=SHARED.begin_native_session().unwrap();assert!(SHARED.mark_native_load_returned(native,None).is_some());
  let mut eng=engine::engine_after_reload_for_test();eng.native_epoch=native;
  let cap=ffi_host::held_feed_capacity_for_test(1_000_000_000);let mt=unsafe{MainThread::assume()};
  let g=SHARED.hls_candidate_generation.load(Ordering::Acquire);let(v,a)=pointers(&mut eng);
  let mut prefix=interval();prefix.transfer.total_us=4_000_000;
  prefix.aus=(0..=10).flat_map(|n|[1,2].map(move|es|HlsAu{data:vec![0;64],pts_ns:n*100_000_000,key:1,es})).collect();
  assert!(matches!(hls_feed_segment_interruptible(&prefix,v,a,true),Ok(true)));
  engine::feed_both_lanes(&mt,&mut eng,&mut crate::player::volume::VolumeState::new());
  // Replace drained queues with tiny actual FIFOs, then fill them with a real
  // prefix. The new producer cannot publish its first disjoint AU into them.
  for slot in [&mut eng.aq_video,&mut eng.aq_audio] {
   if let Some(mut q)=slot.take(){crate::aq::aq_destroy(&mut *q);}*slot=Some(crate::aq::aq_new(64));
  }
  let(v,a)=pointers(&mut eng);let mut queued=interval();queued.transfer.total_us=4_000_000;
  queued.aus=[(1,1_040_000_000),(2,1_040_000_000),(1,1_080_000_000),(2,1_080_000_000)].into_iter()
   .map(|(es,pts_ns)|HlsAu{data:vec![0;64],pts_ns,key:1,es}).collect();
  assert!(matches!(hls_feed_segment_interruptible(&queued,v,a,true),Ok(true)));
  // Pin one real refused AU in each native lane before filling the bounded
  // queues completely; prime must not create room for the disjoint producer.
  engine::feed_both_lanes(&mt,&mut eng,&mut crate::player::volume::VolumeState::new());
  queued.aus=[1,2].into_iter().map(|es|HlsAu{data:vec![0;64],pts_ns:1_120_000_000,key:1,es}).collect();
  assert!(matches!(hls_feed_segment_interruptible(&queued,v,a,true),Ok(true)));
  let(vp,ap)=(v as usize,a as usize);let empty=case=="empty-audio";let hole=case=="internal-gap";
  let work=Producer{video:vp,audio:ap,join:Some(std::thread::spawn(move||{
   let mut output=interval();output.transfer.total_us=4_000_000;
   output.aus=(0..63).flat_map(|n|[1,2].map(move|es|HlsAu{data:if empty&&es==2{vec![]}else{vec![0;64]},pts_ns:if empty||hole{n*100_000_000}else{1_600_000_000+n*100_000_000},key:1,es})).collect();
   if hole {output.aus.retain(|au|au.pts_ns<=1_200_000_000||au.pts_ns>=6_000_000_000);}
   output.video_tail_ns=600_000_000_000;output.audio_tail_ns=Some(600_000_000_000);
   fixture_resident(&mut output,100_000_000);
   hls_feed_external_segment(&output,Duration::from_secs(600),g,vp as *mut AuQueue,ap as *mut AuQueue)
  }))};
  wait_retained();let plays=ffi_host::play_calls_for_test();
  for _ in 0..5 {engine::feed_both_lanes(&mt,&mut eng,&mut crate::player::volume::VolumeState::new());}
  let played=ffi_host::play_calls_for_test()!=plays;drop(work);drop(cap);clear(&mut eng);
  assert!(!played,"{case}: only real overlapping bytes can extend both lanes");
 }
}
