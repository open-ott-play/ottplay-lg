//! Conservative AVC zero-delay recovery-point evidence. The caller supplies
//! FFmpeg-probed parameter sets; this helper additionally proves their ID chain
//! and the recovery message's association with the primary intra picture.
const NAL_BYTES:usize=64*1024;
const AU_BYTES:usize=16*1024*1024;
const NAL_LIMIT:usize=256;

struct Bits<'a>{data:&'a[u8],at:usize}
impl<'a> Bits<'a>{
    fn new(data:&'a[u8])->Self{Self{data,at:0}}
    fn bit(&mut self)->Option<u32>{let b=*self.data.get(self.at/8)?;let v=(b>>(7-self.at%8))&1;self.at+=1;Some(u32::from(v))}
    fn bits(&mut self,n:usize)->Option<u32>{if n>32{return None}let mut v=0;for _ in 0..n{v=(v<<1)|self.bit()?;}Some(v)}
    fn ue(&mut self)->Option<u32>{let mut zeros=0;while self.bit()?==0{zeros+=1;if zeros>31{return None}}let tail=self.bits(zeros)?;Some(((1u32<<zeros)-1)+tail)}
    fn aligned_end(&mut self)->bool{if self.bit()!=Some(1){return false}while self.at<self.data.len()*8{if self.bit()!=Some(0){return false}}true}
}
fn raw_nal(mut nal:&[u8])->&[u8]{
    if nal.starts_with(&[0,0,0,1]){nal=&nal[4..]}else if nal.starts_with(&[0,0,1]){nal=&nal[3..]}
    while nal.last()==Some(&0){nal=&nal[..nal.len()-1]}nal
}
fn rbsp(nal:&[u8],kind:u8)->Option<Vec<u8>>{
    let nal=raw_nal(nal);
    if nal.len()<2||nal.len()>NAL_BYTES||nal[0]&0x80!=0||nal[0]&31!=kind{return None}
    if kind==6&&nal[0]&0x60!=0{return None}
    let data=&nal[1..];let mut out=Vec::with_capacity(data.len());let mut i=0;let mut zeros=0;
    while i<data.len(){let b=data[i];if zeros>=2&&b==3{
        if data.get(i+1).is_none_or(|next|*next>3){return None}zeros=0;i+=1;continue;
    }if zeros>=2&&b<3{return None}out.push(b);zeros=if b==0{zeros+1}else{0};i+=1;}Some(out)
}
// None is malformed; Some(None) is a valid SEI without recovery metadata.
fn recovery_sei(nal:&[u8])->Option<Option<bool>>{
    let bytes=rbsp(nal,6)?;let mut cursor=0;let mut recovery=None;
    while cursor<bytes.len(){
        if cursor+1==bytes.len()&&bytes[cursor]==0x80{return Some(recovery)}
        let mut value=||->Option<usize>{let mut value=0usize;loop{let b=usize::from(*bytes.get(cursor)?);cursor+=1;value=value.checked_add(b)?;if value>NAL_BYTES{return None}if b!=255{return Some(value)}}};
        let kind=value()?;let len=value()?;
        let end=cursor.checked_add(len).filter(|end|*end<bytes.len())?;
        let payload=&bytes[cursor..end];cursor=end;
        if kind==6{
            if recovery.is_some(){return None}
            let mut bits=Bits::new(payload);
            let count=bits.ue()?;let exact=bits.bit()?;let broken=bits.bit()?;let group=bits.bits(2)?;
            // Restrict payload extensions as well: zero-delay syntax and its
            // mandatory bit-alignment marker fit in exactly one byte.
            if payload.len()!=1||!bits.aligned_end(){return None}
            recovery=Some(count==0&&exact==1&&broken==0&&group==0);
        }
    }None
}
pub(super) fn zero_recovery_point(nal:&[u8])->bool{recovery_sei(nal)==Some(Some(true))}
fn parameter_ids(sps:&[u8],pps:&[u8])->Option<(u32,u32)>{
    let sn=raw_nal(sps);let pn=raw_nal(pps);
    if sn.first().is_none_or(|h|h&0x60==0)||pn.first().is_none_or(|h|h&0x60==0){return None}
    let sps=rbsp(sn,7)?;let pps=rbsp(pn,8)?;
    let mut s=Bits::new(&sps);s.bits(8)?;let constraints=s.bits(8)?;if constraints&3!=0{return None}s.bits(8)?;
    let sid=s.ue()?;if sid>31{return None}
    let mut p=Bits::new(&pps);let pid=p.ue()?;let referenced=p.ue()?;
    if pid>255||referenced!=sid{return None}Some((sid,pid))
}
fn intra_slice(nal:&[u8],pps_id:u32)->Option<u32>{
    let nal=raw_nal(nal);
    if nal.first().is_none_or(|h|h&0x80!=0||h&31!=1||h&0x60==0){return None}
    // Only three UE fields are required. Entropy-coded slice payload may be
    // megabytes: extract a bounded RBSP header prefix, never copy/reject it for
    // exceeding the much smaller SEI/parameter-set bound.
    let data=&nal[1..];let mut slice=Vec::with_capacity(32);let mut i=0;let mut zeros=0;
    while i<data.len()&&slice.len()<32{
        let byte=data[i];
        if zeros>=2&&byte==3{if data.get(i+1).is_none_or(|next|*next>3){return None}zeros=0;i+=1;continue}
        if zeros>=2&&byte<3{return None}
        slice.push(byte);zeros=if byte==0{zeros+1}else{0};i+=1;
    }
    let mut bits=Bits::new(&slice);let first=bits.ue()?;let kind=bits.ue()?;let pid=bits.ue()?;
    if !matches!(kind,2|7)||pid!=pps_id{return None}Some(first)
}

fn start(data:&[u8],from:usize)->Option<(usize,usize)>{
    let mut i=from;while i+3<=data.len(){if data[i..].starts_with(&[0,0,1]){return Some((i,3))}
    if data[i..].starts_with(&[0,0,0,1]){return Some((i,4))}i+=1;}None
}
pub(super) fn recovery_entry(annexb:&[u8],sps:&[u8],pps:&[u8])->bool{
    if annexb.len()>AU_BYTES{return false}let Some((_,pid))=parameter_ids(sps,pps)else{return false};
    if start(annexb,0).map(|(at,_)|at)!=Some(0){return false}
    let mut cursor=0;let mut count=0;let mut found=false;let mut recovery_count=0;let mut vcl=false;let mut previous_mb=None;
    while let Some((at,prefix))=start(annexb,cursor){
        count+=1;if count>NAL_LIMIT{return false}let body=at+prefix;
        let end=start(annexb,body).map_or(annexb.len(),|(at,_)|at);
        let nal=raw_nal(&annexb[body..end]);let Some(&header)=nal.first()else{return false};
        if header&0x80!=0{return false}
        match header&31{
            6=>{let Some(message)=recovery_sei(nal)else{return false};if let Some(zero)=message{
                if vcl{return false}recovery_count+=1;if recovery_count>1{return false}found=zero;
            }},
            7|8=>{if vcl{return false}},
            1=>{if !found{return false}let Some(first)=intra_slice(nal,pid)else{return false};
                if !vcl&&first!=0{return false}if previous_mb.is_some_and(|last|first<=last){return false}previous_mb=Some(first);vcl=true;},
            2..=5=>return false,
            9..=11=>{if found||vcl{return false}},
            12=>{},
            _=>return false,
        }
        cursor=end;if cursor>=annexb.len(){break}
    }found&&vcl
}

#[cfg(test)]
#[path="h264_recovery_tests.rs"]
mod tests;
