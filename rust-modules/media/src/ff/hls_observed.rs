//! Bounded facts from one fully fed MPEG-TS segment. A PAT/PMT table plus the
//! checked AVStream.id accessor establishes transport identity; labels and array
//! positions never do. This does not establish a presentation-wide inventory.
use super::inspect::{InspectedTrack, TrackInventory, TrackKind};
use std::collections::{BTreeMap, BTreeSet};

const BODY_BYTES: usize = 64 * 1024 * 1024;
const SECTION_BYTES: usize = 1024;
const PROGRAM_LIMIT: usize = 32;
const TRACK_LIMIT: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TransportIdentity {
    pub transport_stream_id: u16,
    pub program_id: u16,
    pub pid: u16,
    pub stream_type: u8,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Elementary { identity: TransportIdentity, descriptors: Vec<u8> }
#[derive(Clone, Debug, PartialEq, Eq)]
struct Pat { transport_stream_id: u16, version: u8, programs: BTreeMap<u16,u16> }
#[derive(Clone, Debug, PartialEq, Eq)]
struct Pmt { version: u8, streams: Vec<Elementary> }

#[derive(Clone)]
pub struct ProgramTable { pat: Pat, pmts: BTreeMap<u16,Pmt> }
impl std::fmt::Debug for ProgramTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgramTable").field("programs",&self.pmts.len()).finish_non_exhaustive()
    }
}
impl ProgramTable {
    pub fn identities(&self) -> impl Iterator<Item=TransportIdentity> + '_ {
        self.pmts.values().flat_map(|p| p.streams.iter().map(|s| s.identity))
    }
    pub fn identity_for_pid(&self, pid: u16, codec: &str) -> Result<TransportIdentity,&'static str> {
        let mut found = self.pmts.values().flat_map(|p| &p.streams).filter(|s| s.identity.pid == pid);
        let stream = found.next().ok_or("HLS PID is absent from the validated program map")?;
        if found.next().is_some() { return Err("HLS PID belongs to multiple programs"); }
        if !codec_matches(stream,codec) { return Err("HLS codec disagrees with the program map"); }
        Ok(stream.identity)
    }
    /// Infer only the validated PMT video type for this exact unambiguous PID.
    /// The demux caller independently verifies the actual AVCodec and frozen Load.
    pub fn video_identity_for_pid(&self, pid: u16) -> Result<TransportIdentity, &'static str> {
        let mut found = self.identities().filter(|identity| identity.pid == pid);
        let identity = found.next().ok_or("HLS video PID is absent from the validated program map")?;
        if found.next().is_some() { return Err("HLS video PID belongs to multiple programs"); }
        match identity.stream_type {
            0x1b => self.identity_for_pid(pid, "h264"),
            0x24 => self.identity_for_pid(pid, "hevc"),
            _ => Err("Unsupported HLS video program map type"),
        }
    }
    pub fn bind_inventory_for_video(&self, inventory: &TrackInventory, pids: &[(i32,u16)], video_pid: u16)
        -> Result<TrackInventory,&'static str>
    {
        let video = self.video_identity_for_pid(video_pid)?;
        let mut indexes=BTreeMap::new();
        for &(index,pid) in pids {
            if index<0 || index>=TRACK_LIMIT as i32 || !valid_pid(pid) || indexes.insert(index,pid).is_some() {
                return Err("Invalid HLS stream PID binding");
            }
        }
        let mut audio=Vec::new(); let mut subtitles=Vec::new(); let mut bindings=Vec::new();
        for row in inventory.audio.iter().chain(&inventory.subtitles) {
            let pid=*indexes.get(&row.stream_index).ok_or("HLS track has no checked transport PID")?;
            let streams:Vec<_>=self.pmts.values().flat_map(|p| &p.streams).filter(|s| s.identity.pid==pid).collect();
            if streams.len()!=1 { return Err("HLS PID is absent or ambiguous in program map"); }
            let identity=streams[0].identity;
            if identity.program_id!=video.program_id || identity.transport_stream_id!=video.transport_stream_id { continue; }
            if !codec_matches(streams[0],&row.codec) { return Err("HLS codec disagrees with the selected video program"); }
            let mut track=row.clone();
            let target=if row.kind==TrackKind::Audio { &mut audio } else { &mut subtitles };
            track.ordinal=target.len() as i32; target.push(track);
            bindings.push((row.stream_index,identity));
        }
        let mut result=TrackInventory::from_partial(audio,subtitles);
        for (index,identity) in bindings { result.bind_transport_identity(index,identity)?; }
        validate_inventory(&result)?; Ok(result)
    }
    /// Only the checked AVStream.id accessor may supply these stream-index/PID
    /// pairs. PMT ordering is deliberately unrelated to AVStream array ordering.
    pub fn bind_inventory(&self, inventory: &TrackInventory, pids: &[(i32,u16)]) -> Result<TrackInventory,&'static str> {
        let mut by_index = BTreeMap::new();
        for &(index,pid) in pids {
            if index < 0 || index >= TRACK_LIMIT as i32 || !valid_pid(pid) || by_index.insert(index,pid).is_some() {
                return Err("Invalid HLS stream PID binding");
            }
        }
        let mut result = TrackInventory::from_partial(inventory.audio.clone(),inventory.subtitles.clone());
        for track in inventory.audio.iter().chain(&inventory.subtitles) {
            let pid = by_index.get(&track.stream_index).ok_or("HLS track has no checked transport PID")?;
            let identity = self.identity_for_pid(*pid,&track.codec)?;
            result.bind_transport_identity(track.stream_index,identity)?;
        }
        validate_inventory(&result)?;
        Ok(result)
    }
}
fn descriptor_has(bytes: &[u8], tag: u8) -> bool {
    let mut rest = bytes;
    while rest.len() >= 2 {
        let len = usize::from(rest[1]);
        if rest.len() < len+2 { return false; }
        if rest[0] == tag { return true; }
        rest = &rest[len+2..];
    }
    false
}
fn codec_matches(stream: &Elementary, codec: &str) -> bool {
    match stream.identity.stream_type {
        0x1b => codec == "h264",
        0x24 => codec == "hevc",
        0x02 => codec == "mpeg2video",
        0x03 | 0x04 => matches!(codec,"mp2"|"mp3"),
        0x0f => codec == "aac",
        0x11 => matches!(codec,"aac"|"aac_latm"),
        0x81 => codec == "ac3",
        0x87 => codec == "eac3",
        0x90 => codec == "hdmv_pgs_subtitle",
        0x06 => match codec {
            "dvb_subtitle" => descriptor_has(&stream.descriptors,0x59),
            "dvb_teletext" => descriptor_has(&stream.descriptors,0x56),
            "ac3" => descriptor_has(&stream.descriptors,0x6a),
            "eac3" => descriptor_has(&stream.descriptors,0x7a),
            _ => false,
        },
        _ => false,
    }
}
fn valid_pid(pid: u16) -> bool { (0x10..0x1fff).contains(&pid) }
fn crc(bytes: &[u8]) -> u32 {
    let mut value = u32::MAX;
    for byte in bytes {
        value ^= u32::from(*byte)<<24;
        for _ in 0..8 { value = if value & 0x8000_0000 != 0 { (value<<1)^0x04c1_1db7 } else { value<<1 }; }
    }
    value
}
fn section_header(section: &[u8], table_id: u8) -> Result<(), &'static str> {
    if section.len() < 12 || section[0] != table_id || section[1]&0xf0 != 0xb0
        || usize::from(u16::from_be_bytes([section[1]&15,section[2]]))+3 != section.len()
        || section[5]&1 == 0 || section[6] != 0 || section[7] != 0 || crc(section) != 0
    { return Err("Invalid or unsupported HLS program section"); }
    Ok(())
}
fn parse_pat(section: &[u8]) -> Result<Pat,&'static str> {
    section_header(section,0)?;
    let entries = &section[8..section.len()-4];
    if entries.len()%4 != 0 { return Err("Invalid HLS PAT entries"); }
    let mut programs = BTreeMap::new();
    let mut pids = BTreeSet::new();
    for row in entries.chunks_exact(4) {
        let program = u16::from_be_bytes([row[0],row[1]]);
        if row[2]&0xe0 != 0xe0 { return Err("Invalid HLS PAT PID"); }
        let pid = u16::from_be_bytes([row[2]&31,row[3]]);
        if program == 0 { continue; }
        if !valid_pid(pid) || programs.len() >= PROGRAM_LIMIT || programs.insert(program,pid).is_some() || !pids.insert(pid) {
            return Err("Invalid or excessive HLS program table");
        }
    }
    if programs.is_empty() { return Err("HLS PAT has no program"); }
    Ok(Pat { transport_stream_id: u16::from_be_bytes([section[3],section[4]]), version: (section[5]>>1)&31, programs })
}
fn descriptors(bytes: &[u8]) -> Result<(), &'static str> {
    let mut rest = bytes;
    while !rest.is_empty() {
        if rest.len()<2 || usize::from(rest[1])+2>rest.len() { return Err("Invalid HLS PMT descriptor"); }
        rest = &rest[usize::from(rest[1])+2..];
    }
    Ok(())
}
fn parse_pmt(section: &[u8], pat: &Pat, pid: u16) -> Result<(u16,Pmt),&'static str> {
    section_header(section,2)?;
    if section.len()<16 { return Err("Invalid HLS PMT"); }
    let program = u16::from_be_bytes([section[3],section[4]]);
    if pat.programs.get(&program) != Some(&pid) || section[8]&0xe0 != 0xe0 || section[10]&0xf0 != 0xf0 {
        return Err("HLS PMT does not belong to PAT program");
    }
    let info = usize::from(u16::from_be_bytes([section[10]&15,section[11]]));
    let end = section.len()-4;
    let start = 12usize.checked_add(info).filter(|start| *start<=end).ok_or("Invalid HLS PMT program descriptors")?;
    descriptors(&section[12..start])?;
    let mut cursor = start;
    let mut streams = Vec::new();
    let mut pids = BTreeSet::new();
    while cursor<end {
        if end-cursor<5 || section[cursor+1]&0xe0 != 0xe0 || section[cursor+3]&0xf0 != 0xf0 {
            return Err("Invalid HLS PMT elementary stream");
        }
        let stream_pid = u16::from_be_bytes([section[cursor+1]&31,section[cursor+2]]);
        let len = usize::from(u16::from_be_bytes([section[cursor+3]&15,section[cursor+4]]));
        let next = cursor.checked_add(5+len).filter(|next| *next<=end).ok_or("Invalid HLS PMT elementary descriptors")?;
        if !valid_pid(stream_pid) || streams.len() >= TRACK_LIMIT || !pids.insert(stream_pid)
            || pat.programs.values().any(|pid| *pid==stream_pid)
        { return Err("Invalid or duplicate HLS elementary PID"); }
        descriptors(&section[cursor+5..next])?;
        streams.push(Elementary { identity: TransportIdentity { transport_stream_id: pat.transport_stream_id,
            program_id: program, pid: stream_pid, stream_type: section[cursor] }, descriptors: section[cursor+5..next].to_vec() });
        cursor = next;
    }
    if streams.is_empty() { return Err("HLS PMT has no elementary streams"); }
    Ok((program,Pmt { version: (section[5]>>1)&31,streams }))
}

