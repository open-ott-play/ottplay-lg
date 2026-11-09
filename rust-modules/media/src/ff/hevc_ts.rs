//! Bounded single-layer HEVC Annex-B transport preparation. A fresh decoder
//! enters only at a proved BLA/IDR/CRA picture with a matching VPS/SPS/PPS chain.
//! Native capabilities and the already-declared Load codec remain caller-owned.
const AU_BYTES:usize=16*1024*1024;
const PARAM_BYTES:usize=64*1024;
const NAL_LIMIT:usize=512;
#[derive(Clone,Default)]
struct Params{vps:Vec<u8>,sps:Vec<u8>,pps:Vec<u8>}
#[derive(Clone)]
pub(super) struct State{params:Params,needs_entry:bool,discard_rasl:bool}
impl Default for State{fn default()->Self{Self{params:Params::default(),needs_entry:true,discard_rasl:false}}}
impl State{pub(super) fn needs_entry(&self)->bool{self.needs_entry}}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(super) enum Decision{Skip,Start,Continue}
#[derive(Clone,Copy,Debug)]
pub(super) struct AccessInfo{pub(super) vcl:bool,pub(super) key:bool,pub(super) params_changed:bool,pub(super) decision:Decision,pub(super) disposable:bool}
struct Bits<'a>{bytes:&'a[u8],at:usize}
impl<'a> Bits<'a>{
 fn new(bytes:&'a[u8])->Self{Self{bytes,at:0}}
 fn bit(&mut self)->Option<u32>{let byte=*self.bytes.get(self.at/8)?;let bit=u32::from((byte>>(7-self.at%8))&1);self.at+=1;Some(bit)}
 fn bits(&mut self,count:usize)->Option<u32>{if count>32{return None}let mut value=0;for _ in 0..count{value=(value<<1)|self.bit()?;}Some(value)}
 fn skip(&mut self,count:usize)->Option<()>{self.at=self.at.checked_add(count).filter(|end|*end<=self.bytes.len()*8)?;Some(())}
 fn ue(&mut self)->Option<u32>{let mut zero=0;while self.bit()?==0{zero+=1;if zero>31{return None}}Some(((1u32<<zero)-1)+self.bits(zero)?)}
}
fn raw(mut nal:&[u8])->&[u8]{if nal.starts_with(&[0,0,0,1]){nal=&nal[4..]}else if nal.starts_with(&[0,0,1]){nal=&nal[3..]}
 while nal.last()==Some(&0){nal=&nal[..nal.len()-1]}nal}
fn header(nal:&[u8])->Option<(u8,u8)>{if nal.len()<2||nal[0]&0x80!=0{return None}let layer=((nal[0]&1)<<5)|(nal[1]>>3);let tid=nal[1]&7;
 if layer!=0||tid==0{return None}Some(((nal[0]>>1)&63,tid))}
fn unescape(data:&[u8],limit:usize)->Option<Vec<u8>>{let mut result=Vec::with_capacity(data.len().min(limit));let mut zeros=0;let mut at=0;
 while at<data.len()&&result.len()<limit{let byte=data[at];if zeros>=2&&byte==3{if data.get(at+1).is_none_or(|next|*next>3){return None}zeros=0;at+=1;continue}
 if zeros>=2&&byte<3{return None}result.push(byte);zeros=if byte==0{zeros+1}else{0};at+=1;}Some(result)}
fn parameter(nal:&[u8],kind:u8)->Option<Vec<u8>>{let nal=raw(nal);if nal.len()>PARAM_BYTES||header(nal)?!=(kind,1){return None}unescape(&nal[2..],PARAM_BYTES)}
fn parameter_chain(params:&Params)->Option<u32>{
 let v=parameter(&params.vps,32)?;let s=parameter(&params.sps,33)?;let p=parameter(&params.pps,34)?;
 let vid=Bits::new(&v).bits(4)?;let mut s=Bits::new(&s);if s.bits(4)?!=vid{return None}let layers=s.bits(3)? as usize;if layers>6{return None}s.bit()?;
 // profile_tier_level(): general profile/constraints88bits plus level8bits.
 s.skip(96)?;let mut profile=[false;6];let mut level=[false;6];
 for i in 0..layers{profile[i]=s.bit()?!=0;level[i]=s.bit()?!=0;}
 if layers>0{for _ in layers..8{if s.bits(2)?!=0{return None}}}
 for i in 0..layers{if profile[i]{s.skip(88)?;}if level[i]{s.skip(8)?;}}
 let sid=s.ue()?;if sid>15{return None}let mut p=Bits::new(&p);let pid=p.ue()?;let referenced=p.ue()?;
 if pid>63||referenced!=sid{return None}Some(pid)
}
fn slice(nal:&[u8],kind:u8)->Option<(bool,u32)>{let bytes=unescape(&nal[2..],32)?;let mut bits=Bits::new(&bytes);let first=bits.bit()?!=0;
 if (16..=21).contains(&kind){bits.bit()?;}let pid=bits.ue()?;if pid>63{return None}Some((first,pid))}
fn start(data:&[u8],from:usize)->Option<(usize,usize)>{let mut at=from;while at+3<=data.len(){if data[at..].starts_with(&[0,0,1]){return Some((at,3))}
 if data[at..].starts_with(&[0,0,0,1]){return Some((at,4))}at+=1;}None}
fn store(target:&mut Vec<u8>,nal:&[u8])->bool{let changed=target.get(4..)!=Some(nal);target.clear();target.extend_from_slice(&[0,0,0,1]);target.extend_from_slice(nal);changed}
// Only picture_timing is admitted for omission with its own non-reference
// picture. Persistent HDR, active-parameter, caption and unknown SEI remain
// decoder input and must never disappear through the timestamp fallback.
fn picture_local_sei(nal: &[u8]) -> bool {
    if nal.len() > PARAM_BYTES || header(nal).map(|value| value.0) != Some(39) { return false; }
    let Some(bytes) = unescape(&nal[2..], PARAM_BYTES) else { return false; };
    let mut at = 0usize;
    let mut messages = 0;
    while at < bytes.len() {
        if at + 1 == bytes.len() && bytes[at] == 0x80 { return messages > 0; }
        let mut extended = || -> Option<usize> {
            let mut value = 0usize;
            loop {
                let byte = usize::from(*bytes.get(at)?); at += 1;
                value = value.checked_add(byte).filter(|value| *value <= PARAM_BYTES)?;
                if byte != 255 { return Some(value); }
            }
        };
        let Some(kind) = extended() else { return false; };
        let Some(size) = extended() else { return false; };
        if kind != 1 || size == 0 { return false; }
        let Some(end) = at.checked_add(size).filter(|end| *end < bytes.len()) else { return false; };
        at = end; messages += 1;
    }
    false
}
/// The supplied State must be a private segment candidate. Commit it only after
/// the complete segment was validated and successfully fed to its exact owner.
pub(super) fn annexb(data:&[u8],state:&mut State,out:&mut Vec<u8>)->Result<AccessInfo,&'static str>{
 out.clear();if data.len()>AU_BYTES{return Err("HEVC access unit exceeds limit")}
 if start(data,0).map(|(at,_)|at)!=Some(0){return Err("HEVC packet is not Annex-B")}
 let mut candidate=state.clone();let mut cursor=0;let mut count=0;let mut changed=false;let mut present=[false;3];
 let mut picture:Option<(u8,u8,u32)>=None;let mut primary=false;let mut local_metadata=true;
 while let Some((at,prefix))=start(data,cursor){count+=1;if count>NAL_LIMIT{return Err("HEVC NAL count exceeds limit")}
  let begin=at+prefix;let end=start(data,begin).map_or(data.len(),|(at,_)|at);let nal=raw(&data[begin..end]);
  let (kind,tid)=header(nal).ok_or("Invalid or multilayer HEVC NAL header")?;
  match kind{
   32..=34=>{if picture.is_some(){return Err("HEVC parameter set follows picture")}
    parameter(nal,kind).ok_or("Invalid HEVC parameter set")?;let index=usize::from(kind-32);present[index]=true;
    changed|=match kind{32=>store(&mut candidate.params.vps,nal),33=>store(&mut candidate.params.sps,nal),_=>store(&mut candidate.params.pps,nal)};
   },
   0..=9|16..=21=>{if (16..=21).contains(&kind)&&tid!=1{return Err("HEVC IRAP has nonzero temporal layer")}
    let (first,pid)=slice(nal,kind).ok_or("Invalid HEVC slice header")?;
    if let Some((old_kind,old_tid,old_pid))=picture{if first||old_kind!=kind||old_tid!=tid||old_pid!=pid{return Err("Multiple HEVC pictures or inconsistent slice headers")}}
    else{primary=first;picture=Some((kind,tid,pid));}
   },
   35=>{if picture.is_some(){return Err("HEVC access-unit delimiter follows picture")}},
   36|37=>return Err("HEVC end marker needs a new decoder epoch"),
   38=>{},
   39|40=>{local_metadata&=picture_local_sei(nal);},
   _=>return Err("Unsupported HEVC NAL unit type"),
  }
  cursor=end;if cursor>=data.len(){break}
 }
 let kind=picture.map(|picture|picture.0);let key=kind.is_some_and(|kind|(16..=21).contains(&kind));
 if changed{candidate.needs_entry=true;candidate.discard_rasl=false;}
 let dropping_rasl=candidate.discard_rasl&&!candidate.needs_entry&&matches!(kind,Some(8|9));
 if picture.is_some()&&!primary&&!candidate.needs_entry&&!dropping_rasl{return Err("HEVC picture lacks a primary first slice")}
 if let Some((_,_,pid))=picture{
  if !dropping_rasl&&(key||!candidate.needs_entry){let expected=parameter_chain(&candidate.params).ok_or("HEVC requires matching VPS SPS PPS")?;
   if pid!=expected{return Err("HEVC slice does not reference retained parameter sets")}}
 }
 let decision=if candidate.needs_entry{
  if key&&primary{candidate.needs_entry=false;candidate.discard_rasl=matches!(kind,Some(16..=18|21));Decision::Start}else{Decision::Skip}
 }else if candidate.discard_rasl&&matches!(kind,Some(8|9)){Decision::Skip}
 else{if key{candidate.discard_rasl=matches!(kind,Some(16..=18));}
  else if matches!(kind,Some(0..=5)){candidate.discard_rasl=false;}Decision::Continue};
 if decision!=Decision::Skip{
  if key{for (i,param) in [&candidate.params.vps,&candidate.params.sps,&candidate.params.pps].into_iter().enumerate(){if !present[i]{out.extend_from_slice(param);}}}
  out.extend_from_slice(data);
 }
 let disposable=kind==Some(0)&&!changed&&local_metadata&&decision==Decision::Continue;
 *state=candidate;Ok(AccessInfo{vcl:picture.is_some(),key,params_changed:changed,decision,disposable})
}

#[cfg(test)]
#[path="hevc_ts_tests.rs"]
mod tests;
