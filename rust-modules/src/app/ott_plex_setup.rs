//! Pure planning for reconnecting the OTT Plex profile list.
//!
//! The runner supplies the current, credential-eligible server snapshot. This
//! module never reads the Plex registry, starts authentication, or saves state.
use plx_data::ott::{ProviderKind, ProviderProfile, ProviderProfiles};
use std::collections::BTreeSet;

pub(super) struct PlexSetupServer {
    pub machine_id: String,
    pub name: String,
    /// A naming hint only. Persistent matching always uses `machine_id`.
    pub id_hint: u16,
}

#[derive(Debug, PartialEq)]
pub(super) enum PlexSetupPlan {
    Login,
    Profiles {
        updated: ProviderProfiles,
        first_profile_id: String,
        changed: bool,
        omitted_servers: usize,
    },
}

pub(super) fn plan(
    profiles: &ProviderProfiles,
    servers: &[PlexSetupServer],
) -> Result<PlexSetupPlan, &'static str> {
    profiles.validate().map_err(|_| "Invalid saved provider profiles")?;
    if servers.is_empty() {
        return Ok(PlexSetupPlan::Login);
    }

    let mut updated = profiles.clone();
    let mut seen = BTreeSet::new();
    let mut first_profile_id = None;
    let mut omitted_servers = 0;
    for server in servers {
        if !seen.insert(server.machine_id.as_str()) {
            continue;
        }
        let matching = |profile: &ProviderProfile| {
            profile.kind == ProviderKind::Plex && profile.source == server.machine_id
        };
        let existing = updated.profiles.iter().position(|p| p.enabled && matching(p))
            .or_else(|| updated.profiles.iter().position(matching));
        if let Some(index) = existing {
            let profile = &mut updated.profiles[index];
            profile.enabled = true;
            first_profile_id.get_or_insert_with(|| profile.id.clone());
            continue;
        }

        let mut candidate = ProviderProfile {
            id: format!("plex-native-{}", server.id_hint),
            name: if server.name.trim().is_empty() {
                "Plex".into()
            } else {
                server.name.clone()
            },
            kind: ProviderKind::Plex,
            source: server.machine_id.clone(),
            source_alias: String::new(),
            enabled: true,
        };
        candidate.validate().map_err(|_| "Invalid Plex server metadata")?;
        candidate.id = available_id(&candidate.id, &updated)?;
        let id = candidate.id.clone();
        updated.profiles.push(candidate);
        // Existing state and this individual profile were validated above; IDs
        // are unique and the active selection is unchanged. Failure here is the
        // aggregate profile limit, whose authority remains ProviderProfiles.
        if updated.validate().is_err() {
            updated.profiles.pop();
            omitted_servers += 1;
            continue;
        }
        first_profile_id.get_or_insert(id);
    }

    let first_profile_id = first_profile_id.ok_or("No free Plex profile slot")?;
    let changed = updated != *profiles;
    Ok(PlexSetupPlan::Profiles {
        updated,
        first_profile_id,
        changed,
        omitted_servers,
    })
}