#[derive(Default)]
struct Section {
    pending: Vec<u8>,
    cc: Option<u8>,
    last_payload: Vec<u8>,
}
impl Section {
    fn bytes(&mut self, mut bytes: &[u8], start: bool) -> Result<Vec<Vec<u8>>,&'static str> {
        let mut complete = Vec::new();
        if self.pending.is_empty() && !start { return Ok(complete); }
        while !bytes.is_empty() {
            if self.pending.is_empty() && bytes[0] == 0xff { break; }
            let goal = if self.pending.len()<3 { 3 } else {
                3+usize::from(u16::from_be_bytes([self.pending[1]&15,self.pending[2]]))
            };
            if goal<3 || goal>SECTION_BYTES { return Err("HLS PSI section exceeds limit"); }
            let take = (goal-self.pending.len()).min(bytes.len());
            self.pending.extend_from_slice(&bytes[..take]); bytes = &bytes[take..];
            if self.pending.len()>=3 {
                let total = 3+usize::from(u16::from_be_bytes([self.pending[1]&15,self.pending[2]]));
                if !(12..=SECTION_BYTES).contains(&total) { return Err("Invalid HLS PSI section length"); }
                if self.pending.len()==total { complete.push(std::mem::take(&mut self.pending)); }
            }
        }
        Ok(complete)
    }
    fn packet(&mut self, payload: &[u8], cc: u8, start: bool, discontinuity: bool) -> Result<Vec<Vec<u8>>,&'static str> {
        if discontinuity {
            if !self.pending.is_empty() { return Err("HLS PSI interrupted by transport discontinuity"); }
            self.cc = None;
        }
        if let Some(previous) = self.cc {
            if cc == previous {
                if self.last_payload == payload { return Ok(Vec::new()); }
                return Err("Conflicting duplicate HLS PSI packet");
            }
            if cc != (previous+1)&15 { return Err("HLS PSI continuity counter changed"); }
        }
        self.cc = Some(cc); self.last_payload = payload.to_vec();
        if !start { return self.bytes(payload,false); }
        let pointer = usize::from(*payload.first().ok_or("Empty HLS PSI payload")?);
        if pointer+1>payload.len() { return Err("Invalid HLS PSI pointer"); }
        let mut result = Vec::new();
        if !self.pending.is_empty() {
            result.extend(self.bytes(&payload[1..pointer+1],false)?);
            if !self.pending.is_empty() { return Err("Incomplete HLS PSI before new section"); }
        }
        result.extend(self.bytes(&payload[pointer+1..],true)?);
        Ok(result)
    }
}

