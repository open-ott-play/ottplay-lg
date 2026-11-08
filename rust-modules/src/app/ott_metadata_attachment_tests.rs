use super::*;
use crate::app::ott_m3u_attachment::{Authorities, Registry, SETTINGS_KEY};
use plx_data::ott::{favorites::{Locator, Origin}, PortalRequest, ProviderProfiles};
use std::collections::BTreeMap;
use std::sync::mpsc::channel;

#[test]
fn nested_preview_keeps_parent_identity_and_uses_portal_metadata_snapshot() {
    let mut profiles = ProviderProfiles::default();
    profiles.profiles.push(ProviderProfile { id:"m3u-9".into(),name:"Home".into(),kind:ProviderKind::M3u,
        source:"https://must-not-fetch.invalid/list.m3u".into(),source_alias:"Home".into(),enabled:true });
    let registry = Registry::read(&BTreeMap::from([(SETTINGS_KEY.into(),json!({"version":1,"entries":[{
        "id":"attached","parent_profile_id":"m3u-9","kind":"vportal",
        "link":"portal::[key:fixture]https://portal.invalid/api"
    }]}))]),&profiles).unwrap();
    let mut authorities = Authorities::default(); authorities.observe(&registry,&profiles).unwrap();
    let receipt = authorities.capture("m3u-9").unwrap();
    let favorite = Favorite { title:"Film".into(),origin:Origin { provider:"m3u".into(),profile:"Home".into(),
        source_alias:"Home".into(),subprofile:None }, locator:Locator::VPortal { id_field:"id".into(),id:"7".into() } };
    let mut page = PortalRequest::root().parse_response(&json!({"type":"videoportal","items":[{
        "type":"stream","title":"Film","id":7,"url":"/movie.mp4","description":"Full synopsis.","poster":"/cover.jpg"
    }]}).to_string(),&receipt.effective_profile().unwrap()).unwrap();
    let row = page.catalog.entries.remove(0);
    let source = FavoriteSource::M3uAttachment(receipt);
    let cancel = AtomicBool::new(false);
    let worker = OffFrame::for_test();
    let (tx,rx) = channel();
    assert!(run_favorite(source.owner(),&favorite,41,&cancel,&tx,||Ok(row.clone()),
        |entry|source.inspect(&worker,&favorite,entry,41,Query::CurrentNext { now: 0 },&cancel)));
    let Event::Preview(value)=rx.try_recv().unwrap() else { panic!("expected preview"); };
    assert_eq!(value.profile_id,"m3u-9");
    assert_eq!(value.generation,41);
    assert_eq!(value.entry_id,format!("favorite:{}",favorite.identity()));
    assert_eq!(value.entry,Some(row.clone()));
    let value=value.result.unwrap();
    assert_eq!(value["description"],"Full synopsis.");
    assert_eq!(value["poster"],row.poster);
    assert!(value.get("programmes").is_none());
    assert!(!value.to_string().contains("fixture"));
}
