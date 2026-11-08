//! Durable adapter for the shared portable Favorites protocol.
use super::*;
use plx_data::ott::favorites_wire::{self as wire, Context, DurableStatus, Fx, Outcome, Resolution};

pub(super) struct CommitResult {
    invocation: Invocation,
    operation: String,
    state: Option<Saved>,
    outcome: Outcome,
    deadline: u64,
}

// Test-only construction of an existing typed completion; the actual finish
// handler and publication logic remain unchanged.
#[cfg(all(test, feature = "hostsim"))]
impl CommitResult {
    pub(super) fn from_parts_for_test(invocation: Invocation, operation: String,
        state: Option<Saved>, outcome: Outcome, deadline: u64) -> Self {
        Self { invocation, operation, state, outcome, deadline }
    }
}

/// `private::save_guarded` refuses before rename with Err; only Uncertain may have
/// published the after-image. A failed read-back is equally inconclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WriteFailure {
    NotApplied,
    /// A storage refusal before publication retains its typed diagnostic.
    NotAppliedStorage(private::Error),
    Unknown,
}

pub(super) fn preflight(state: &Saved) -> Result<(), &'static str> {
    state.validate()?;
    let value = serde_json::to_value(state).map_err(|_| "Invalid native state")?;
    private::validate_value(Namespace::OttState, &value).map_err(|_| "Native state exceeds storage capacity")
}

pub(super) fn write_verified(state: &Saved, permit: &private::WritePermit) -> Result<(), WriteFailure> {
    write_verified_with(
        state,
        |value| private::save_guarded(Namespace::OttState, value, permit),
        || private::load(Namespace::OttState),
    )
}

fn write_verified_with(
    state: &Saved,
    save: impl FnOnce(&Value) -> Result<CommitReceipt, private::Error>,
    load: impl FnOnce() -> Result<Option<Value>, private::Error>,
) -> Result<(), WriteFailure> {
    preflight(state).map_err(|_|WriteFailure::NotApplied)?;
    let value=serde_json::to_value(state).map_err(|_|WriteFailure::NotApplied)?;
    match save(&value).map_err(WriteFailure::NotAppliedStorage)? {
        CommitReceipt::Durable=>{}, _=>return Err(WriteFailure::Unknown),
    }
    if load().map_err(|_|WriteFailure::Unknown)?.as_ref()!=Some(&value) {
        return Err(WriteFailure::Unknown);
    }
    Ok(())
}

/// Read-back classification says which image is visible, not whether fsync
/// completed. A caller must still retain its uncertainty fence without Durable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Readback { Before, After, Other }

pub(super) fn classify_readback(before:&Saved,after:&Saved,found:Option<&Value>)->Result<Readback,&'static str> {
    let after=serde_json::to_value(after).map_err(|_|"Invalid settings after-image")?;
    if found==Some(&after) {return Ok(Readback::After);}
    let before=serde_json::to_value(before).map_err(|_|"Invalid settings before-image")?;
    Ok(if found==Some(&before) {Readback::Before} else {Readback::Other})
}

/// An interrupted write has both hashes in its durable intent. Reconcile only
/// an exact old/new document; a different document stays blocked for inspection.
pub(super) fn recover(state:Saved,permit:&private::WritePermit)->Result<Saved,&'static str> {
    recover_with(state,&mut |value|write_verified(value,permit))
}

fn recover_with(state:Saved,persist:&mut impl FnMut(&Saved)->Result<(),WriteFailure>)->Result<Saved,&'static str> {
    let mut settled=state.clone();
    if !settle_receipts(&mut settled)? {return Ok(state);}
    // A recovery write can also be uncertain. Keep the loaded document usable,
    // but retain its unresolved receipt so no writer can overwrite it.
    if persist(&settled).is_err() {return Ok(state);}
    Ok(settled)
}

