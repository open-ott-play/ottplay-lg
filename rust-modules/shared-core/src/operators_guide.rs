//! Replay the canonical guide state machine with bounded, request-scoped response data.
use super::{with_planner, CatalogProfile, Error, INPUT_BYTES, OUTPUT_BYTES};
use rquickjs::Function;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
#[derive(Clone, Serialize)]
pub struct GuideReply {
    pub phase: String,
    pub data: Value,
    pub failed: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuidePlan {
    pub phase: Option<String>,
    pub result: Value,
}
pub fn guide_plan(
    profile: CatalogProfile,
    current: bool,
    replies: &[GuideReply],
    archive_hours: u64,
) -> Result<GuidePlan, Error> {
    if replies.len() > 4 || archive_hours > 8760 {
        return Err(Error::InvalidInput);
    }
    let input = serde_json::to_vec(&json!([profile, current, replies, archive_hours]))
        .map_err(|_| Error::InvalidInput)?;
    if input.len() > INPUT_BYTES {
        return Err(Error::InvalidInput);
    }
    let output = with_planner(|planner| {
        planner.context.with(|ctx| {
        let invoke=||->rquickjs::Result<String>{
            let adapter:Function=ctx.eval("(function(a){var c=new OttPlayCore.OperatorGuideClient(a[0],a[1]);a[2].forEach(function(r){if(!c.phase()||c.phase()!==r.phase)throw new Error('invalid guide phase');if(!r.failed)c.accept(r.data,r.phase,a[0]==='itv'?0:a[3]);c.complete();});var value=c.result();return JSON.stringify({phase:c.phase()||null,result:value===undefined?null:value});})")?;
            adapter.call((ctx.json_parse(input)?,))
        };
        invoke().map_err(|_|{let _=ctx.catch();Error::InvalidInput})
    })
    })?;
    if output.len() > OUTPUT_BYTES {
        return Err(Error::InvalidResult);
    }
    serde_json::from_str(&output).map_err(|_| Error::InvalidResult)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shura_week_then_archive_replays_state_without_losing_first_response() {
        let first = guide_plan(CatalogProfile::Shura, false, &[], 24).unwrap();
        assert_eq!(first.phase.as_deref(), Some("week"));
        let replies = vec![GuideReply {
            phase: "week".into(),
            failed: false,
            data: json!([{"start_time":100,"duration":100,"name":"Current","text":"Full synopsis"}]),
        }];
        let second = guide_plan(CatalogProfile::Shura, false, &replies, 24).unwrap();
        assert_eq!(second.phase.as_deref(), Some("archive"));
        let mut replies = replies;
        replies.push(GuideReply {
            phase: "archive".into(),
            failed: false,
            data: json!([{"start_time":0,"duration":100,"name":"Earlier"}]),
        });
        let final_plan = guide_plan(CatalogProfile::Shura, false, &replies, 24).unwrap();
        assert!(final_plan.phase.is_none());
        assert_eq!(final_plan.result.as_array().unwrap().len(), 2);
        assert_eq!(final_plan.result[1]["name"], "Current");
        assert_eq!(
            guide_plan(CatalogProfile::Shura, false, &[], 24)
                .unwrap()
                .result,
            Value::Null
        );
    }
    #[test]
    fn guide_replay_rejects_wrong_phase_and_maps_itv_response() {
        let wrong = GuideReply {
            phase: "archive".into(),
            failed: false,
            data: json!([]),
        };
        assert!(guide_plan(CatalogProfile::Itv, false, &[wrong], 0).is_err());
        let reply = GuideReply {
            phase: "all".into(),
            failed: false,
            data: json!({"res":[{"startTime":100,"stopTime":200,"title":"Fixture","desc":"Description"}]}),
        };
        let plan = guide_plan(CatalogProfile::Itv, false, &[reply], 0).unwrap();
        assert!(plan.phase.is_none());
        assert_eq!(plan.result[0]["time"], 100);
        assert_eq!(plan.result[0]["descr"], "Description");
    }
}
