//! Fullscreen Favorites use the admitted source, never the browsed catalogue row.
use super::*;

pub(super) struct Pending {
    page: InstanceId,
    playing: Playing,
    managed_read: Option<crate::app::adapters::session::ManagedRead>,
    favorite: String,
    added: bool,
}

fn page(app: &App) -> Option<InstanceId> {
    (app.running
        && playback_allowed(app)
        && matches!(app.route(), AppArg::Player)
        && !app.pages.has_pending_navigation()
        && !app.pages.surface_up())
    .then(|| app.pages.top_page())
    .flatten()
}

/// Refresh only receipt metadata; the originally admitted open media remains the
/// authority. Inventory replacement withdraws that media's source-owned fence.
fn local_grant(
    runtime: &Runtime,
    playing: &Playing,
) -> Result<Option<transient::LocalSelection>, &'static str> {
    if playing.profile.kind != ProviderKind::Local
        || plx_data::ott::http_url(&playing.profile.source).is_some()
    {
        return Ok(None);
    }
    let (
        Some(plx_media::ff::ExternalSource::HeldLocal(held)),
        MediaLocator::Local { relative_path },
    ) = (playing.source.as_ref(), &playing.entry.media)
    else {
        return Err("The playing local file has no retained grant");
    };
    if !held.is_live() {
        return Err("The playing local file was withdrawn");
    }
    let selected = if let Some(previous) = playing.local_selection.as_ref() {
        // This keeps exact source id/authority/generation before a receipt refresh.
        let root = runtime
            .current_local_source(previous)
            .ok_or("The playing local source changed")?;
        runtime.bind_local_selection(root)?
    } else {
        runtime
            .local_profile_selection(&playing.profile)
            .ok_or("The playing local source is unavailable")?
    };
    let read = runtime
        .capture_local_source(&selected)
        .ok_or("The playing local source is unavailable")?;
    if read.source.descriptor().canonical_path.as_deref() != Some(playing.profile.source.as_str()) {
        return Err("The playing local source changed");
    }
    if let Some(transient::TransientMedia::Local(session)) = runtime.transient.accepted.as_ref() {
        if session.read.selection.source_id == selected.source_id
            && (session.read.selection.authority != selected.authority
                || session.read.selection.generation != selected.generation
                || !session.read.is_live()
                || !read.same_grant(&session.read))
        {
            return Err("The playing local source capability changed");
        }
    }
    if read.source.descriptor().kind == crate::app::ott_local_session::SourceKind::File
        && read.source.single_file(relative_path).as_ref() != Some(held)
    {
        return Err("The playing local file capability changed");
    }
    // A non-session Folder retains its original opened media with the root's
    // withdrawal fence; a newly opened same-path root cannot revive that fence.
    Ok(Some(selected))
}

fn owner<'a>(
    runtime: &'a Runtime,
    playing: &'a Playing,
) -> Result<selection::SelectionOwner<'a>, &'static str> {
    use selection::SelectionOwner;
    if let Some(receipt) = runtime.playing_attachment.as_ref() {
        if !runtime.m3u_attachments.is_current(receipt)
            || receipt.effective_profile().ok().as_ref() != Some(&playing.profile)
        {
            return Err("The playing attachment changed");
        }
        return Ok(SelectionOwner::M3uAttachment(receipt));
    }
    let refreshed = local_grant(runtime, playing)?;
    if playing.local_selection.is_some() {
        let selected = refreshed
            .as_ref()
            .ok_or("The playing local source changed")?;
        let root = runtime
            .current_local_source(selected)
            .ok_or("The playing local source is unavailable")?;
        if transient::source_profile(root).ok().as_ref() == Some(&playing.profile) {
            return Ok(SelectionOwner::LocalRoot(root));
        }
        if root.kind == crate::app::ott_local_session::SourceKind::File {
            // Ordinary File rows retain their durable numbered profile, while
            // replay uses source_profile(root). Both must capture the same root.
            // local_grant already proved the exact live single-file handle;
            // do not turn an edited, removed or duplicated profile into authority.
            let mut profiles = runtime.saved.profiles.profiles.iter()
                .filter(|profile| profile.id == playing.profile.id);
            if playing.profile.kind == ProviderKind::Local && playing.profile.enabled
                && profiles.next() == Some(&playing.profile) && profiles.next().is_none()
            {
                return Ok(SelectionOwner::LocalRoot(root));
            }
            return Err("The playing local profile changed");
        }
        if root.kind != crate::app::ott_local_session::SourceKind::Folder
            || playing.profile.kind != ProviderKind::Local
            || root.canonical_path.as_deref() != Some(playing.profile.source.as_str())
        {
            return Err("The playing local source changed");
        }
    }
    Ok(SelectionOwner::Profile(&playing.profile))
}