/// Sequential TS188 bytes from one complete external mux segment. Any seek,
/// truncation, scrambled packet, conflicting table or bound violation poisons it.
#[derive(Default)]
pub struct TsProbe {
    bytes: usize,
    pending: Vec<u8>,
    sections: BTreeMap<u16,Section>,
    pat: Option<Pat>,
    pmts: BTreeMap<u16,Pmt>,
    failed: bool,
}
impl TsProbe {
    pub fn new() -> Self { Self::default() }
    pub fn feed(&mut self, bytes: &[u8]) -> Result<(), &'static str> { self.feed_at(self.bytes as i64,bytes) }
    pub fn feed_at(&mut self, offset: i64, bytes: &[u8]) -> Result<(), &'static str> {
        if self.failed { return Err("HLS program probe was invalidated"); }
        let result = self.feed_inner(offset,bytes);
        if result.is_err() { self.failed=true; }
        result
    }
    fn feed_inner(&mut self, offset: i64, bytes: &[u8]) -> Result<(), &'static str> {
        if usize::try_from(offset).ok()!=Some(self.bytes) { return Err("HLS program probe requires sequential transport bytes"); }
        self.bytes = self.bytes.checked_add(bytes.len()).filter(|n| *n<=BODY_BYTES).ok_or("HLS program probe exceeds byte limit")?;
        let mut input = bytes;
        if !self.pending.is_empty() {
            let take = (188-self.pending.len()).min(input.len());
            self.pending.extend_from_slice(&input[..take]); input=&input[take..];
            if self.pending.len()==188 {
                let packet = std::mem::take(&mut self.pending); self.packet(&packet)?;
            }
        }
        while input.len()>=188 { self.packet(&input[..188])?; input=&input[188..]; }
        self.pending.extend_from_slice(input);
        Ok(())
    }
    fn packet(&mut self, packet: &[u8]) -> Result<(), &'static str> {
        if packet[0]!=0x47 || packet[1]&0x80!=0 || packet[3]&0xc0!=0 { return Err("Invalid or scrambled HLS TS packet"); }
        let pid = u16::from_be_bytes([packet[1]&31,packet[2]]);
        let control = (packet[3]>>4)&3;
        if control==0 { return Err("Invalid HLS TS adaptation control"); }
        let mut offset = 4;
        let mut discontinuity = false;
        if control&2!=0 {
            let len = usize::from(packet[4]);
            offset=5+len;
            if offset>188 { return Err("Invalid HLS TS adaptation length"); }
            if len>0 { discontinuity=packet[5]&0x80!=0; }
        }
        if control&1==0 || offset==188 { return Ok(()); }
        if pid!=0 && !self.pat.as_ref().is_some_and(|pat| pat.programs.values().any(|p| *p==pid)) { return Ok(()); }
        let sections = self.sections.entry(pid).or_default().packet(&packet[offset..],packet[3]&15,packet[1]&0x40!=0,discontinuity)?;
        for section in sections {
            if pid==0 {
                let candidate = parse_pat(&section)?;
                if self.pat.as_ref().is_some_and(|previous| *previous!=candidate) { return Err("HLS PAT changed within segment"); }
                self.pat=Some(candidate);
            } else {
                let (program,candidate)=parse_pmt(&section,self.pat.as_ref().ok_or("Missing HLS PAT")?,pid)?;
                if self.pmts.get(&program).is_some_and(|old| *old!=candidate) { return Err("HLS PMT changed within segment"); }
                let retained: usize = self.pmts.values().map(|p| p.streams.len()).sum();
                if !self.pmts.contains_key(&program) && retained+candidate.streams.len()>TRACK_LIMIT { return Err("HLS elementary stream table exceeds limit"); }
                self.pmts.insert(program,candidate);
            }
        }
        Ok(())
    }
    /// Header peek for choosing a PID while the segment remains privately
    /// buffered. The caller must still finish the entire probe before feeding.
    pub fn table(&self) -> Result<ProgramTable,&'static str> {
        if self.failed { return Err("HLS program probe was invalidated"); }
        let pat=self.pat.as_ref().ok_or("HLS segment has no validated PAT")?;
        if pat.programs.len()!=self.pmts.len() { return Err("HLS segment is missing a validated PMT"); }
        Ok(ProgramTable { pat:pat.clone(),pmts:self.pmts.clone() })
    }
    pub fn finish(self) -> Result<ProgramTable,&'static str> {
        if self.failed || !self.pending.is_empty() || self.sections.values().any(|s| !s.pending.is_empty()) {
            return Err("Incomplete or invalidated HLS program probe");
        }
        let pat = self.pat.ok_or("HLS segment has no validated PAT")?;
        if pat.programs.len()!=self.pmts.len() { return Err("HLS segment is missing a validated PMT"); }
        Ok(ProgramTable { pat,pmts:self.pmts })
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Scope { source: String, variant: String, worker: u64, discontinuity: u64, playback:Option<crate::route::PlaybackIdentity> }
impl Scope {
    pub fn new(source: &str, variant: &str, worker: u64, discontinuity: u64) -> Result<Self,&'static str> {
        if source.len()>64*1024 || variant.len()>64*1024 { return Err("HLS observation source exceeds size limit"); }
        Ok(Self { source: plx_data::ott::http_url(source).ok_or("Invalid HLS observation source")?,
            variant: plx_data::ott::http_url(variant).ok_or("Invalid HLS observation variant")?, worker,discontinuity,playback:None })
    }
    pub fn with_playback(mut self, playback:crate::route::PlaybackIdentity)->Self { self.playback=Some(playback); self }
    pub fn playback(&self)->Option<&crate::route::PlaybackIdentity> { self.playback.as_ref() }
    pub fn worker(&self) -> u64 { self.worker }
    pub fn discontinuity(&self) -> u64 { self.discontinuity }
    pub fn matches_source(&self, source: &str) -> bool { plx_data::ott::http_url(source).is_some_and(|source| source==self.source) }
    pub fn same_session(&self, other: &Self) -> bool { self.source==other.source && self.variant==other.variant && self.worker==other.worker && self.playback==other.playback }
}
impl std::fmt::Debug for Scope {
    fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result {
        f.debug_struct("HlsObservedScope").field("worker",&self.worker).field("discontinuity",&self.discontinuity).finish_non_exhaustive()
    }
}
fn rows(inventory: &TrackInventory, kind: TrackKind) -> &[InspectedTrack] {
    if kind==TrackKind::Audio { &inventory.audio } else { &inventory.subtitles }
}
fn validate_inventory(inventory: &TrackInventory) -> Result<(), &'static str> {
    if inventory.audio.len()+inventory.subtitles.len()>TRACK_LIMIT { return Err("Observed HLS track limit exceeded"); }
    let mut indexes=BTreeSet::new(); let mut identities=BTreeSet::new();
    for kind in [TrackKind::Audio,TrackKind::Subtitle] {
        for (ordinal,row) in rows(inventory,kind).iter().enumerate() {
            let identity=inventory.transport_identity(row.stream_index).ok_or("Observed HLS track has no transport identity")?;
            if row.kind!=kind || row.ordinal!=ordinal as i32 || !(0..TRACK_LIMIT as i32).contains(&row.stream_index)
                || !indexes.insert(row.stream_index) || !identities.insert(identity) || !valid_pid(identity.pid)
                || identity.program_id==0 || row.codec.is_empty() || row.codec.len()>64 || row.language.len()>128 || row.title.len()>1024
            { return Err("Invalid observed HLS track inventory"); }
        }
    }
    Ok(())
}
#[derive(Clone)]
pub struct Snapshot { scope: Scope, sequence: u64, revision: u64, inventory: TrackInventory, video:Option<TransportIdentity>, selected_audio:Option<TransportIdentity> }
impl Snapshot {
    pub fn scope(&self)->&Scope { &self.scope }
    pub fn sequence(&self)->u64 { self.sequence }
    pub fn revision(&self)->u64 { self.revision }
    pub fn inventory(&self)->&TrackInventory { &self.inventory }
    pub fn video_identity(&self)->Option<TransportIdentity> { self.video }
    pub fn selected_audio(&self)->Option<TransportIdentity> { self.selected_audio }
}
impl std::fmt::Debug for Snapshot {
    fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result {
        f.debug_struct("HlsObservedSnapshot").field("scope",&self.scope).field("sequence",&self.sequence)
            .field("revision",&self.revision).field("audio",&self.inventory.audio.len()).field("subtitles",&self.inventory.subtitles.len()).finish()
    }
}
#[derive(Clone,Debug)]
pub struct Candidate { inventory:TrackInventory, video:TransportIdentity, selected_audio:Option<TransportIdentity> }
impl Candidate {
    pub fn new(table:&ProgramTable,inventory:&TrackInventory,pids:&[(i32,u16)],video_pid:u16,selected_audio_pid:Option<u16>)
        ->Result<Self,&'static str>
    {
        let video=table.video_identity_for_pid(video_pid)?;
        let inventory=table.bind_inventory_for_video(inventory,pids,video_pid)?;
        let selected_audio=selected_audio_pid.map(|pid| {
            inventory.audio.iter().find_map(|row| inventory.transport_identity(row.stream_index).filter(|identity| identity.pid==pid))
                .ok_or("Selected audio PID is not in the video program")
        }).transpose()?;
        Ok(Self { inventory,video,selected_audio })
    }
    pub fn inventory(&self)->&TrackInventory { &self.inventory }
    pub fn video_identity(&self)->TransportIdentity { self.video }
    pub fn selected_audio(&self)->Option<TransportIdentity> { self.selected_audio }
}
#[derive(Clone,Debug)]
pub struct SubtitleTarget { scope:Scope,generation:u64,selection:Selection }
impl SubtitleTarget {
    pub fn scope(&self)->&Scope { &self.scope }
    pub fn generation(&self)->u64 { self.generation }
    pub fn selection(&self)->&Selection { &self.selection }
}
#[derive(Clone,Default)]
pub struct State { current:Option<Snapshot>,revision:u64,subtitle_generation:u64,subtitle_target:Option<SubtitleTarget> }
impl State {
    pub fn snapshot(&self)->Option<&Snapshot> { self.current.as_ref() }
    pub fn clear(&mut self) { self.current=None; self.revision=self.revision.saturating_add(1); self.clear_subtitle(); }
    pub fn clear_subtitle(&mut self) {
        self.subtitle_generation=self.subtitle_generation.saturating_add(1); self.subtitle_target=None;
    }
    pub fn subtitle_target(&self)->Option<&SubtitleTarget> { self.subtitle_target.as_ref() }
    pub fn target_is_current(&self,target:&SubtitleTarget)->bool {
        self.current.as_ref().is_some_and(|snapshot| snapshot.scope==target.scope)
            && self.subtitle_target.as_ref().is_some_and(|current| current.generation==target.generation && current.scope==target.scope)
    }
    pub fn set_subtitle(&mut self,scope:&Scope,selection:Option<Selection>)->Result<Option<SubtitleTarget>,&'static str> {
        let snapshot=self.current.as_ref().filter(|snapshot| snapshot.scope==*scope).ok_or("Observed HLS subtitle owner changed")?;
        if let Some(selection)=&selection {
            if selection.kind()!=TrackKind::Subtitle { return Err("Observed HLS subtitle target is not a subtitle"); }
            selection.resolve(&snapshot.inventory)?;
        }
        self.clear_subtitle();
        self.subtitle_target=selection.map(|selection| SubtitleTarget { scope:scope.clone(),generation:self.subtitle_generation,selection });
        Ok(self.subtitle_target.clone())
    }
    /// Worker publication edge only: the complete validated segment has already
    /// been fed successfully. A demux/read result alone must never call this.
    pub(crate) fn commit_candidate(&mut self,scope:Scope,sequence:u64,candidate:Candidate)->Result<Snapshot,&'static str> {
        self.commit_inner(scope,sequence,candidate.inventory,Some(candidate.video),candidate.selected_audio)
    }
    #[cfg(any(test,feature="test-support"))]
    pub(crate) fn commit(&mut self,scope:Scope,sequence:u64,inventory:TrackInventory)->Result<Snapshot,&'static str> {
        self.commit_inner(scope,sequence,inventory,None,None)
    }
    #[cfg(any(test,feature="test-support"))]
    pub(crate) fn commit_selected(&mut self,scope:Scope,sequence:u64,inventory:TrackInventory,selected_audio:Option<TransportIdentity>)
        ->Result<Snapshot,&'static str> { self.commit_inner(scope,sequence,inventory,None,selected_audio) }
    fn commit_inner(&mut self,scope:Scope,sequence:u64,mut inventory:TrackInventory,video:Option<TransportIdentity>,selected_audio:Option<TransportIdentity>)
        ->Result<Snapshot,&'static str>
    {
        validate_inventory(&inventory)?;
        if self.current.as_ref().is_some_and(|old| old.scope==scope && sequence<=old.sequence) {
            return Err("Observed HLS segment sequence did not advance");
        }
        if let Some(selected)=selected_audio {
            if !inventory.audio.iter().any(|row| inventory.transport_identity(row.stream_index)==Some(selected)) {
                return Err("Observed selected audio PID is absent");
            }
        }
        if self.subtitle_target.as_ref().is_some_and(|target| target.scope!=scope || target.selection.resolve(&inventory).is_err()) {
            self.clear_subtitle();
        }
        inventory.mark_incomplete();
        self.revision=self.revision.checked_add(1).ok_or("Observed HLS revision overflow")?;
        let snapshot=Snapshot { scope,sequence,revision:self.revision,inventory,video,selected_audio };
        self.current=Some(snapshot.clone()); Ok(snapshot)
    }
}
#[derive(Clone,Debug,PartialEq,Eq)]
pub struct Selection { kind: TrackKind, identity: TransportIdentity, codec: String, channels:i64, sample_rate:i64, bit_depth:i64 }
impl Selection {
    pub fn new(inventory:&TrackInventory,kind:TrackKind,ordinal:i32)->Result<Self,&'static str> {
        validate_inventory(inventory)?;
        let row=usize::try_from(ordinal).ok().and_then(|i| rows(inventory,kind).get(i)).ok_or("Observed HLS track is missing")?;
        Ok(Self { kind,identity:inventory.transport_identity(row.stream_index).ok_or("Observed HLS PID is missing")?,
            codec:row.codec.clone(),channels:row.channels,sample_rate:row.sample_rate,bit_depth:row.bit_depth })
    }
    pub fn kind(&self)->TrackKind { self.kind }
    pub fn transport_identity(&self)->TransportIdentity { self.identity }
    pub fn codec(&self)->&str { &self.codec }
    pub fn resolve(&self,current:&TrackInventory)->Result<i32,&'static str> {
        validate_inventory(current)?;
        let mut found=rows(current,self.kind).iter().filter(|row| current.transport_identity(row.stream_index)==Some(self.identity));
        let row=found.next().ok_or("Selected HLS PID is absent from this segment")?;
        if found.next().is_some() || row.codec!=self.codec || row.channels!=self.channels
            || row.sample_rate!=self.sample_rate || row.bit_depth!=self.bit_depth {
            return Err("Selected HLS transport codec shape changed");
        }
        Ok(row.ordinal)
    }
    pub fn resolve_stream_index(&self,current:&TrackInventory)->Result<i32,&'static str> {
        let ordinal=self.resolve(current)?;
        Ok(rows(current,self.kind)[ordinal as usize].stream_index)
    }
}

