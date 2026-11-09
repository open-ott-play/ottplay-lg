use super::*;
#[test]
fn disposable_picture_excludes_references_entry_and_persistent_sei() {
    let mut state=State::default();
    assert!(!run(&mut state,&[params(),picture(19)].concat()).0.disposable);
    assert!(run(&mut state,&picture(0)).0.disposable);
    assert!(!run(&mut state,&picture(1)).0.disposable);
    let timing=nal(39,&[1,3,0x12,0x34,0x56,0x80]);
    assert!(run(&mut state,&[timing,picture(0)].concat()).0.disposable);
    for kind in [0,4,129,137,144] {
        let persistent=nal(39,&[kind,1,0x55,0x80]);
        assert!(!run(&mut state,&[persistent,picture(0)].concat()).0.disposable);
    }
    let malformed=nal(39,&[1,3,0x12,0x80]);
    assert!(!run(&mut state,&[malformed,picture(0)].concat()).0.disposable);
}
fn nal(kind:u8,rbsp:&[u8])->Vec<u8>{let mut result=vec![0,0,0,1,kind<<1,1];let mut zeros=0;for &byte in rbsp{if zeros>=2&&byte<=3{result.push(3);zeros=0}result.push(byte);zeros=if byte==0{zeros+1}else{0};}result}
fn params()->Vec<u8>{let vps=nal(32,&[0x08]);let mut sps=vec![1];sps.extend_from_slice(&[0;12]);sps.push(0xc0);[vps,nal(33,&sps),nal(34,&[0xe0])].concat()}
fn picture(kind:u8)->Vec<u8>{nal(kind,if (16..=21).contains(&kind){&[0xa0]}else{&[0xc0]})}
fn run(state:&mut State,bytes:&[u8])->(AccessInfo,Vec<u8>){let mut out=Vec::new();let info=annexb(bytes,state,&mut out).unwrap();(info,out)}
#[test]
fn cold_hevc_requires_actual_irap_and_matching_parameter_sets(){let mut state=State::default();assert!(state.needs_entry());
 let (info,out)=run(&mut state,&[params(),picture(1)].concat());assert_eq!(info.decision,Decision::Skip);assert!(out.is_empty());
 let (info,out)=run(&mut state,&picture(19));assert!(info.key&&info.vcl);assert_eq!(info.decision,Decision::Start);assert!(!state.needs_entry());
 assert!(out.starts_with(&[0,0,0,1,64,1]));assert!(!info.params_changed);
 assert!(annexb(&picture(19),&mut State::default(),&mut Vec::new()).is_err());}
#[test]
fn cold_cra_drops_only_associated_rasl_until_trailing_picture(){let mut state=State::default();
 let (start,_)=run(&mut state,&[params(),picture(21)].concat());assert_eq!(start.decision,Decision::Start);
 for kind in [8,9]{let (info,out)=run(&mut state,&picture(kind));assert_eq!(info.decision,Decision::Skip);assert!(out.is_empty());}
 assert_eq!(run(&mut state,&picture(1)).0.decision,Decision::Continue);
 assert_eq!(run(&mut state,&picture(8)).0.decision,Decision::Continue);
 assert_eq!(run(&mut state,&picture(21)).0.decision,Decision::Continue);
 assert_eq!(run(&mut state,&picture(9)).0.decision,Decision::Continue,"ordinary continuous CRA does not discard RASL");}
#[test]
fn reserved_multilayer_temporal_and_parameter_chain_changes_fail_closed(){let mut state=State::default();run(&mut state,&[params(),picture(20)].concat());
 for kind in [10,15,22,23,31,41,62]{assert!(annexb(&picture(kind),&mut state,&mut Vec::new()).is_err());}
 let mut bad=picture(19);bad[5]=0;assert!(annexb(&bad,&mut state,&mut Vec::new()).is_err());
 let mut bad=picture(19);bad[4]|=1;assert!(annexb(&bad,&mut state,&mut Vec::new()).is_err());
 let wrong=nal(34,&[0x4a]);assert!(annexb(&[wrong,picture(19)].concat(),&mut state,&mut Vec::new()).is_err());
 assert!(annexb(&[picture(19),nal(34,&[0xe0])].concat(),&mut state,&mut Vec::new()).is_err());}