fn settle_receipts(state:&mut Saved)->Result<bool,&'static str> {
    let revision=wire::revision(&state.favorites)?;
    let mut changed=false;
    for status in &mut state.receipts {
        if status.outcome!=Outcome::Unknown {continue;}
        let Some(receipt)=status.receipt.as_mut() else {continue;};
        // An incomplete/mismatched intent cannot prove which operation wrote
        // the document, even when one of its hashes happens to match.
        if status.operation_id!=receipt.operation_id
            || status.operation_runtime.as_ref()!=Some(&receipt.operation_runtime)
            || status.transaction_id.as_ref()!=Some(&receipt.transaction_id)
            || receipt.outcome!=Outcome::Unknown
            || receipt.new_revision!=receipt.result_document_sha256 {continue;}
        let outcome=if receipt.new_revision==revision {Outcome::Verified}
            else if receipt.expected_revision==revision {Outcome::NotApplied} else {continue;};
        status.outcome=outcome;status.state=if outcome==Outcome::Verified {"committed"} else {"aborted"}.into();
        receipt.outcome=outcome;
        if outcome==Outcome::NotApplied {status.receipt=None;}
        changed=true;
    }
    Ok(changed)
}

/// The same sequencing runs with real storage and an injected in-memory store.
/// Once intent publication is uncertain, never report a clean pre-write refusal.
/// The caller supplies one fixed monotonic deadline; wall-clock changes cannot renew it.
fn commit_with(
    pending:Saved, applied:Saved,
    mut still_valid:impl FnMut()->bool,
    mut persist:impl FnMut(&Saved)->Result<(),WriteFailure>,
)->(Option<Saved>,Outcome) {
    if !still_valid() {return (None,Outcome::NotApplied);}
    match persist(&pending) {
        Err(WriteFailure::NotApplied | WriteFailure::NotAppliedStorage(_))=>return (None,Outcome::NotApplied),
        Err(WriteFailure::Unknown)=>return (Some(pending),Outcome::Unknown),
        Ok(())=>{},
    }
    if still_valid() {
        match persist(&applied) {
            Ok(())=>return (Some(applied),Outcome::Verified),
            Err(WriteFailure::Unknown)=>return (Some(pending),Outcome::Unknown),
            Err(WriteFailure::NotApplied | WriteFailure::NotAppliedStorage(_))=>{},
        }
    }
    let mut aborted=pending.clone();
    if let Some(status)=aborted.receipts.last_mut() {
        status.outcome=Outcome::NotApplied;status.state="aborted".into();status.receipt=None;
    }
    if persist(&aborted).is_ok() {return (Some(aborted),Outcome::NotApplied);}
    (Some(pending),Outcome::Unknown)
}

pub(super) fn resolution_context(profiles:&ProviderProfiles,settings:&BTreeMap<String,Value>,catalog_revision:u64)->Result<String,&'static str> {
    // Source locations/credentials and subprofile mappings affect rebinding;
    // expose only their hash. Catalog revision avoids rehashing every media row.
    serde_json::to_vec(&("ott-favorites-resolution-v1",profiles,settings,catalog_revision))
        .map(|bytes|wire::hash(&bytes)).map_err(|_|"Invalid profile context")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NonceError { NotReady, Exhausted }

pub(super) fn nonce(runtime:&mut Runtime)->Result<String,NonceError> {
    let mut input=runtime.entropy.ok_or(NonceError::NotReady)?.to_vec();
    runtime.uuid_counter=runtime.uuid_counter.checked_add(1).ok_or(NonceError::Exhausted)?;
    input.extend_from_slice(&runtime.uuid_counter.to_be_bytes());
    let digest=plx_base::sha256::sha256(&input);
    let mut bytes:[u8;16]=digest[..16].try_into().map_err(|_|NonceError::Exhausted)?;
    bytes[6]=(bytes[6]&15)|64;bytes[8]=(bytes[8]&63)|128;
    let hex:String=bytes.iter().map(|b|format!("{b:02x}")).collect();
    Ok(format!("{}-{}-{}-{}-{}",&hex[..8],&hex[8..12],&hex[12..16],&hex[16..20],&hex[20..]))
}

fn uuid(runtime:&mut Runtime)->Result<String,&'static str> {
    nonce(runtime).map_err(|error|match error {
        NonceError::NotReady=>"Operation identities are not ready",
        NonceError::Exhausted=>"Operation identity limit reached",
    })
}