#[cfg(test)]
#[path="hls_observed_tests.rs"]
mod tests;

#[cfg(any(test,feature="test-support"))]
impl Snapshot {
    pub fn for_test(scope:Scope,sequence:u64,inventory:TrackInventory)->Result<Self,&'static str> {
        State::default().commit(scope,sequence,inventory)
    }
    /// Synthetic menu/retention contract only, no transport or decoder proof.
    pub fn fixture_for_test(scope:Scope)->Self {
        let row=|kind,index,ordinal,codec:&str| InspectedTrack { kind,stream_index:index,ordinal,codec:codec.into(),
            language:"eng".into(),title:"Same label".into(),channels:if kind==TrackKind::Audio {2} else {0},
            sample_rate:if kind==TrackKind::Audio {48000} else {0},bit_depth:0,decoder_available:kind==TrackKind::Audio,
            disposition_known:false,default:false,forced:false };
        let identity=|pid,stream_type| TransportIdentity { transport_stream_id:1,program_id:1,pid,stream_type };
        let inventory=TrackInventory::from_partial(vec![row(TrackKind::Audio,1,0,"aac"),row(TrackKind::Audio,3,1,"aac")],
            vec![row(TrackKind::Subtitle,4,0,"dvb_teletext")]).with_transport_for_test(&[(1,identity(0x101,0x0f)),
                (3,identity(0x102,0x0f)),(4,identity(0x120,0x06))]).unwrap();
        State::default().commit_selected(scope,0,inventory,Some(identity(0x101,0x0f))).unwrap()
    }
}

