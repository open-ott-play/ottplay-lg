use super::*;
const SPS:&[u8]=&[0,0,0,1,0x67,0x42,0x00,0x1e,0xc0];
const PPS:&[u8]=&[0,0,0,1,0x68,0xe0];
const SEI:&[u8]=&[0,0,0,1,0x06,6,1,0xc4,0x80];
const INTRA:&[u8]=&[0,0,0,1,0x61,0xbc];
fn access(nals:&[&[u8]])->Vec<u8>{nals.concat()}
#[test]
fn zero_recovery_requires_exact_zero_delay_unbroken_syntax(){
    assert!(zero_recovery_point(SEI));
    for bad in [&[0x06,6,1,0x44,0x80][..],&[0x06,6,1,0x84,0x80],&[0x06,6,1,0xe4,0x80],
        &[0x06,6,1,0xcc,0x80],&[0x06,6,1,0xc0,0x80],&[0x06,6,2,0xc4,0,0x80],&[0x06,6,1,0xc4],
        &[0x06,6,1,0xc4,6,1,0xc4,0x80]]{assert!(!zero_recovery_point(bad));}
}
#[test]
fn recovery_entry_requires_associated_intra_and_matching_parameter_chain(){
    assert!(recovery_entry(&access(&[SPS,PPS,SEI,INTRA]),SPS,PPS));
    assert!(recovery_entry(&access(&[SEI,INTRA]),SPS,PPS));
    assert!(!recovery_entry(&access(&[INTRA,SEI]),SPS,PPS));
    assert!(!recovery_entry(&access(&[SEI,INTRA,SPS]),SPS,PPS));
    assert!(!recovery_entry(&access(&[SEI,INTRA,INTRA]),SPS,PPS));
    assert!(!recovery_entry(&access(&[SEI,INTRA]),SPS,&[0x68,0x4a]));
    assert!(!recovery_entry(&access(&[SEI,&[0,0,1,0x61,0xf0]]),SPS,PPS));
    assert!(!recovery_entry(&access(&[SEI,&[0,0,1,0x01,0xbc]]),SPS,PPS));
}
#[test]
fn malformed_sei_emulation_prevention_and_truncation_fail_closed(){
    for nal in [&[0x06,5,4,0,0,3,4,0x80][..],&[0x06,5,3,0,0,3],&[0x06,5,3,0,0,1,0x80],&[0x06,255]]{
        assert!(!zero_recovery_point(nal));
    }
    let huge=vec![0;NAL_BYTES+1];assert!(!zero_recovery_point(&huge));
}

#[test]
fn recovery_never_crosses_au_delimiters_or_unsupported_extensions(){
    for nal in [&[0,0,1,9,0x10][..],&[0,0,1,10,0x80],&[0,0,1,11,0x80],&[0,0,1,0x73,0xbc],
        &[0,0,1,0x74,0xbc],&[0,0,1,0x75,0xbc]] {
        assert!(!recovery_entry(&access(&[SEI,nal,INTRA]),SPS,PPS));
    }
}
#[test]
fn large_realistic_intra_payload_keeps_bounded_header_proof(){
    let mut intra=INTRA.to_vec();intra.extend_from_slice(&vec![0x55;200_000]);
    assert!(recovery_entry(&access(&[SPS,PPS,SEI,&intra]),SPS,PPS));
}