pub(super) fn command(runtime:&mut Runtime, invocation:&Invocation, now:u64)->Option<Value> {
    // Resolve against the current runtime while retaining the single protocol owner.
    let mut protocol = std::mem::take(&mut runtime.favorites);
    let result=(|| {
        let ids=[uuid(runtime)?,uuid(runtime)?];
        let revision=wire::revision(&runtime.saved.favorites)?;
        let context=resolution_context(&runtime.saved.profiles,&runtime.saved.settings,runtime.catalog_revision)?;
        let profiles=&runtime.saved.profiles;
        let active=profiles.active_profile.as_ref();
        let resolve=|favorite:&Favorite| match selection::resolve_profile(profiles,favorite) {
            Ok(profile)=>if runtime.favorite_permit(profile, favorite).is_err() { Resolution::Missing } else if active==Some(&profile.id) {
                if runtime.catalog.entries.iter().any(|entry|selection::legacy_id_collision(favorite,entry)) {
                    return Resolution::Ambiguous;
                }
                let mut matches=runtime.catalog.entries.iter().filter(|entry|selection::matches_entry(favorite,entry));
                match (matches.next(),matches.next()) {
                    (Some(_),None)=>Resolution::Resolved,
                    (Some(_),Some(_))=>Resolution::Ambiguous,
                    _=>Resolution::Missing,
                }
            } else {Resolution::Missing},
            Err(selection::BindingError::AmbiguousSource)=>Resolution::Ambiguous,
            Err(selection::BindingError::UnsupportedSource) if favorite.origin.provider == "m3u"
                && matches!(favorite.locator,plx_data::ott::favorites::Locator::VPortal {..}) => {
                match runtime.attachment_for(favorite) {
                    Ok(Some(receipt)) if runtime.m3u_attachments.is_current(&receipt) => {
                        let preview=runtime.favorite_preview.as_ref().is_some_and(|(id,entry)|
                            *id==format!("favorite:{}",favorite.identity())
                                && runtime.metadata_attachment.as_ref()==Some(&receipt)
                                && selection::matches_entry(favorite,entry));
                        if preview {Resolution::Resolved} else {Resolution::Missing}
                    },
                    _=>Resolution::Missing,
                }
            },
            Err(selection::BindingError::UnsupportedProvider|selection::BindingError::UnsupportedSource)=>Resolution::Unsupported,
            Err(_)=>Resolution::Missing,
        };
        let cx=Context {runtime:runtime.remote.runtime(),now_ms:now,wall_now_ms:runtime.wall_clock_ms,revision:&revision,context_revision:&context,
            document:&runtime.saved.favorites,authorized:true,write_ready:runtime.loaded&&!runtime.saving&&!runtime.persistence_uncertain,
            write_blocker:if runtime.saving {Some("busy")} else if !runtime.loaded {Some("unsupported_persistence")}
                else if runtime.persistence_uncertain {Some("reconciliation_required")} else {None},
            fresh_ids:[&ids[0],&ids[1]],resolve:&resolve,durable_status:&runtime.saved.receipts};
        Ok(protocol.execute(&invocation.params,invocation.params.to_string().len()+160,invocation.deadline_ms,&cx))
    })();
    runtime.favorites = protocol;
    match result {
        Err(error)=>Some(rejected(error)),
        Ok(Fx::Reply(data))=>Some(if data.get("error").is_some() {json!({"status":"rejected","data":data})} else {ok(data)}),
        Ok(Fx::Commit(intent))=>{
            let current=runtime.saved.clone();
            // A Favorites transaction changes only the document/receipts. Its
            // applied settings image is identical, so retain these exact pins.
            let pins=match super::super::ott_kiosk_stored::read(&current.settings) {
                Ok(pin)=>pin.map_or_else(Vec::new,|pin|pin.pins().to_vec()),
                Err(())=>return Some(json!({"status":"rejected","data":runtime.favorites.finish_commit(&intent.receipt.operation_id,Outcome::NotApplied)})),
            };
            let tx=runtime.tx.clone();let invocation=invocation.clone();
            let write_permit=runtime.write_permit.clone();
            let operation=intent.receipt.operation_id.clone();
            let admission_operation=operation.clone();
            let deadline=runtime.clock_sample.map(|sample|sample.deadline_at_unix_ms(intent.deadline_ms));
            // Keep the protocol in applying state until the durable result arrives.
            let started=plx_base::task::spawn_off_frame("ott-favorites",move |worker| {
                let mut pending=current;
                pending.receipts.retain(|r|r.operation_id!=intent.receipt.operation_id);
                while pending.receipts.len()>=32 {pending.receipts.remove(0);}
                let mut status=DurableStatus::pending(&intent);let mut receipt=intent.receipt.clone();
                receipt.outcome=Outcome::Unknown;status.receipt=Some(receipt);pending.receipts.push(status);
                let mut applied=pending.clone();applied.favorites=intent.document;
                applied.local_favorite_proofs.prune_with_pins(&applied.favorites,&pins);
                if let Some(last)=applied.receipts.last_mut() {*last=DurableStatus::verified(intent.receipt);}
                // Allocate/preflight the complete after-image before writing a pending journal.
                let identities_ready = favorite_item_ids::prepare(&mut applied, worker)
                    .and_then(|_| preflight(&applied));
                // A worker panic must still release the in-flight command with
                // an unresolved fence; it must not leave saving=true forever.
                let fallback=pending.clone();
                let (state,outcome)=std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    if identities_ready.is_err() { return (None, Outcome::NotApplied); }
                    commit_with(pending,applied,
                        ||write_permit.is_valid()&&deadline.is_some_and(|deadline|!deadline.expired(worker)),
                        |value|write_verified(value,&write_permit))
                })).unwrap_or((Some(fallback),Outcome::Unknown));
                let _=tx.send(Job::FavoriteCommit(CommitResult {invocation,operation,state,outcome,deadline:intent.deadline_ms}));
                plx_machine::idle::invalidate();
            });
            if started {runtime.saving=true;None}
            else {Some(json!({"status":"rejected","data":runtime.favorites.finish_commit(&admission_operation,Outcome::NotApplied)}))}
        }
    }
}

