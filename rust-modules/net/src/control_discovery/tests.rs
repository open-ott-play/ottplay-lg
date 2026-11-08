use super::*;
fn name(value: &str) -> Vec<u8> {
    let mut data = Vec::new();
    for label in value.trim_end_matches('.').split('.') {
        data.push(label.len() as u8); data.extend(label.bytes());
    }
    data.push(0); data
}
fn packet(owner: &str, kind: u16, data: &[u8]) -> Vec<u8> {
    let mut b=vec![0,1,0x81,0x80,0,1,0,1,0,0,0,0];
    b.extend(name(owner)); b.extend(kind.to_be_bytes()); b.extend([0,1]);
    b.extend([0xc0,12]);b.extend(kind.to_be_bytes());b.extend([0,1,0,0,0,30]);
    b.extend((data.len() as u16).to_be_bytes()); b.extend(data); b
}
fn txt(items: &[&str]) -> Vec<u8> {
    items.iter().flat_map(|v| std::iter::once(v.len() as u8).chain(v.bytes())).collect()
}
fn fixtures(owner: &str, kind: u16, search: bool) -> Result<wire::Answer, Error> {
    let service="_ottplay-ctrl._tcp.lan.example.";
    let id="home._ottplay-ctrl._tcp.lan.example.";
    let actual=if search { service } else { owner };
    let data=match kind {
        PTR=>name(id),
        SRV=>{ let mut b=vec![0,0,0,0,1,187];b.extend(name("controller.example."));b },
        TXT=>txt(&["txtvers=1","scheme=https","path=/ott-control"]),
        _=>unreachable!(),
    };
    wire::parse(&packet(actual,kind,&data),owner,kind,search)
}
#[test]
fn home_ptr_srv_txt_uses_the_existing_https_pairing_contract() {
    let servers=discover_with(vec!["LAN.example.".into(),"lan.example".into()],||true,fixtures).unwrap();
    assert_eq!(servers,vec![ControlServer { id:"home._ottplay-ctrl._tcp.lan.example.".into(),
        domain:"lan.example".into(),address:"https://controller.example/ott-control".into() }]);
}
#[test]
fn unreadable_resolver_file_can_use_os_suffix_search_without_a_hardcoded_domain() {
    let mut queried=Vec::new();
    let servers=discover_with(vec![],||true,|owner,kind,search| {
        queried.push((owner.to_owned(),kind,search));fixtures(owner,kind,search)
    }).unwrap();
    assert_eq!(servers.len(),1);
    assert_eq!(queried[0],(SERVICE.into(),PTR,true));
    assert_eq!(queried.len(),3);
    assert!(queried[1..].iter().all(|q|!q.2));
}
#[test]
fn expired_or_canceled_discovery_does_not_start_another_lookup() {
    let calls=std::cell::Cell::new(0);
    let result=discover_with(vec![],||calls.get()<1,|owner,kind,search| {
        calls.set(calls.get()+1);fixtures(owner,kind,search)
    });
    assert_eq!(result,Err(Error::TimedOut));assert_eq!(calls.get(),1);
}
#[test]
fn malformed_and_missing_instance_metadata_are_errors_not_empty() {
    for invalid in [txt(&["txtvers=1","scheme=http","path=/control"]),
        txt(&["txtvers=1","scheme=https","path=/../control"]),
        txt(&["txtvers=1","scheme=https","path=/control","path=/other"]),
        vec![255,b'x']] {
        let result=discover_with(vec![],||true,|owner,kind,search| {
            if kind==TXT { wire::parse(&packet(owner,kind,&invalid),owner,kind,search) }
            else { fixtures(owner,kind,search) }
        });assert_eq!(result,Err(Error::Malformed));
    }
    let result=discover_with(vec![],||true,|owner,kind,search| {
        if kind==SRV { Ok(wire::Answer {question:owner.into(),records:vec![]}) }
        else { fixtures(owner,kind,search) }
    });assert_eq!(result,Err(Error::Malformed));
}
#[test]
fn well_formed_empty_is_distinct_from_network_and_parser_failure() {
    let empty=discover_with(vec![],||true,|owner,_,_|Ok(wire::Answer{question:owner.into(),records:vec![]}));
    assert!(empty.unwrap().is_empty());
    for error in [Error::Unavailable,Error::Malformed,Error::Unsupported,Error::TooLarge] {
        assert_eq!(discover_with(vec![],||true,|_,_,_|Err(error)),Err(error));
    }
}
#[test]
fn wrong_owner_and_all_truncated_packets_are_rejected() {
    let owner="_ottplay-ctrl._tcp.lan.example.";
    let b=packet(owner,PTR,&name("home._ottplay-ctrl._tcp.lan.example."));
    for end in 0..b.len() { assert!(wire::parse(&b[..end],owner,PTR,false).is_err(),"{end}"); }
    assert!(wire::parse(&b,"_ottplay-ctrl._tcp.other.",PTR,false).is_err());
    let invalid=packet(owner,PTR,&name("home._ottplay-ctrl._tcp.other."));
    assert_eq!(discover_with(vec!["lan.example".into()],||true,|_,_,_|wire::parse(&invalid,owner,PTR,false)),Err(Error::Malformed));
}
#[test]
fn duplicate_instance_metadata_is_rejected_instead_of_choosing_an_endpoint() {
    let result=discover_with(vec![],||true,|owner,kind,search| {
        let mut answer=fixtures(owner,kind,search)?;
        if kind==SRV { answer.records.push(Record {data:answer.records[0].data.clone(),ttl:Duration::from_secs(30)}); }
        Ok(answer)
    });assert_eq!(result,Err(Error::Malformed));
}