/// A current complete owner catalogue supersedes retained evidence. Incomplete
/// refreshes and unrelated browsing retain only evidence from native admission.
fn channel_capture(runtime: &Runtime, playing: &Playing) -> Result<Option<selection::ChannelCapture>, &'static str> {
    if playing.profile.kind != ProviderKind::M3u { return Ok(None); }
    let bindings = selection::SelectionBindings::profiles_only(&runtime.saved.profiles)
        .with_attachments(&runtime.m3u_attachments);
    let owner = owner(runtime, playing)?;
    if runtime.catalog_complete && runtime.effective_profile() == Some(&playing.profile) {
        return selection::capture_channel(&bindings, owner, &runtime.catalog.entries, &playing.entry)
            .map_err(selection::BindingError::message);
    }
    let proof = playing.channel_capture.as_ref().ok_or("The playing channel catalogue is not complete")?;
    proof.favorite(&bindings, owner, &playing.entry).map_err(selection::BindingError::message)?;
    Ok(Some(proof.clone()))
}

pub(super) fn capture(runtime: &Runtime, playing: &Playing) -> Result<Favorite, &'static str> {
    let owner = owner(runtime, playing)?;
    let channel = channel_capture(runtime, playing)?;
    let bindings = if playing.profile.kind == ProviderKind::Local {
        runtime.selection_bindings().map_err(selection::BindingError::message)?
    } else {
        selection::SelectionBindings::profiles_only(&runtime.saved.profiles)
            .with_temporary_profile(managed::binding_profile(runtime)
                .or_else(|| classic_source::binding_profile(runtime)))
            .with_attachments(&runtime.m3u_attachments)
    };
    match channel {
        Some(proof) => proof.favorite(&bindings, owner, &playing.entry),
        None => selection::capture_bound(&bindings, owner, &playing.entry),
    }.map_err(selection::BindingError::message)
}

/// The caller supplies an actual admitted Playing value after checking its native identity.
/// One admitted media row is enough to attest its own exact Local relative path; it does
/// not authorize a basename imported from another player or a row in a different source.
pub(super) fn candidate(
    runtime: &Runtime,
    playing: &Playing,
) -> Result<(Saved, String, bool), &'static str> {
    let owner = owner(runtime, playing)?;
    let bindings = if playing.profile.kind == ProviderKind::Local {
        runtime
            .selection_bindings()
            .map_err(selection::BindingError::message)?
    } else {
        selection::SelectionBindings::profiles_only(&runtime.saved.profiles)
            .with_temporary_profile(managed::binding_profile(runtime)
                .or_else(|| classic_source::binding_profile(runtime)))
            .with_attachments(&runtime.m3u_attachments)
    };
    let channel = channel_capture(runtime, playing)?;
    let favorite = capture(runtime, playing)?;
    let mut saved = runtime.saved.clone();
    let kiosk = super::super::ott_kiosk_stored::read(&saved.settings)
        .map_err(|_| "Invalid stored kiosk policy")?;
    let added = saved
        .local_favorite_proofs
        .toggle_capture_with_channel(
            &mut saved.favorites,
            kiosk.as_ref().map_or(&[], |pin| pin.pins()),
            &bindings,
            owner,
            std::slice::from_ref(&playing.entry),
            &playing.entry,
            channel.as_ref(),
        )
        .map_err(selection::BindingError::message)?;
    // The existing proof-aware toggler also removes legacy equivalent identities.
    Ok((saved, favorite.identity(), added))
}

