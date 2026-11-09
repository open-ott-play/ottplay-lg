use super::*;
// Structural header vectors only: zero-filled payload deliberately makes no
// claim about CRC or decoder output. Actual source/decoder fixture is separate.
fn vector(fscod: u8, size_code: u8, mode: u8, lfe: u8) -> Vec<u8> {
    let rates = [48_000,44_100,32_000];
    let bitrate = [32,40,48,56,64,80,96,112,128,160,192,224,256,320,384,448,512,576,640][usize::from(size_code / 2)];
    let size = if fscod == 1 { 2 * (320 * bitrate / 147 + usize::from(size_code & 1)) }
        else { bitrate * 192_000 / rates[usize::from(fscod)] };
    let mut out = vec![0;size]; out[..2].copy_from_slice(&[0x0b,0x77]);
    out[4] = fscod << 6 | size_code; out[5] = 8 << 3;
    let mut bit = 48;
    let mut put = |value: u8, n: usize| { for i in (0..n).rev() { out[bit/8] |= ((value >> i)&1) << (7-bit%8); bit+=1; } };
    put(mode,3);
    if mode == 2 { put(0,2); } else {
        if mode & 1 != 0 && mode != 1 { put(0,2); }
        if mode & 4 != 0 { put(0,2); }
    }
    put(lfe,1); out
}
#[test]
fn all_rates_sizes_and_channel_modes() {
    for (fscod,rate) in [48_000,44_100,32_000].into_iter().enumerate() {
        for code in 0..38 { for mode in 0..8 { for lfe in 0..2 {
            let channels = [2,1,2,3,3,4,4,5][usize::from(mode)] + i64::from(lfe);
            let raw = vector(fscod as u8,code,mode,lfe);
            let actual = frame(&raw,channels,rate).unwrap();
            assert_eq!(actual.duration_ns,1536 * 1_000_000_000 / rate);
        }}}
    }
}
#[test]
fn rejects_truncation_concatenation_shape_change_and_eac3() {
    let raw = vector(0,20,7,1);
    assert_eq!(frame(&raw,6,48_000).unwrap().duration_ns,32_000_000);
    for size in 0..raw.len() { assert!(frame(&raw[..size],6,48_000).is_err()); }
    let mut joined = raw.clone(); joined.extend_from_slice(&raw);
    assert!(frame(&joined,6,48_000).is_err());
    assert!(frame(&raw,2,48_000).is_err()); assert!(frame(&raw,6,44_100).is_err());
    for bsid in 9..32 { let mut wrong = raw.clone(); wrong[5] = bsid << 3; assert!(frame(&wrong,6,48_000).is_err()); }
    for header in [0xc0,38,63] { let mut wrong=raw.clone(); wrong[4]=header; assert!(frame(&wrong,6,48_000).is_err()); }
}
#[test]
fn does_not_admit_unknown_or_other_codecs() {
    assert!(!format("eac3",6,48_000)); assert!(!format("AC3",6,48_000));
    for (channels,rate) in [(0,48_000),(7,48_000),(2,0),(2,24_000)] { assert!(!format("ac3",channels,rate)); }
}
