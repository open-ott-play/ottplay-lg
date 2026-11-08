use super::super::Response;
use super::*;
use serde_json::json;
use std::collections::VecDeque;
struct Http {
    responses: VecDeque<(u16, Value)>,
    methods: Vec<String>,
    bodies: Vec<Value>,
    cancelled: bool,
    cancel_after: Option<usize>,
}
impl ProviderHttp for Http {
    fn request(&mut self, request: &Request) -> Result<Response, &'static str> {
        assert!(request.url.ends_with("/api/"));
        let body: Value = serde_json::from_slice(request.body.as_ref().unwrap()).unwrap();
        self.methods
            .push(body["method"].as_str().unwrap().to_string());
        self.bodies.push(body);
        let (status, value) = self.responses.pop_front().expect("unexpected request");
        if self.cancel_after == Some(self.methods.len()) {
            self.cancelled = true;
        }
        Ok(Response {
            status,
            body: serde_json::to_vec(&value).unwrap(),
        })
    }
    fn cancelled(&self) -> bool {
        self.cancelled
    }
}
fn http(rows: Vec<(u16, Value)>) -> Http {
    Http {
        responses: rows.into(),
        methods: Vec::new(),
        bodies: Vec::new(),
        cancelled: false,
        cancel_after: None,
    }
}
fn profile() -> ProviderProfile {
    ProviderProfile {
        id: "api".into(),
        name: "Portal".into(),
        kind: ott::ProviderKind::Stalker,
        source: r#"{"portal":"https://portal.invalid/api/","mac":"00:1a:79:01:02:03"}"#.into(),
        source_alias: String::new(),
        enabled: true,
    }
}
fn response(value: Value) -> (u16, Value) {
    (200, json!({"result":value}))
}
#[test]
fn authorization_renews_once_and_then_returns_fresh_catalog() {
    let mut h = http(vec![
        response(json!(true)),
        (401, json!({})),
        response(json!(true)),
        response(json!([{"id":7,"name":"News"}])),
    ]);
    let mut entries = Vec::new();
    super::super::visit_with(&profile(), &mut h, &mut |c| {
        entries.extend(c);
        Ok(())
    })
    .unwrap();
    assert_eq!(
        h.methods,
        ["handshake", "get_channels", "handshake", "get_channels"]
    );
    assert_eq!(entries.len(), 1);
}
#[test]
fn second_authorization_refusal_is_terminal() {
    let mut h = http(vec![
        response(json!(true)),
        (403, json!({})),
        response(json!(true)),
        (403, json!({})),
    ]);
    assert!(
        super::super::visit_with(&profile(), &mut h, &mut |_| panic!("must not publish")).is_err()
    );
    assert_eq!(h.methods.len(), 4);
}
#[test]
fn malformed_profile_never_performs_io_and_cancelled_reply_never_publishes() {
    let mut h = http(vec![]);
    let mut p = profile();
    p.source = r#"{"portal":"file:///api/","mac":"bad"}"#.into();
    assert!(super::super::visit_with(&p, &mut h, &mut |_| panic!("must not publish")).is_err());
    assert!(h.methods.is_empty());
    let mut h = http(vec![
        response(json!(true)),
        response(json!([{"id":7,"name":"News"}])),
    ]);
    h.cancel_after = Some(2);
    assert_eq!(
        super::super::visit_with(&profile(), &mut h, &mut |_| panic!("late publication")),
        Err("Provider request cancelled")
    );
}
#[test]
fn current_next_guide_sends_bounded_window_and_omits_expired_programmes() {
    let mut h = http(vec![
        response(json!(true)),
        response(
            json!([{"start":100,"end":200,"name":"Past"},{"start":200,"end":300,"name":"Current"},{"start":300,"end":400,"name":"Future"}]),
        ),
    ]);
    let p = profile();
    let c = Config::parse(&p.source).unwrap();
    let e = codec::catalog(&p, &c, &json!([{"id":7,"name":"News"}]))
        .unwrap()
        .entries
        .remove(0);
    let request = ott::guide::Request {
        context: ott::guide::Context {
            generation: 1,
            profile_id: p.id.clone(),
            entry_id: e.id.clone(),
        },
        query: ott::guide::Query::CurrentNext { now: 250 },
    };
    let result = super::super::guide_with(&p, &e, &request, &mut h).unwrap();
    assert_eq!(
        result
            .programmes
            .iter()
            .map(|p| p.title.as_str())
            .collect::<Vec<_>>(),
        ["Current", "Future"]
    );
    assert_eq!(result.coverage, ott::guide::Coverage::RequestedWindow);
    assert_eq!(h.bodies[1]["params"]["from"], 0);
    assert_eq!(h.bodies[1]["params"]["to"], 86650);
    assert_eq!(h.methods, ["handshake", "get_epg"]);
}