fn available_id(base: &str, profiles: &ProviderProfiles) -> Result<String, &'static str> {
    // N occupied IDs cannot consume all N+1 distinct candidates. The bound comes
    // from the validated input rather than a second profile-cap constant.
    for suffix in 0..=profiles.profiles.len() {
        let id = if suffix == 0 { base.to_owned() } else { format!("{base}_{suffix}") };
        if !profiles.profiles.iter().any(|profile| profile.id == id) {
            return Ok(id);
        }
    }
    Err("No free Plex profile identifier")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(machine: &str, id_hint: u16) -> PlexSetupServer {
        PlexSetupServer { machine_id: machine.into(), name: "Discovered server".into(), id_hint }
    }

    fn profile(id: &str, machine: &str, enabled: bool) -> ProviderProfile {
        ProviderProfile {
            id: id.into(), name: "My Plex name".into(), kind: ProviderKind::Plex,
            source: machine.into(), source_alias: "Portable alias".into(), enabled,
        }
    }

    fn saved(rows: Vec<ProviderProfile>) -> ProviderProfiles {
        ProviderProfiles { version: 1, active_profile: None, profiles: rows }
    }

    fn result(plan: PlexSetupPlan) -> (ProviderProfiles, String, bool, usize) {
        match plan {
            PlexSetupPlan::Profiles { updated, first_profile_id, changed, omitted_servers } => {
                (updated, first_profile_id, changed, omitted_servers)
            }
            PlexSetupPlan::Login => panic!("Expected available Plex profiles"),
        }
    }

    #[test]
    fn no_current_servers_requires_login_even_with_disabled_or_stale_profiles() {
        assert_eq!(plan(&ProviderProfiles::default(), &[]), Ok(PlexSetupPlan::Login));
        let rows = saved(vec![profile("disabled", "old-a", false), profile("stale", "old-b", true)]);
        let before = rows.clone();
        assert_eq!(plan(&rows, &[]), Ok(PlexSetupPlan::Login));
        assert_eq!(rows, before);
    }

    #[test]
    fn disabled_match_is_reenabled_without_replacing_identity_or_selection() {
        let mut other = profile("m3u-profile", "https://fixture.invalid/list.m3u", true);
        other.kind = ProviderKind::M3u;
        let mut rows = saved(vec![other, profile("persistent-plex-id", "machine-a", false)]);
        rows.active_profile = Some("m3u-profile".into());
        let mut expected = rows.clone();
        expected.profiles[1].enabled = true;
        let (updated, first, changed, omitted) = result(plan(&rows, &[server("machine-a", 77)]).unwrap());
        assert_eq!(updated, expected);
        assert_eq!(first, "persistent-plex-id");
        assert!(changed);
        assert_eq!(omitted, 0);
        assert!(updated.validate().is_ok());
        assert!(!rows.profiles[1].enabled, "Planning must not mutate saved input");
    }

    #[test]
    fn enabled_match_wins_over_an_earlier_disabled_alias() {
        let rows = saved(vec![profile("disabled", "machine-a", false), profile("enabled", "machine-a", true)]);
        let (updated, first, changed, omitted) = result(plan(&rows, &[server("machine-a", 9)]).unwrap());
        assert_eq!(updated, rows);
        assert_eq!(first, "enabled");
        assert!(!changed);
        assert_eq!(omitted, 0);
    }

    #[test]
    fn reconnect_ignores_an_unusable_discovered_name_for_an_existing_profile() {
        let rows = saved(vec![profile("persistent-id", "machine-a", false)]);
        for discovered_name in ["bad\nlabel".to_owned(), "x".repeat(513)] {
            let mut discovered = server("machine-a", 12);
            discovered.name = discovered_name;
            let (updated, first, changed, omitted) = result(plan(&rows, &[discovered]).unwrap());
            let mut expected = rows.clone();
            expected.profiles[0].enabled = true;
            assert_eq!(updated, expected);
            assert_eq!(first, "persistent-id");
            assert!(changed);
            assert_eq!(omitted, 0);
        }
    }

    #[test]
    fn repeated_connect_and_changed_registry_slot_keep_the_saved_id() {
        let (first, id, changed, _) = result(plan(&ProviderProfiles::default(), &[server("machine-a", 3)]).unwrap());
        assert!(changed);
        let (again, same_id, changed, omitted) = result(plan(&first, &[server("machine-a", 99)]).unwrap());
        assert_eq!(again, first);
        assert_eq!(same_id, id);
        assert!(!changed);
        assert_eq!(omitted, 0);
    }

    #[test]
    fn new_ids_avoid_all_provider_ids_without_overwriting_stale_plex() {
        let mut local = profile("plex-native-2_1", "/media/fixture", true);
        local.kind = ProviderKind::Local;
        let rows = saved(vec![profile("plex-native-2", "old-machine", false), local]);
        let (updated, first, _, _) = result(plan(&rows, &[server("new-machine", 2)]).unwrap());
        assert_eq!(first, "plex-native-2_2");
        assert_eq!(&updated.profiles[..2], rows.profiles.as_slice());
        assert_eq!(updated.profiles[2].source, "new-machine");
        assert!(updated.validate().is_ok());
    }

    #[test]
    fn repeated_machine_is_deduplicated_but_case_is_not_normalized() {
        let (updated, _, _, omitted) = result(plan(&ProviderProfiles::default(), &[
            server("Machine-A", 4), server("Machine-A", 5), server("machine-a", 4),
        ]).unwrap());
        assert_eq!(updated.profiles.len(), 2);
        assert_eq!(updated.profiles[0].source, "Machine-A");
        assert_eq!(updated.profiles[1].source, "machine-a");
        assert_ne!(updated.profiles[0].id, updated.profiles[1].id);
        assert_eq!(omitted, 0);
    }

    #[test]
    fn capacity_omits_only_new_servers_and_still_reenables_an_existing_match() {
        let rows = saved((0..15).map(|n| profile(&format!("saved-{n}"), &format!("machine-{n}"), false)).collect());
        assert!(rows.validate().is_ok());
        let (updated, first, changed, omitted) = result(plan(&rows, &[
            server("new-machine", 21), server("new-machine", 22), server("machine-9", 99),
        ]).unwrap());
        assert_eq!(updated.profiles.len(), 15);
        assert_eq!(first, "saved-9");
        assert!(updated.profiles[9].enabled);
        assert!(changed);
        assert_eq!(omitted, 1, "Count omitted machines, not repeated registry entries");
        assert!(updated.validate().is_ok());
    }

    #[test]
    fn a_full_list_with_no_represented_current_server_returns_capacity_error() {
        let rows = saved((0..15).map(|n| profile(&format!("saved-{n}"), &format!("old-{n}"), true)).collect());
        let before = rows.clone();
        assert_eq!(plan(&rows, &[server("new-machine", 1)]), Err("No free Plex profile slot"));
        assert_eq!(rows, before);
    }

    #[test]
    fn first_connection_reports_overflow_without_losing_the_usable_profiles() {
        let servers: Vec<_> = (0..16).map(|n| server(&format!("machine-{n}"), n)).collect();
        let (updated, first, changed, omitted) = result(plan(&ProviderProfiles::default(), &servers).unwrap());
        assert_eq!(updated.profiles.len(), 15);
        assert_eq!(first, "plex-native-0");
        assert!(changed);
        assert_eq!(omitted, 1);
        assert!(updated.validate().is_ok());
        assert!(updated.active_profile.is_none());
    }

    #[test]
    fn invalid_saved_state_is_not_misreported_as_login_or_capacity() {
        let rows = saved(vec![profile("duplicate", "machine-a", true), profile("duplicate", "machine-b", true)]);
        assert_eq!(plan(&rows, &[]), Err("Invalid saved provider profiles"));
        assert_eq!(plan(&rows, &[server("machine-a", 1)]), Err("Invalid saved provider profiles"));
    }

    #[test]
    fn invalid_server_metadata_is_not_misreported_as_capacity() {
        let rows = ProviderProfiles::default();
        assert_eq!(plan(&rows, &[server("", 1)]), Err("Invalid Plex server metadata"));
        assert_eq!(plan(&rows, &[server(&"x".repeat(257), 1)]), Err("Invalid Plex server metadata"));
        let mut invalid = server("machine-a", 1);
        invalid.name = "bad\nlabel".into();
        assert_eq!(plan(&rows, &[invalid]), Err("Invalid Plex server metadata"));
        let mut unnamed = server("machine-a", 1);
        unnamed.name = "  ".into();
        let (updated, _, _, _) = result(plan(&rows, &[unnamed]).unwrap());
        assert_eq!(updated.profiles[0].name, "Plex");
        assert!(updated.validate().is_ok());
    }
}