/// A channel admitted before the final batch gains evidence when its own
/// catalogue completes. Later browsing can retain this bounded projection.
pub(super) fn catalog_finished(runtime: &mut Runtime) {
    if !runtime.catalog_complete { return; }
    let Some(playing) = runtime.playing.as_ref().filter(|playing|
        playing.profile.kind == ProviderKind::M3u && runtime.effective_profile() == Some(&playing.profile)) else { return; };
    let bindings = selection::SelectionBindings::profiles_only(&runtime.saved.profiles)
        .with_attachments(&runtime.m3u_attachments);
    let captured = selection::capture_channel(&bindings, selection::SelectionOwner::Profile(&playing.profile),
        &runtime.catalog.entries, &playing.entry).ok().flatten();
    if let Some(playing) = runtime.playing.as_mut() { playing.channel_capture = captured; }
}

fn notice(app: &mut App, message: &'static str) -> bool {
    let now = app.loop_t;
    if let Some(player) = bridge::player_mut(&mut app.pages) {
        return player.favorite_notice(message, now);
    }
    false
}

pub(super) fn toggle(runtime: &mut Runtime, app: &mut App) -> Result<(), &'static str> {
    let result = (|| {
        let page = page(app).ok_or("Favorites require the active video page")?;
        let playing = runtime
            .playing
            .as_ref()
            .filter(|playing| playing.identity.is_some() && playing_matches(playing, app))
            .cloned()
            .ok_or("The playing source is no longer current")?;
        input_policy::ui(
            runtime,
            &OttAction::ToggleFavorite(playing.selection_id.clone()),
        )?;
        let owner = owner(runtime, &playing)?;
        let policy = match owner {
            selection::SelectionOwner::M3uAttachment(receipt) => receipt.parent().clone(),
            _ => playing.profile.clone(),
        };
        runtime.require_parental_entry(&policy, Some(&playing.entry))?;
        classic_source::guard_profile(runtime, app, &playing.profile)?;
        let managed_read = managed::capture_read(runtime, app, &playing.profile)?;
        let (saved, favorite, added) = candidate(runtime, &playing)?;
        runtime.save(saved, None, false)?;
        runtime.pending_playing_favorite = Some(Pending {
            page,
            playing,
            managed_read,
            favorite,
            added,
        });
        Ok(())
    })();
    if result.is_err() {
        notice(app, plx_platform::i18n::msg::ott_favorite_update_failed());
    }
    result
}

pub(super) fn settled(runtime: &mut Runtime, app: &mut App, success: bool) -> bool {
    let Some(pending) = runtime.pending_playing_favorite.take() else {
        return false;
    };
    if page(app) != Some(pending.page)
        || !playing_matches(&pending.playing, app)
        || owner(runtime, &pending.playing).is_err()
        || pending.managed_read.as_ref().is_some_and(|read| !read.current())
        || managed::guard_profile(runtime, app, &pending.playing.profile).is_err()
        || classic_source::guard_profile(runtime, app, &pending.playing.profile).is_err()
        || !runtime.playing.as_ref().is_some_and(|current| {
            current.identity == pending.playing.identity
                && current.profile == pending.playing.profile
                && current.entry.id == pending.playing.entry.id
                && current.entry.media == pending.playing.entry.media
        })
    {
        return false;
    }
    let matches = runtime
        .saved
        .favorites
        .entries()
        .iter()
        .any(|favorite| favorite.identity() == pending.favorite)
        == pending.added;
    let message = if !success || !matches {
        plx_platform::i18n::msg::ott_favorite_update_failed()
    } else if pending.added {
        plx_platform::i18n::msg::ott_favorite_added()
    } else {
        plx_platform::i18n::msg::ott_favorite_removed()
    };
    notice(app, message)
}
