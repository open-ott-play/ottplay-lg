//! Restricted DNS response decoder. Compression pointers remain inside this
//! response, names are bounded to 255 octets, and every section is walked.
use super::Record;
#[cfg(any(target_os = "linux", test))]
use super::{Error, PTR, SRV, TXT, wire_name};
#[cfg(any(target_os = "linux", test))]
use std::time::Duration;
#[cfg(any(target_os = "linux", test))]
const MAX_RECORDS: usize = 128;
#[cfg(any(target_os = "linux", test))]
const MAX_JUMPS: usize = 128;
pub(super) struct Answer { pub question: String, pub records: Vec<Record> }
#[cfg(any(target_os = "linux", test))]
fn u16_at(b: &[u8], at: usize) -> Result<u16, Error> {
    let s = b.get(at..at + 2).ok_or(Error::Malformed)?;
    Ok(u16::from_be_bytes([s[0], s[1]]))
}
#[cfg(any(target_os = "linux", test))]
fn header(b: &[u8]) -> Result<(usize, usize, usize, usize), Error> {
    if b.len() < 12 || b[2] & 0xf8 != 0x80 || b[2] & 2 != 0
        || !matches!(b[3] & 15, 0 | 3) { return Err(Error::Malformed); }
    Ok((usize::from(u16_at(b, 4)?), usize::from(u16_at(b, 6)?),
        usize::from(u16_at(b, 8)?), usize::from(u16_at(b, 10)?)))
}
/// libc gives no response length on NXDOMAIN/NODATA. Only its fixed header is
/// usable then; a positive answer without a length must never become empty.
#[cfg(any(target_os = "linux", test))]
pub(super) fn negative(b: &[u8], question: &str) -> Result<Answer, Error> {
    if b.get(2).is_none_or(|v| v & 0x80 == 0) { return Err(Error::Unavailable); }
    let (qd, an, _, _) = header(b)?;
    if qd > 1 || an != 0 { return Err(Error::Malformed); }
    Ok(Answer { question: question.into(), records: Vec::new() })
}
#[cfg(any(target_os = "linux", test))]
fn name(b: &[u8], mut at: usize) -> Result<(Vec<u8>, usize), Error> {
    let mut output = Vec::new();
    let mut next = None;
    let mut jumps = 0;
    loop {
        let size = *b.get(at).ok_or(Error::Malformed)?;
        if size & 0xc0 == 0xc0 {
            let low = *b.get(at + 1).ok_or(Error::Malformed)?;
            next.get_or_insert(at + 2);
            at = (usize::from(size & 0x3f) << 8) | usize::from(low);
            jumps += 1;
            if jumps > MAX_JUMPS { return Err(Error::Malformed); }
        } else {
            if size > 63 { return Err(Error::Malformed); }
            at += 1;
            output.push(size);
            let end = at + usize::from(size);
            output.extend_from_slice(b.get(at..end).ok_or(Error::Malformed)?);
            if output.len() > 255 { return Err(Error::Malformed); }
            at = end;
            if size == 0 { return Ok((output, next.unwrap_or(at))); }
        }
    }
}
#[cfg(any(target_os = "linux", test))]
fn text(data: &[u8]) -> Result<String, Error> {
    if data == [0] { return Ok(".".into()); }
    wire_name(data).ok_or(Error::Malformed)
}
#[cfg(any(target_os = "linux", test))]
pub(super) fn parse(b: &[u8], requested: &str, kind: u16, search: bool)
    -> Result<Answer, Error>
{
    let (qd, an, ns, ar) = header(b)?;
    if qd != 1 || an + ns + ar > MAX_RECORDS { return Err(Error::Malformed); }
    let (qname, mut at) = name(b, 12)?;
    let question = text(&qname)?;
    if u16_at(b, at)? != kind || u16_at(b, at + 2)? != 1
        || (!search && question != requested) { return Err(Error::Malformed); }
    at += 4;
    let mut records = Vec::new();
    for index in 0..an + ns + ar {
        let (owner, end) = name(b, at)?;
        at = end;
        let rtype = u16_at(b, at)?;
        let class = u16_at(b, at + 2)?;
        let fixed = b.get(at..at + 10).ok_or(Error::Malformed)?;
        let ttl = u32::from_be_bytes(fixed[4..8].try_into().map_err(|_| Error::Malformed)?);
        let size = usize::from(u16_at(b, at + 8)?);
        at += 10;
        let end = at + size;
        let raw = b.get(at..end).ok_or(Error::Malformed)?;
        if index < an && rtype == kind {
            if class != 1 || text(&owner)? != question { return Err(Error::Malformed); }
            let data = match kind {
                PTR => { let (value, next) = name(b, at)?;
                    if next != end { return Err(Error::Malformed); } value },
                SRV => { if size < 7 { return Err(Error::Malformed); }
                    let (value, next) = name(b, at + 6)?;
                    if next != end { return Err(Error::Malformed); }
                    let mut result = raw[..6].to_vec(); result.extend(value); result },
                TXT => { if size > 2048 { return Err(Error::TooLarge); } raw.to_vec() },
                _ => return Err(Error::Malformed),
            };
            records.push(Record { data, ttl: Duration::from_secs(u64::from(ttl)) });
        }
        at = end;
    }
    if at != b.len() || (an != 0 && records.is_empty())
        || (b[3] & 15 == 3 && an != 0) { return Err(Error::Malformed); }
    Ok(Answer { question, records })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compression_is_bounded_and_preserves_consumed_length() {
        let b = [1,b'a',0,0xc0,0];
        assert_eq!(name(&b,3).unwrap(),(vec![1,b'a',0],5));
        assert!(name(&[0xc0,0],0).is_err());
        assert!(name(&[0xc0,255],0).is_err());
        assert!(name(&[64,0],0).is_err());
        assert!(name(&[2,b'a'],0).is_err());
    }
    #[test]
    fn no_response_or_truncation_never_becomes_empty() {
        assert!(negative(&[0;12],"x.").is_err());
        let mut b=[0;12];b[2]=0x80;
        assert!(negative(&b,"x.").unwrap().records.is_empty());
        b[2]|=2;assert!(negative(&b,"x.").is_err());b[2]=0x80;
        b[7]=1;assert!(negative(&b,"x.").is_err());
    }
    #[test]
    fn full_empty_packet_checks_question_and_trailing_data() {
        let mut b=vec![0,0,0x81,0x80,0,1,0,0,0,0,0,0,1,b'x',0,0,12,0,1];
        assert!(parse(&b,"x.",PTR,false).unwrap().records.is_empty());
        assert!(parse(&b,"other.",PTR,false).is_err());
        assert!(parse(&b,"x.",SRV,false).is_err());
        b.push(0);assert!(parse(&b,"x.",PTR,false).is_err());
    }
}