#[test]
fn dispatched_day_guide_sends_exact_window_and_filters_overlaps() {
    let p = profile();
    let c = Config::parse(&p.source).unwrap();
    let entry = codec::catalog(&p, &c, &json!([{"id":7,"name":"News"}]))
        .unwrap()
        .entries
        .remove(0);
    let request = ott::guide::Request {
        context: ott::guide::Context {
            generation: 9,
            profile_id: p.id.clone(),
            entry_id: entry.id.clone(),
        },
        query: ott::guide::Query::Day {
            start: 200,
            end: 400,
        },
    };
    let mut h = http(vec![
        response(json!(1)),
        response(json!([
            {"start":100,"end":200,"name":"Past"},
            {"start":190,"end":300,"name":"First"},
            {"start":300,"end":410,"name":"Last"},
            {"start":400,"end":500,"name":"Later"}
        ])),
    ]);
    let result = super::super::guide_with(&p, &entry, &request, &mut h).unwrap();
    assert_eq!(h.methods, ["handshake", "get_epg"]);
    assert_eq!(h.bodies[1]["params"]["ch_id"], "7");
    assert_eq!(h.bodies[1]["params"]["from"], 200);
    assert_eq!(h.bodies[1]["params"]["to"], 400);
    assert_eq!(
        result
            .programmes
            .iter()
            .map(|p| p.title.as_str())
            .collect::<Vec<_>>(),
        ["First", "Last"]
    );
    assert_eq!(result.query, request.query);
    assert!(result.can_publish(&request, false));
}

#[test]
fn invalid_day_query_is_refused_before_any_request() {
    let p = profile();
    let c = Config::parse(&p.source).unwrap();
    let entry = codec::catalog(&p, &c, &json!([{"id":7,"name":"News"}]))
        .unwrap()
        .entries
        .remove(0);
    let request = ott::guide::Request {
        context: ott::guide::Context {
            generation: 9,
            profile_id: p.id.clone(),
            entry_id: entry.id.clone(),
        },
        query: ott::guide::Query::Day {
            start: 100,
            end: 100,
        },
    };
    let mut h = http(vec![]);
    assert!(super::super::guide_with(&p, &entry, &request, &mut h).is_err());
    assert!(h.methods.is_empty());
}

#[test]
fn integer_handshake_continues_to_channel_discovery() {
    let mut h = http(vec![
        response(json!(1)),
        response(json!([{"id":7,"name":"News"}])),
    ]);
    let mut count = 0;
    super::super::visit_with(&profile(), &mut h, &mut |rows| {
        count += rows.len();
        Ok(())
    })
    .unwrap();
    assert_eq!(count, 1);
    assert_eq!(h.methods, ["handshake", "get_channels"]);
}

#[test]
fn dispatched_api_catalog_resolves_fresh_and_preserves_channel_identity() {
    let p = profile();
    let mut h = http(vec![
        response(json!(1)),
        response(json!([
            {"id":7,"name":"News","url":"https://stream.invalid/old?token=old"}
        ])),
        response(json!(1)),
        response(json!([
            {"id":7,"name":"News","url":"https://stream.invalid/new?token=new"}
        ])),
    ]);
    let mut rows = Vec::new();
    super::super::visit_with(&p, &mut h, &mut |batch| {
        rows.extend(batch);
        Ok(())
    })
    .unwrap();
    let wire = serde_json::to_string(&rows[0]).unwrap();
    assert!(!wire.contains("token=old"));
    let entry: CatalogEntry = serde_json::from_str(&wire).unwrap();
    assert_eq!(entry, rows[0]);
    let resolved = super::super::resolve_with(&p, &entry, &mut h).unwrap();
    assert_eq!(resolved, "https://stream.invalid:443/new?token=new");
    assert_eq!(
        h.methods,
        ["handshake", "get_channels", "handshake", "get_channels"]
    );
    assert!(h
        .bodies
        .iter()
        .all(|body| body["params"]["mac"] == "00:1a:79:01:02:03"));
}

#[test]
fn malformed_locator_and_guide_context_stop_before_http() {
    let p = profile();
    let c = Config::parse(&p.source).unwrap();
    let mut e = codec::catalog(&p, &c, &json!([{"id":7,"name":"News"}]))
        .unwrap()
        .entries
        .remove(0);
    let mut h = http(vec![]);
    e.media = MediaLocator::StalkerApi {
        channel_id: "7\n".into(),
    };
    assert!(super::super::resolve_with(&p, &e, &mut h).is_err());
    let request = ott::guide::Request {
        context: ott::guide::Context {
            generation: 7,
            profile_id: "another".into(),
            entry_id: e.id.clone(),
        },
        query: ott::guide::Query::CurrentNext { now: 250 },
    };
    assert!(super::super::guide_with(&p, &e, &request, &mut h).is_err());
    assert!(h.methods.is_empty());
}

#[test]
fn late_api_guide_keeps_context_and_cancelled_guide_never_escapes() {
    let p = profile();
    let c = Config::parse(&p.source).unwrap();
    let mut e = codec::catalog(&p, &c, &json!([{"id":7,"name":"News"}]))
        .unwrap()
        .entries
        .remove(0);
    // A stale presentation ID must not choose a different channel's guide.
    e.provider_id = Some("999".into());
    let request = ott::guide::Request {
        context: ott::guide::Context {
            generation: 7,
            profile_id: p.id.clone(),
            entry_id: e.id.clone(),
        },
        query: ott::guide::Query::CurrentNext { now: 250 },
    };
    let replies = vec![
        response(json!(true)),
        response(json!([{"start":200,"end":300,"name":"Current"}])),
    ];
    let mut h = http(replies.clone());
    let result = super::super::guide_with(&p, &e, &request, &mut h).unwrap();
    assert_eq!(h.bodies[1]["params"]["ch_id"], "7");
    let mut next = request.clone();
    next.context.generation += 1;
    assert!(!result.can_publish(&next, false));
    let mut h = http(replies);
    h.cancel_after = Some(2);
    assert!(super::super::guide_with(&p, &e, &request, &mut h).is_err());
}
