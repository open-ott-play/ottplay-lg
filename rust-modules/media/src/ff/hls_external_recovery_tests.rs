//! Exercise the actual external interval feeder, bounded FIFOs and native Play.
use super::*;
use crate::player::{engine,ffi_host,TX};
use plx_base::task::MainThread;
use plx_platform::tv::sink::VideoSink;
use std::time::{Duration,Instant};
fn interval()->HlsSegmentOutput {
 HlsSegmentOutput {transport_timing:None,webvtt_clock:None,observed:None,
  aus:[(1,1_600_000_000),(2,1_600_000_000),(1,1_640_000_000),(2,1_640_000_000)].into_iter()
   .map(|(es,pts_ns)|HlsAu{data:vec![0;64],pts_ns,key:1,es}).collect(),
  transfer:SegmentTransfer{bytes:256,active_us:100_000,total_us:100_000,audio_expected:true},
  video_width:160,video_height:90,video_tail_ns:1_680_000_000,audio_tail_ns:Some(1_680_000_000)}
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