impl Snapshot {
    /// Sequence/revision advancement alone does not replace a user's menu choice.
    pub fn same_catalog(&self,other:&Self)->bool {
        self.scope==other.scope && self.inventory==other.inventory && self.video==other.video && self.selected_audio==other.selected_audio
    }
}
static OBSERVED:std::sync::Mutex<Option<State>>=std::sync::Mutex::new(None);
#[derive(Clone)]
pub struct SubtitleRequest { playback:crate::route::PlaybackIdentity,source:String,variant:String,generation:u64,selection:Selection }
impl std::fmt::Debug for SubtitleRequest {
    fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result {
        f.debug_struct("HlsSubtitleRequest").field("generation",&self.generation).field("selection",&self.selection).finish_non_exhaustive()
    }
}
impl SubtitleRequest {
    pub fn generation(&self)->u64 { self.generation }
    pub fn selection(&self)->&Selection { &self.selection }
    pub fn matches_scope(&self,scope:&Scope)->bool {
        scope.playback()==Some(&self.playback) && scope.source==self.source && scope.variant==self.variant
    }
}
static SUBTITLE:std::sync::Mutex<(u64,Option<SubtitleRequest>)>=std::sync::Mutex::new((0,None));
pub fn observed_hls_tracks()->Option<Snapshot> {
    let snapshot=OBSERVED.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|state| state.snapshot().cloned())?;
    let epoch=u32::try_from(snapshot.scope.worker()).ok()?;
    (snapshot.scope.playback().is_some() && super::SHARED.native_load_elapsed(epoch).is_some()).then_some(snapshot)
}
pub fn snapshot_is_current(snapshot:&Snapshot)->bool {
    observed_hls_tracks().is_some_and(|current| current.same_catalog(snapshot))
}
pub(crate) fn commit_observed(scope:Scope,sequence:u64,candidate:Candidate)->Result<Snapshot,&'static str> {
    let epoch=u32::try_from(scope.worker()).map_err(|_| "Invalid observed HLS worker")?;
    if scope.playback().is_none() || !super::SHARED.native_load_elapsed(epoch).is_some() { return Err("Observed HLS worker is inactive"); }
    OBSERVED.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(State::default).commit_candidate(scope,sequence,candidate)
}
pub(crate) fn retire_observed(native_epoch:u32) {
    let mut state=OBSERVED.lock().unwrap_or_else(|e| e.into_inner());
    if state.as_ref().and_then(|state| state.snapshot()).is_some_and(|snapshot| snapshot.scope.worker()==u64::from(native_epoch)) {
        *state=None;
    }
}
pub fn set_subtitle_selection(snapshot:&Snapshot,selection:Selection)->Result<(),&'static str> {
    if !snapshot_is_current(snapshot) || selection.kind()!=TrackKind::Subtitle { return Err("Observed HLS subtitle owner changed"); }
    selection.resolve(snapshot.inventory())?;
    let playback=snapshot.scope.playback().cloned().ok_or("Observed HLS logical owner is missing")?;
    let mut desired=SUBTITLE.lock().unwrap_or_else(|e| e.into_inner());
    desired.0=desired.0.checked_add(1).ok_or("Observed HLS subtitle generation overflow")?;
    desired.1=Some(SubtitleRequest { playback,source:snapshot.scope.source.clone(),variant:snapshot.scope.variant.clone(),
        generation:desired.0,selection });
    Ok(())
}
pub fn clear_subtitle_selection() {
    let mut desired=SUBTITLE.lock().unwrap_or_else(|e| e.into_inner());
    desired.0=desired.0.saturating_add(1); desired.1=None;
}
pub fn subtitle_request(playback:&crate::route::PlaybackIdentity,source:&str)->Option<SubtitleRequest> {
    let request=SUBTITLE.lock().unwrap_or_else(|e| e.into_inner()).1.clone()?;
    (request.playback==*playback && plx_data::ott::http_url(source).is_some_and(|source| source==request.source)).then_some(request)
}
pub fn subtitle_request_is_current(request:&SubtitleRequest)->bool {
    SUBTITLE.lock().unwrap_or_else(|e| e.into_inner()).1.as_ref().is_some_and(|current|
        current.generation==request.generation && current.playback==request.playback && current.source==request.source && current.variant==request.variant)
}