pub(super) fn finish(runtime:&mut Runtime,result:CommitResult) {
    runtime.saving=false;
    if result.outcome==Outcome::Unknown {runtime.retire_profile_edits();}
    if let Some(state)=result.state {
        runtime.observe_profile_edits(&state);
        runtime.saved=state;
        if result.outcome==Outcome::Unknown {runtime.m3u_attachments.invalidate_all();}
        else {runtime.observe_m3u_attachments();}
    }
    let mut data=runtime.favorites.finish_commit(&result.operation,result.outcome);
    if runtime.now_ms()>=result.deadline {
        // Persist the actual outcome for status/restart, but do not acknowledge
        // success after this command's admission has expired.
        data=json!({"op":"commit","runtime":result.invocation.params["runtime"],"subject":"global","version":1,"error":"expired"});
    }
    runtime.remote.complete(result.invocation,if data.get("error").is_none() && result.outcome==Outcome::Verified {ok(data)} else {json!({"status":"rejected","data":data})});
    runtime.changed();
}

#[cfg(test)]
mod tests {
    use super::*;
    use plx_data::ott::favorites::{Locator, Origin as FavoriteOrigin};
    fn pending(expected:&str,new:&str)->DurableStatus {
        let receipt=wire::Receipt {changed:true,context_revision:"0".repeat(64),counts:wire::Counts::default(),
            expected_revision:expected.into(),input_document_sha256:new.into(),new_revision:new.into(),
            operation_id:"op".into(),operation_runtime:"runtime".into(),outcome:Outcome::Unknown,
            persistence:"persistent".into(),plan_digest:"0".repeat(64),required_kinds:Vec::new(),
            result_document_sha256:new.into(),subject:"global".into(),transaction_id:"transaction".into(),version:1};
        DurableStatus {operation_id:"op".into(),operation_runtime:Some("runtime".into()),transaction_id:Some("transaction".into()),
            outcome:Outcome::Unknown,state:"unknown".into(),receipt:Some(receipt)}
    }
    #[test] fn restart_reconciles_only_exact_before_or_after_document() {
        let mut state=Saved::default();let revision=wire::revision(&state.favorites).unwrap();
        state.receipts.push(pending(&revision,&"f".repeat(64)));
        assert!(settle_receipts(&mut state).unwrap());
        assert_eq!(state.receipts[0].outcome,Outcome::NotApplied);
        assert!(state.receipts[0].receipt.is_none());
        state.receipts[0]=pending(&"f".repeat(64),&revision);
        assert!(settle_receipts(&mut state).unwrap());assert_eq!(state.receipts[0].outcome,Outcome::Verified);
        state.receipts[0]=pending(&"e".repeat(64),&"f".repeat(64));
        assert!(!settle_receipts(&mut state).unwrap());assert_eq!(state.receipts[0].outcome,Outcome::Unknown);
    }
    fn images()->(Saved,Saved) {
        let mut before=Saved::default();
        before.settings.insert("test-setting".into(),json!("preserve"));
        before.category="selected".into();before.favorites_selected=true;
        let mut after=before.clone();
        after.favorites.toggle(Favorite {title:"Film".into(),origin:FavoriteOrigin {
            provider:"local".into(),profile:"Home".into(),subprofile:None,source_alias:"Movies".into()},
            locator:Locator::Local {root_alias:"Movies".into(),relative_path:"Film.mp4".into()}}).unwrap();
        let status=pending(&wire::revision(&before.favorites).unwrap(),&wire::revision(&after.favorites).unwrap());
        let mut receipt=status.receipt.clone().unwrap();receipt.outcome=Outcome::Verified;
        before.receipts.push(status);after.receipts.push(DurableStatus::verified(receipt));
        (before,after)
    }
    #[test] fn expired_command_does_not_touch_storage() {
        let (before,after)=images();
        let (saved,outcome)=commit_with(before,after,||false,|_|panic!("write after expiry"));
        assert!(saved.is_none());assert_eq!(outcome,Outcome::NotApplied);
    }
    #[test] fn expiry_and_failed_abort_write_keeps_the_unknown_fence() {
        let (before,after)=images();let mut validity=[true,false].into_iter();let mut writes=0;
        let (saved,outcome)=commit_with(before.clone(),after,||validity.next().unwrap(),|candidate| {
            writes+=1;
            if writes==1 {assert_eq!(candidate.receipts[0].outcome,Outcome::Unknown);Ok(())}
            else {assert_eq!(candidate.receipts[0].outcome,Outcome::NotApplied);Err(WriteFailure::Unknown)}
        });
        assert_eq!(writes,2);assert_eq!(outcome,Outcome::Unknown);
        assert_eq!(serde_json::to_value(saved.unwrap()).unwrap(),serde_json::to_value(before).unwrap());
    }
    #[test] fn expiry_during_intent_write_does_not_publish_the_document() {
        let (before,after)=images();
        let valid=std::cell::Cell::new(true);let mut writes=0;
        let (saved,outcome)=commit_with(before.clone(),after,||valid.get(),|candidate| {
            writes+=1;
            assert_eq!(candidate.favorites,before.favorites);
            if writes==1 {
                assert_eq!(candidate.receipts[0].outcome,Outcome::Unknown);
                valid.set(false);
            } else {
                assert_eq!(candidate.receipts[0].outcome,Outcome::NotApplied);
            }
            Ok(())
        });
        assert_eq!(writes,2);assert_eq!(outcome,Outcome::NotApplied);
        assert_eq!(saved.unwrap().favorites,before.favorites);
    }
    #[test] fn no_op_expiry_records_aborted_not_verified() {
        let (mut before,_)=images();let revision=wire::revision(&before.favorites).unwrap();
        before.receipts=vec![pending(&revision,&revision)];
        let mut after=before.clone();let mut receipt=after.receipts[0].receipt.clone().unwrap();
        receipt.outcome=Outcome::Verified;after.receipts[0]=DurableStatus::verified(receipt);
        let mut validity=[true,false].into_iter();
        let (saved,outcome)=commit_with(before,after,||validity.next().unwrap(),|_|Ok(()));
        assert_eq!(outcome,Outcome::NotApplied);
        let status=&saved.unwrap().receipts[0];
        assert_eq!(status.outcome,Outcome::NotApplied);assert_eq!(status.state,"aborted");assert!(status.receipt.is_none());
    }
    #[test] fn pre_rename_refusal_is_not_an_unknown_operation() {
        let (before,after)=images();let mut writes=0;
        let (saved,outcome)=commit_with(before,after,||true,|_| {writes+=1;Err(WriteFailure::NotApplied)});
        assert_eq!(writes,1);assert!(saved.is_none());assert_eq!(outcome,Outcome::NotApplied);
    }
    #[test] fn uncertain_document_write_is_not_retried_or_rolled_back() {
        let (before,after)=images();let mut writes=Vec::new();
        let (saved,outcome)=commit_with(before.clone(),after.clone(),||true,|state| {
            writes.push(serde_json::to_value(state).unwrap());
            if writes.len()==2 {Err(WriteFailure::Unknown)} else {Ok(())}
        });
        assert_eq!(writes.len(),2);assert_eq!(outcome,Outcome::Unknown);
        assert_eq!(writes[1],serde_json::to_value(&after).unwrap());
        assert_eq!(serde_json::to_value(saved.unwrap()).unwrap(),serde_json::to_value(before).unwrap());
        let recovered=recover_with(after,&mut |_|panic!("completed record must not be replayed")).unwrap();
        assert_eq!(recovered.receipts[0].outcome,Outcome::Verified);
    }
    #[test] fn refused_document_write_is_settled_before_unlocking() {
        let (before,after)=images();let mut writes=0;
        let (saved,outcome)=commit_with(before.clone(),after,||true,|_| {
            writes+=1;if writes==2 {Err(WriteFailure::NotApplied)} else {Ok(())}
        });
        assert_eq!(writes,3);assert_eq!(outcome,Outcome::NotApplied);
        let saved=saved.unwrap();assert_eq!(saved.favorites,before.favorites);
        assert_eq!(saved.receipts[0].outcome,Outcome::NotApplied);
    }
    #[test] fn successful_commit_preserves_other_settings_and_selection() {
        let (before,after)=images();let mut writes=0;
        let (saved,outcome)=commit_with(before,after.clone(),||true,|_|{writes+=1;Ok(())});
        assert_eq!(writes,2);assert_eq!(outcome,Outcome::Verified);
        assert_eq!(serde_json::to_value(saved.unwrap()).unwrap(),serde_json::to_value(after).unwrap());
    }
    #[test] fn failed_recovery_keeps_readable_state_with_a_write_fence() {
        let (before,_)=images();
        let recovered=recover_with(before.clone(),&mut |_|Err(WriteFailure::Unknown)).unwrap();
        assert_eq!(serde_json::to_value(recovered).unwrap(),serde_json::to_value(before).unwrap());
    }
    #[test] fn recovery_never_attests_a_mismatched_receipt() {
        let (mut before,_)=images();before.receipts[0].operation_id="another-operation".into();
        assert!(!settle_receipts(&mut before).unwrap());assert_eq!(before.receipts[0].outcome,Outcome::Unknown);
    }
    #[test] fn readback_compares_the_whole_saved_state() {
        let (before,after)=images();
        assert_eq!(classify_readback(&before,&after,Some(&serde_json::to_value(&after).unwrap())),Ok(Readback::After));
        assert_eq!(classify_readback(&before,&after,Some(&serde_json::to_value(&before).unwrap())),Ok(Readback::Before));
        let mut other=after.clone();other.category="changed".into();
        assert_eq!(classify_readback(&before,&after,Some(&serde_json::to_value(other).unwrap())),Ok(Readback::Other));
        assert_eq!(classify_readback(&before,&after,None),Ok(Readback::Other));
    }
    #[test] fn context_tracks_catalog_and_private_subprofile_configuration() {
        let mut state=Saved::default();
        let original=resolution_context(&state.profiles,&state.settings,0).unwrap();
        assert_ne!(original,resolution_context(&state.profiles,&state.settings,1).unwrap());
        state.settings.insert("profileSources".into(),json!({"Home":{"VPortal":"private-source"}}));
        let configured=resolution_context(&state.profiles,&state.settings,0).unwrap();
        assert_ne!(original,configured);assert_eq!(configured.len(),64);assert!(!configured.contains("private-source"));
        state.profiles.active_profile=Some("another-profile".into());
        assert_ne!(configured,resolution_context(&state.profiles,&state.settings,0).unwrap());
    }


    fn diagnostic_storage_error() -> private::Error {
        private::Error::Store(plx_platform::storage::StoreError::Io {
            stage: plx_platform::storage::CommitStage::CreateTemp,
            errno: 13,
        })
    }

    #[test]
    fn verified_write_preserves_prepublication_storage_errors() {
        let state = Saved::default();
        for error in [
            diagnostic_storage_error(),
            private::Error::ProtectionUnavailable,
            private::Error::Invalid,
            private::Error::TooLarge,
            private::Error::Revoked,
        ] {
            let result = write_verified_with(
                &state,
                |value| {
                    assert_eq!(value, &serde_json::to_value(&state).unwrap());
                    Err(error)
                },
                || panic!("a refused write must not read back"),
            );
            assert_eq!(result, Err(WriteFailure::NotAppliedStorage(error)));
        }
    }

    #[test]
    fn verified_write_keeps_uncertain_and_readback_failures_unknown() {
        let state = Saved::default();
        assert_eq!(
            write_verified_with(
                &state,
                |_| Ok(CommitReceipt::Uncertain {
                    stage: plx_platform::storage::CommitStage::ParentSync,
                    errno: 5,
                }),
                || panic!("readback cannot attest an uncertain commit"),
            ),
            Err(WriteFailure::Unknown),
        );
        for found in [
            Err(diagnostic_storage_error()),
            Ok(None),
            Ok(Some(json!({"version": 99}))),
        ] {
            assert_eq!(
                write_verified_with(&state, |_| Ok(CommitReceipt::Durable), || found),
                Err(WriteFailure::Unknown),
            );
        }
        assert_eq!(
            write_verified_with(
                &state,
                |_| Ok(CommitReceipt::Durable),
                || Ok(Some(serde_json::to_value(&state).unwrap())),
            ),
            Ok(()),
        );
    }

    #[test]
    fn verified_write_preflight_refusal_never_reaches_storage() {
        let mut state = Saved::default();
        state.version = 0;
        assert_eq!(
            write_verified_with(
                &state,
                |_| panic!("invalid state must not be written"),
                || panic!("invalid state must not be read back"),
            ),
            Err(WriteFailure::NotApplied),
        );
    }

    #[test]
    fn typed_intent_refusal_never_publishes_pending() {
        let (before, after) = images();
        let mut writes = 0;
        let (saved, outcome) = commit_with(before, after, || true, |_| {
            writes += 1;
            Err(WriteFailure::NotAppliedStorage(diagnostic_storage_error()))
        });
        assert_eq!(writes, 1);
        assert!(saved.is_none());
        assert_eq!(outcome, Outcome::NotApplied);
    }

    #[test]
    fn typed_document_refusal_requires_a_durable_abort() {
        for refuse_abort in [false, true] {
            let (before, after) = images();
            let mut writes = 0;
            let (saved, outcome) = commit_with(before.clone(), after.clone(), || true, |candidate| {
                writes += 1;
                match writes {
                    1 => {
                        assert_eq!(candidate.receipts[0].outcome, Outcome::Unknown);
                        Ok(())
                    }
                    2 => {
                        assert_eq!(candidate.favorites, after.favorites);
                        Err(WriteFailure::NotAppliedStorage(diagnostic_storage_error()))
                    }
                    3 => {
                        assert_eq!(candidate.favorites, before.favorites);
                        assert_eq!(candidate.receipts[0].outcome, Outcome::NotApplied);
                        assert!(candidate.receipts[0].receipt.is_none());
                        if refuse_abort {
                            Err(WriteFailure::NotAppliedStorage(diagnostic_storage_error()))
                        } else {
                            Ok(())
                        }
                    }
                    _ => panic!("unexpected retry"),
                }
            });
            assert_eq!(writes, 3);
            let saved = saved.unwrap();
            if refuse_abort {
                assert_eq!(outcome, Outcome::Unknown);
                assert_eq!(serde_json::to_value(saved).unwrap(), serde_json::to_value(before).unwrap());
            } else {
                assert_eq!(outcome, Outcome::NotApplied);
                let mut expected = before;
                expected.receipts[0].outcome = Outcome::NotApplied;
                expected.receipts[0].state = "aborted".into();
                expected.receipts[0].receipt = None;
                assert_eq!(serde_json::to_value(saved).unwrap(), serde_json::to_value(expected).unwrap());
            }
        }
    }

    #[test]
    fn typed_recovery_refusal_keeps_original_unknown_receipt() {
        let (before, _) = images();
        let mut writes = 0;
        let recovered = recover_with(before.clone(), &mut |candidate| {
            writes += 1;
            assert_eq!(candidate.receipts[0].outcome, Outcome::NotApplied);
            Err(WriteFailure::NotAppliedStorage(diagnostic_storage_error()))
        }).unwrap();
        assert_eq!(writes, 1);
        assert_eq!(recovered.receipts[0].outcome, Outcome::Unknown);
        assert_eq!(serde_json::to_value(recovered).unwrap(), serde_json::to_value(before).unwrap());
    }
}
