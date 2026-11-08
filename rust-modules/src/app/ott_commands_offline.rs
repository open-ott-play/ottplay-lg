//! Portable offline profiles describe media, never disclose or grant local paths.
use super::*;

fn document(saved: &Saved, n: usize) -> Option<&Value> {
    saved.settings.get(&format!("offlineProfile:{n}"))
}
fn binding<'a>(saved: &'a Saved, doc: &Value) -> Option<&'a str> {
    let id = doc["binding_id"].as_str()?;
    let binding = saved.settings.get(&format!("offlineBinding:{id}"))?;
    if binding["sources"] != doc["sources"] {
        return None;
    }
    binding["root"].as_str()
}
// Match the shared portable profile grammar: a leading URI scheme is not a
// file name, but a colon inside a name or a nested relative path is allowed.
fn has_uri_scheme(value: &str) -> bool {
    let Some((prefix, _)) = value.split_once(':') else {
        return false;
    };
    let mut bytes = prefix.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_alphabetic())
        && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'.' | b'-'))
}
fn portable_name(value: &Value) -> bool {
    text(value, 512, false)
        .is_some_and(|s| !s.contains(['/', '\\']) && s != "." && s != ".." && !has_uri_scheme(s))
}
fn relative(value: &Value) -> bool {
    text(value, 1024, false).is_some_and(|s| {
        !s.starts_with('/')
            && !s.contains('\\')
            && !has_uri_scheme(s)
            && !s.split('/').any(|p| p.is_empty() || p == "." || p == "..")
    })
}
fn valid_document(doc: &Value) -> bool {
    if !fields(
        doc,
        &[
            "version",
            "provider",
            "name",
            "binding_id",
            "sources",
            "selected",
        ],
    ) || doc["version"] != 1
        || doc["provider"] != "offline"
        || text(&doc["name"], 256, true).is_none()
        || doc.to_string().len() > 12288
    {
        return false;
    }
    let id = doc["binding_id"].as_str().unwrap_or("").as_bytes();
    if id.len() != 36
        || !id.iter().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                *b == b'-'
            } else {
                b.is_ascii_digit() || (b'a'..=b'f').contains(b)
            }
        })
        || id[14] != b'4'
        || !b"89ab".contains(&id[19])
    {
        return false;
    }
    let Some(sources) = doc["sources"].as_array().filter(|s| s.len() <= 32) else {
        return false;
    };
    if sources.iter().enumerate().any(|(i, s)| {
        !fields(s, &["key", "kind", "name"])
            || s["key"] != format!("s{i}")
            || !matches!(s["kind"].as_str(), Some("folder" | "file"))
            || !portable_name(&s["name"])
    }) {
        return false;
    }
    doc["selected"].is_null()
        || (fields(&doc["selected"], &["name", "source_key"])
            && relative(&doc["selected"]["name"])
            && sources
                .iter()
                .any(|s| s["key"] == doc["selected"]["source_key"]))
}
fn binding_id(request_id: &str) -> Result<String, &'static str> {
    // New RPC IDs carry 128 server-generated random bits. Preserve their entropy;
    // add UUID version/variant bits without asking the frame thread for OS entropy.
    if request_id.len() != 32
        || !request_id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("A request identity is required to save an offline profile");
    }
    let mut id = request_id.as_bytes().to_vec();
    id[12] = b'4';
    let nibble = (id[16] as char)
        .to_digit(16)
        .ok_or("Invalid request identity")?;
    id[16] = b"89ab"[(nibble & 3) as usize];
    let id = String::from_utf8(id).map_err(|_| "Invalid request identity")?;
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &id[..8],
        &id[8..12],
        &id[12..16],
        &id[16..20],
        &id[20..]
    ))
}
fn capture(
    runtime: &Runtime,
    profile: &ProviderProfile,
    inv: &Invocation,
    name: &str,
) -> Result<Value, &'static str> {
    let name_on_disk = if profile.source_alias.is_empty() {
        std::path::Path::new(&profile.source)
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or("The root folder has no portable name")?
    } else {
        &profile.source_alias
    };
    let mut doc = json!({"version":1,"provider":"offline","name":name,"binding_id":binding_id(&inv.id)?,"sources":[{"key":"s0","kind":"folder","name":name_on_disk}],"selected":null});
    if let Some(playing) = runtime
        .playing
        .as_ref()
        .filter(|p| p.profile.id == profile.id)
    {
        if let MediaLocator::Local { relative_path } = &playing.entry.media {
            doc["selected"] = json!({"source_key":"s0","name":relative_path});
        }
    }
    if !valid_document(&doc) {
        return Err("This local profile cannot be represented portably");
    }
    Ok(doc)
}
pub(super) fn metadata(saved: &Saved, n: usize) -> Value {
    let profile = profile_index(saved, ProviderKind::Local, n).map(|i| &saved.profiles.profiles[i]);
    let doc = document(saved, n);
    json!({"number":n,"name":doc.map(|d|d["name"].clone()).unwrap_or_else(||json!(profile.map(|p|p.name.as_str()).unwrap_or(""))),
        "active":profile.is_some_and(|p|Some(&p.id)==saved.profiles.active_profile.as_ref()),"configured":doc.is_some()||profile.is_some(),
        "binding_required":doc.is_some_and(|d|binding(saved,d).is_none()),"source_count":doc.and_then(|d|d["sources"].as_array()).map(|s|s.len()).unwrap_or(usize::from(profile.is_some())),
        "selected_name":doc.and_then(|d|d["selected"]["name"].as_str()).unwrap_or("")})
}
pub(super) fn unbound_rows(saved: &Saved) -> Vec<(usize, String)> {
    (1..=15)
        .filter_map(|n| {
            let doc = document(saved, n)?;
            (binding(saved, doc).is_none())
                .then(|| (n, doc["name"].as_str().unwrap_or("Local videos").to_owned()))
        })
        .collect()
}
pub(super) fn bind(saved: &Saved, number: usize, path: &str) -> Result<Saved, &'static str> {
    if !(1..=15).contains(&number) {
        return Err("Choose a profile number from 1 to 15");
    }
    let doc = document(saved, number)
        .filter(|d| valid_document(d))
        .ok_or("Import this local profile first")?;
    let sources = doc["sources"].as_array().unwrap();
    if sources.len() != 1 || sources[0]["kind"] != "folder" {
        return Err("This native folder editor accepts a single-folder profile");
    }
    let mut candidate = saved.clone();
    let index = insert_profile(
        &mut candidate,
        ProviderKind::Local,
        number,
        path.into(),
        doc["name"].as_str().map(str::to_owned),
    )?;
    candidate.profiles.profiles[index].source_alias = sources[0]["name"].as_str().unwrap().into();
    candidate.settings.insert(
        format!("offlineBinding:{}", doc["binding_id"].as_str().unwrap()),
        json!({"root":path,"sources":sources}),
    );
    Ok(candidate)
}
/// An imported selection is only a fallback for its currently bound profile.
/// This derives a catalogue ID, never a grant or a fabricated playback checkpoint.
pub(super) fn selected_resume(saved: &Saved) -> Option<continuity::ResumePoint> {
    if saved.favorites_selected { return None; }
    let active = saved.profiles.active_profile.as_ref()?;
    let index = saved.profiles.profiles.iter().position(|profile|
        profile.id == *active && profile.enabled && profile.kind == ProviderKind::Local)?;
    let profile = &saved.profiles.profiles[index];
    // HTTP Local profiles use server-library identities, not filesystem rows.
    if !profile.source.starts_with('/') { return None; }
    let number = (1..=15).find(|number|
        profile_index(saved, ProviderKind::Local, *number) == Some(index))?;
    let doc = document(saved, number).filter(|doc| valid_document(doc))?;
    let sources = doc["sources"].as_array()?;
    if sources.len() != 1 || sources[0]["kind"] != "folder"
        || sources[0]["name"].as_str()? != profile.alias()
        || binding(saved, doc)? != profile.source
        || doc["selected"]["source_key"] != "s0" { return None; }
    let catalog = plx_data::ott::catalog_from_local_files(&[plx_data::ott::LocalFile {
        relative_path: doc["selected"]["name"].as_str()?.into(),
        size: 0,
        duration_ms: None,
    }], profile).ok()?;
    Some(continuity::ResumePoint {
        entry_id: catalog.entries.into_iter().next()?.id,
        position_ms: 0,
    })
}
pub(super) fn request(runtime: &mut Runtime, inv: &Invocation) -> Option<Value> {
    let p = &inv.params;
    if inv.action == "profiles" {
        return Some(if fields(p, &[]) {
            ok(
                json!({"provider":"offline","profiles":(1..=15).map(|n|metadata(&runtime.saved,n)).collect::<Vec<_>>()}),
            )
        } else {
            rejected("The profiles query accepts no parameters")
        });
    }
    let n = match number(p) {
        Ok(n) => n,
        Err(e) => return Some(rejected(e)),
    };
    let mut saved = runtime.saved.clone();
    match inv.action.as_str() {
        "profile_import" => {
            if !fields(p, &["number", "profile"]) || !valid_document(&p["profile"]) {
                return Some(rejected("Invalid portable local video profile"));
            }
            if profile_index(&saved, ProviderKind::Local, n).is_some_and(|i| {
                Some(&saved.profiles.profiles[i].id) == saved.profiles.active_profile.as_ref()
            }) {
                return Some(rejected(
                    "Import into an inactive local profile, then load it",
                ));
            }
            let doc = p["profile"].clone();
            if let Some(root) = binding(&saved, &doc).map(str::to_owned) {
                let result = insert_profile(
                    &mut saved,
                    ProviderKind::Local,
                    n,
                    root,
                    doc["name"].as_str().map(str::to_owned),
                );
                match result {
                    Ok(index) => {
                        saved.profiles.profiles[index].source_alias =
                            doc["sources"][0]["name"].as_str().unwrap_or("").into()
                    }
                    Err(e) => return Some(rejected(e)),
                };
            } else if let Some(i) = profile_index(&saved, ProviderKind::Local, n) {
                // Importing metadata cannot retain a different folder's authority.
                let removed = saved.profiles.profiles.remove(i);
                saved.settings.remove(&format!("slot:{}", removed.id));
            }
            saved.settings.insert(format!("offlineProfile:{n}"), doc);
        }
        "profile_save" => {
            if !(fields(p, &["number"]) || fields(p, &["number", "name"]))
                || p.get("name").is_some_and(|v| text(v, 256, true).is_none())
            {
                return Some(rejected("Invalid local profile name"));
            }
            let Some(current) = runtime.effective_profile().filter(|p| p.kind == ProviderKind::Local) else {
                return Some(rejected("Select a local folder before saving its profile"));
            };
            let name = p["name"].as_str().unwrap_or(&current.name);
            let doc = match capture(runtime, current, inv, name) {
                Ok(doc) => doc,
                Err(e) => return Some(rejected(e)),
            };
            let result = insert_profile(
                &mut saved,
                ProviderKind::Local,
                n,
                current.source.clone(),
                Some(name.into()),
            );
            match result {
                Ok(index) => {
                    saved.profiles.profiles[index].source_alias =
                        doc["sources"][0]["name"].as_str().unwrap_or("").into()
                }
                Err(e) => return Some(rejected(e)),
            };
            saved.settings.insert(
                format!("offlineBinding:{}", doc["binding_id"].as_str().unwrap()),
                json!({"root":current.source,"sources":doc["sources"]}),
            );
            saved.settings.insert(format!("offlineProfile:{n}"), doc);
        }
        "profile_export" => {
            if !fields(p, &["number"]) {
                return Some(rejected("Profile export accepts only a number"));
            }
            if let Some(doc) = document(&saved, n) {
                return Some(ok(json!({"provider":"offline","number":n,"profile":doc})));
            }
            let Some(index) = profile_index(&saved, ProviderKind::Local, n) else {
                return Some(rejected("Configure this local profile before exporting it"));
            };
            let profile = &saved.profiles.profiles[index];
            let doc = match capture(runtime, profile, inv, &profile.name) {
                Ok(doc) => doc,
                Err(e) => return Some(rejected(e)),
            };
            saved.settings.insert(
                format!("offlineBinding:{}", doc["binding_id"].as_str().unwrap()),
                json!({"root":profile.source,"sources":doc["sources"]}),
            );
            saved
                .settings
                .insert(format!("offlineProfile:{n}"), doc.clone());
            return defer(
                runtime,
                inv,
                saved,
                false,
                ok(json!({"provider":"offline","number":n,"profile":doc})),
            );
        }
        "profile" => {
            if !fields(p, &["number"]) {
                return Some(rejected("Profile load accepts only a number"));
            }
            if document(&saved, n).is_some_and(|doc| binding(&saved, doc).is_none()) {
                return Some(rejected(
                    "Bind this imported profile to a local folder on this installation first",
                ));
            }
            let Some(index) = profile_index(&saved, ProviderKind::Local, n) else {
                return Some(rejected("Configure this local profile before loading it"));
            };
            saved.profiles.active_profile = Some(saved.profiles.profiles[index].id.clone());

        }
        _ => return Some(unsupported("Unsupported local profile operation")),
    }
    let row = metadata(&saved, n);
    let load = inv.action == "profile";
    let mut data =
        json!({"provider":"offline","profile":row,"binding_required":row["binding_required"]});
    data[if load { "dispatched" } else { "saved" }] = json!(true);
    defer(runtime, inv, saved, load, ok(data))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Value {
        json!({"version":1,"provider":"offline","name":"Films","binding_id":"01234567-89ab-4def-8123-456789abcdef","sources":[{"key":"s0","kind":"folder","name":"Films"}],"selected":{"source_key":"s0","name":"Comedy/example.mkv"}})
    }
    #[test]
    fn portable_document_rejects_paths_and_metadata_import_grants_nothing() {
        let valid = fixture();
        assert!(valid_document(&valid));
        assert!(binding(&Saved::default(), &valid).is_none());
        let mut path = valid.clone();
        path["sources"][0]["name"] = json!("/private/Films");
        assert!(!valid_document(&path));
        let mut escape = valid;
        escape["selected"]["name"] = json!("../outside.mkv");
        assert!(!valid_document(&escape));
    }
    #[test]
    fn portable_document_accepts_shared_safe_colon_names() {
        for (name, selected) in [
            ("Season 1: Extras", "Season 1/Part: 2.mp4"),
            ("2026: Films", "2026: Films/Opening.mp4"),
            ("Фильмы: архив", "Сезон: первый/Серия: 1.mp4"),
        ] {
            let mut doc = fixture();
            doc["sources"][0]["name"] = json!(name);
            doc["selected"]["name"] = json!(selected);
            assert!(valid_document(&doc), "shared portable name was rejected");
        }
    }
    #[test]
    fn portable_document_still_rejects_schemes_and_path_escapes() {
        for name in ["file:movie.mp4", "HTTPS:movie.mp4", "x+test.1-a:movie.mp4", "C:movie.mp4",
            "../Movies", "/Movies", "Movies\\Other", ".", ".."] {
            let mut doc = fixture();
            doc["sources"][0]["name"] = json!(name);
            assert!(!valid_document(&doc), "unsafe portable source name was accepted");
        }
        for selected in ["file:movie.mp4", "HTTPS://example.invalid/movie.mp4", "C:movie.mp4",
            "/Movies/Part: 2.mp4", "Movies\\Part: 2.mp4", "Movies/../Part: 2.mp4",
            "Movies/./Part: 2.mp4", "Movies//Part: 2.mp4", "Movies/"] {
            let mut doc = fixture();
            doc["selected"]["name"] = json!(selected);
            assert!(!valid_document(&doc), "unsafe portable selection was accepted");
        }
    }
    #[test]
    fn safe_colon_profile_binding_preserves_names_and_private_root() {
        let mut doc = fixture();
        doc["sources"][0]["name"] = json!("Season 1: Extras");
        doc["selected"]["name"] = json!("Season 1/Part: 2.mp4");
        let mut saved = Saved::default();
        saved.settings.insert("offlineProfile:15".into(), doc.clone());
        let bound = bind(&saved, 15, "/media/usb/ReceiverFolder").unwrap();
        let index = profile_index(&bound, ProviderKind::Local, 15).unwrap();
        assert_eq!(bound.profiles.profiles[index].source_alias, "Season 1: Extras");
        assert_eq!(document(&bound, 15), Some(&doc));
        assert_eq!(binding(&bound, &doc), Some("/media/usb/ReceiverFolder"));
        assert!(!document(&bound, 15).unwrap().to_string().contains("/media/usb"));
        assert!(bound.validate().is_ok());
    }

    #[test]
    fn request_entropy_produces_portable_uuid_and_distinct_bindings() {
        let a = binding_id("0123456789abcdef0123456789abcdef").unwrap();
        let b = binding_id("1123456789abcdef0123456789abcdef").unwrap();
        assert_ne!(a, b);
        let mut doc = fixture();
        doc["binding_id"] = json!(a);
        assert!(valid_document(&doc));
    }
    #[test]
    fn local_binding_is_private_and_does_not_rewrite_portable_document() {
        let mut saved = Saved::default();
        saved.settings.insert("offlineProfile:15".into(), fixture());
        assert_eq!(unbound_rows(&saved), vec![(15, "Films".into())]);
        assert!(bind(&saved, 15, "../other").is_err());
        let bound = bind(&saved, 15, "/media/usb/OtherFolderName").unwrap();
        assert!(unbound_rows(&bound).is_empty());
        assert_eq!(document(&bound, 15), document(&saved, 15));
        assert_eq!(
            binding(&bound, document(&bound, 15).unwrap()),
            Some("/media/usb/OtherFolderName")
        );
        let profile =
            &bound.profiles.profiles[profile_index(&bound, ProviderKind::Local, 15).unwrap()];
        assert_eq!(profile.source_alias, "Films");
        assert!(!document(&bound, 15)
            .unwrap()
            .to_string()
            .contains("/media/usb"));
        assert!(bound.validate().is_ok());
    }

    fn selected_fixture() -> Saved {
        let mut saved = Saved::default();
        saved.settings.insert("offlineProfile:15".into(), fixture());
        let mut saved = bind(&saved, 15, "/media/receiver/RenamedFolder").unwrap();
        let index = profile_index(&saved, ProviderKind::Local, 15).unwrap();
        saved.profiles.active_profile = Some(saved.profiles.profiles[index].id.clone());
        saved
    }

    fn selected_entry(saved: &Saved, path: &str) -> CatalogEntry {
        let profile = &saved.profiles.profiles[profile_index(saved, ProviderKind::Local, 15).unwrap()];
        plx_data::ott::catalog_from_local_files(&[plx_data::ott::LocalFile {
            relative_path: path.into(), size: 42, duration_ms: Some(60_000),
        }], profile).unwrap().entries.remove(0)
    }

    #[test]
    fn portable_selection_uses_receiver_catalog_id_without_mutating_saved_state() {
        let saved = selected_fixture();
        let before = serde_json::to_value(&saved).unwrap();
        let selected = selected_resume(&saved).unwrap();
        assert_eq!(selected.entry_id, selected_entry(&saved, "Comedy/example.mkv").id);
        assert_eq!(selected.position_ms, 0);
        assert_eq!(serde_json::to_value(&saved).unwrap(), before);
        assert!(saved.resume.is_empty(), "portable selection is not a playback checkpoint");
    }

    #[test]
    fn portable_selection_requires_current_slot_and_exact_private_binding() {
        for change in ["inactive", "disabled", "slot", "binding", "binding-root", "source", "http",
            "alias", "binding-sources", "favorites"] {
            let mut saved = selected_fixture();
            let index = profile_index(&saved, ProviderKind::Local, 15).unwrap();
            let binding_key = format!("offlineBinding:{}", fixture()["binding_id"].as_str().unwrap());
            match change {
                "inactive" => saved.profiles.active_profile = None,
                "disabled" => saved.profiles.profiles[index].enabled = false,
                "slot" => { saved.settings.insert(format!("slot:{}", saved.profiles.profiles[index].id), json!(14)); }
                "binding" => { saved.settings.remove(&binding_key); }
                "binding-root" => saved.settings.get_mut(&binding_key).unwrap()["root"] = json!("/media/other"),
                "source" => saved.profiles.profiles[index].source = "/media/other".into(),
                "http" => {
                    saved.profiles.profiles[index].source = "https://media.invalid/library.json".into();
                    saved.settings.get_mut(&binding_key).unwrap()["root"] = json!("https://media.invalid/library.json");
                }
                "alias" => saved.profiles.profiles[index].source_alias = "Other films".into(),
                "binding-sources" => saved.settings.get_mut(&binding_key).unwrap()["sources"][0]["name"] = json!("Other films"),
                "favorites" => saved.favorites_selected = true,
                _ => unreachable!(),
            }
            assert!(selected_resume(&saved).is_none(), "{change}");
        }
    }

    #[test]
    fn portable_selection_ignores_absent_unsupported_or_invalid_media() {
        for change in ["absent", "source-key", "escape", "unsupported", "file", "multiple"] {
            let mut saved = selected_fixture();
            let doc = saved.settings.get_mut("offlineProfile:15").unwrap();
            match change {
                "absent" => doc["selected"] = Value::Null,
                "source-key" => doc["selected"]["source_key"] = json!("s1"),
                "escape" => doc["selected"]["name"] = json!("../outside.mp4"),
                "unsupported" => doc["selected"]["name"] = json!("Comedy/notes.txt"),
                "file" => doc["sources"][0]["kind"] = json!("file"),
                "multiple" => doc["sources"].as_array_mut().unwrap().push(json!({"key":"s1","kind":"folder","name":"More films"})),
                _ => unreachable!(),
            }
            // Match metadata for the file/multiple cases too: a private binding
            // alone must not make unsupported profile shapes restorable.
            let sources = doc["sources"].clone();
            let binding_key = format!("offlineBinding:{}", doc["binding_id"].as_str().unwrap());
            saved.settings.get_mut(&binding_key).unwrap()["sources"] = sources;
            assert!(selected_resume(&saved).is_none(), "{change}");
        }
    }

    #[cfg(feature = "hostsim")]
    mod catalog_restore {
        use super::*;

        fn runtime() -> Runtime {
            let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
            runtime.saved = selected_fixture();
            // Keep the real loader's worker slot occupied: these tests supply
            // catalogue batches to its actual continuity state, without IO.
            runtime.catalog_done.store(false, Ordering::Release);
            runtime
        }

        #[test]
        fn catalog_load_uses_portable_selection_only_after_current_native_checkpoint() {
            let _serial = plx_base::testlock::serial();
            for source in ["portable", "saved", "memory", "stale"] {
                let mut runtime = runtime();
                let profile = runtime.active().unwrap().clone();
                let portable = selected_entry(&runtime.saved, "Comedy/example.mkv").id;
                let native = selected_entry(&runtime.saved, "Drama/newer.mp4").id;
                let point = SavedResume { entry_id: native.clone(), position_ms: 42_000,
                    source_fingerprint: if source == "stale" { "old-source".into() }
                        else { profile_fingerprint(&profile) } };
                match source {
                    "saved" | "stale" => { runtime.saved.resume.insert(profile.id.clone(), point); }
                    "memory" => { runtime.checkpoints.insert(profile.id.clone(), point); }
                    _ => {}
                }
                let before = serde_json::to_value(&runtime.saved).unwrap();
                runtime.load_catalog();
                runtime.validate_bound_resume();
                let point = runtime.continuity.pending_restore().unwrap();
                assert_eq!(point.entry_id, if matches!(source, "saved" | "memory") { native } else { portable });
                assert_eq!(point.position_ms, if matches!(source, "saved" | "memory") { 42_000 } else { 0 });
                assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
            }
        }

        #[test]
        fn portable_selection_survives_binding_completion_only_for_current_bound_source() {
            let _serial = plx_base::testlock::serial();
            let mut app = crate::app::run::lifecycle_regression_tests::app();
            for change in ["none", "missing", "root", "alias", "selected", "checkpoint"] {
                let mut runtime = runtime();
                runtime.load_catalog();
                let expected = runtime.continuity.pending_restore().cloned().unwrap();
                let generation = runtime.generation;
                runtime.local_inventory.phase = transient::InventoryPhase::Loading(generation);
                let index = profile_index(&runtime.saved, ProviderKind::Local, 15).unwrap();
                match change {
                    "missing" => { runtime.saved.settings.remove(&format!("offlineBinding:{}", fixture()["binding_id"].as_str().unwrap())); }
                    "root" => runtime.saved.profiles.profiles[index].source = "/media/other".into(),
                    "alias" => runtime.saved.profiles.profiles[index].source_alias = "Other films".into(),
                    "selected" => runtime.saved.settings.get_mut("offlineProfile:15").unwrap()["selected"]["name"] = json!("Comedy/other.mp4"),
                    "checkpoint" => {
                        let profile = runtime.active().unwrap().clone();
                        runtime.checkpoints.insert(profile.id.clone(), SavedResume {
                            entry_id: selected_entry(&runtime.saved, "Comedy/newer.mp4").id,
                            position_ms: 9_000, source_fingerprint: profile_fingerprint(&profile),
                        });
                    }
                    _ => {}
                }
                // A completed inventory operation must validate the pending
                // selection even when it supplies no new native file grants.
                transient::binding_result(&mut runtime, &mut app, generation, Ok(ott_catalog::LocalBindings {
                    sources: Vec::new(), lookup: None, matched: Vec::new(), operation_error: None,
                }));
                assert!(matches!(runtime.local_inventory.phase, transient::InventoryPhase::Ready(_)));
                if change == "none" {
                    assert_eq!(runtime.continuity.pending_restore(), Some(&expected));
                } else {
                    assert!(runtime.continuity.pending_restore().is_none(), "{change}");
                }
            }
        }

        #[test]
        fn imported_selection_starts_first_discovered_then_restores_once() {
            let _serial = plx_base::testlock::serial();
            let mut runtime = runtime();
            let first = selected_entry(&runtime.saved, "Comedy/first.mp4").id;
            let selected = selected_entry(&runtime.saved, "Comedy/example.mkv").id;
            runtime.load_catalog();
            let generation = runtime.generation;
            assert_eq!(runtime.continuity.catalog_updated(generation, &[&first], false),
                continuity::Decision::Start { entry_id: first.clone(), position_ms: 0, reason: continuity::Reason::First });
            assert_eq!(runtime.continuity.pending_restore().unwrap().entry_id, selected);
            assert_eq!(runtime.continuity.catalog_updated(generation, &[&first, &selected], false),
                continuity::Decision::Start { entry_id: selected.clone(), position_ms: 0, reason: continuity::Reason::Restore });
            assert_eq!(runtime.continuity.catalog_updated(generation, &[&first, &selected], true), continuity::Decision::Idle);
            assert!(runtime.continuity.pending_restore().is_none());
        }

        #[test]
        fn missing_imported_selection_finishes_without_restarting_first_item() {
            let _serial = plx_base::testlock::serial();
            let mut runtime = runtime();
            let first = selected_entry(&runtime.saved, "Comedy/first.mp4").id;
            runtime.load_catalog();
            let generation = runtime.generation;
            assert!(matches!(runtime.continuity.catalog_updated(generation, &[&first], false), continuity::Decision::Start { .. }));
            assert_eq!(runtime.continuity.catalog_updated(generation, &[&first], true), continuity::Decision::Idle);
            assert!(runtime.continuity.pending_restore().is_none());
        }

        #[test]
        fn portable_selection_cannot_replace_global_favorites_or_newer_intent() {
            let _serial = plx_base::testlock::serial();
            for change in ["favorites", "http", "stop", "selection", "generation"] {
                let mut runtime = runtime();
                let first = selected_entry(&runtime.saved, "Comedy/first.mp4").id;
                let selected = selected_entry(&runtime.saved, "Comedy/example.mkv").id;
                runtime.saved.favorites_selected = change == "favorites";
                if change == "http" {
                    let index = profile_index(&runtime.saved, ProviderKind::Local, 15).unwrap();
                    runtime.saved.profiles.profiles[index].source = "https://media.invalid/library.json".into();
                    let key = format!("offlineBinding:{}", fixture()["binding_id"].as_str().unwrap());
                    runtime.saved.settings.get_mut(&key).unwrap()["root"] = json!("https://media.invalid/library.json");
                }
                runtime.load_catalog();
                let generation = runtime.generation;
                if change == "favorites" {
                    assert_eq!(runtime.page, "favorites");
                    assert!(runtime.continuity.pending_restore().is_none());
                    continue;
                }
                if change == "http" {
                    assert!(runtime.continuity.pending_restore().is_none());
                    continue;
                }
                match change {
                    "stop" => runtime.continuity.stop(generation),
                    "selection" => runtime.continuity.user_selected(generation, &first),
                    "generation" => runtime.cancel_catalog(),
                    _ => unreachable!(),
                }
                assert_eq!(runtime.continuity.catalog_updated(generation, &[&first, &selected], true), continuity::Decision::Idle, "{change}");
                assert!(runtime.continuity.pending_restore().is_none(), "{change}");
            }
        }
    }
}