#[cfg(test)]
mod video_codec_tests {
    use super::*;
    fn table(stream_type:u8) -> ProgramTable {
        ProgramTable {
            pat: Pat { transport_stream_id:1, version:0, programs:BTreeMap::from([(1,0x1000)]) },
            pmts:BTreeMap::from([(1,Pmt { version:0, streams:vec![Elementary {
                identity:TransportIdentity { transport_stream_id:1, program_id:1, pid:0x100, stream_type },
                descriptors:Vec::new(),
            }] })]),
        }
    }
    #[test]
    fn candidate_binds_only_validated_h264_or_hevc_video_pid() {
        let inventory=TrackInventory::from_partial(Vec::new(),Vec::new());
        for (stream_type,codec) in [(0x1b,"h264"),(0x24,"hevc")] {
            let table=table(stream_type);
            let candidate=Candidate::new(&table,&inventory,&[],0x100,None).unwrap();
            assert_eq!(candidate.video_identity().stream_type,stream_type);
            assert_eq!(table.identity_for_pid(0x100,codec).unwrap(),candidate.video_identity());
            assert!(!candidate.inventory().is_complete());
        }
        for kind in [0x02,0x0f,0x81] {
            assert!(Candidate::new(&table(kind),&inventory,&[],0x100,None).is_err());
        }
    }
    #[test]
    fn observed_video_pid_rejects_ambiguous_program_and_codec_mismatch() {
        let mut table=table(0x24);
        assert!(table.identity_for_pid(0x100,"h264").is_err());
        assert!(table.video_identity_for_pid(0x101).is_err());
        let mut second=table.pmts[&1].clone();
        second.streams[0].identity.program_id=2;
        table.pmts.insert(2,second);
        assert!(table.video_identity_for_pid(0x100).is_err());
        let inventory=TrackInventory::from_partial(Vec::new(),Vec::new());
        assert!(Candidate::new(&table,&inventory,&[],0x100,None).is_err());
    }
}
