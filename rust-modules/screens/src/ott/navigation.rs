//! A compact shared native header. Less frequently used actions remain in the
//! Browse form; a narrow layout never drops the only path to an action.
use plx_machine::machine::{GroupId, Measure};
use plx_platform::i18n::msg;
use plx_ui::page_header::{self, HeaderItem, HeaderRow, PageHeader, PageHeaderSpec, Slot};
use plx_ui::route_screen::RouteLayout;
use plx_ui::Rect;
use plx_ui::table::Row;
use super::{FormAction, OttAction, OttView};
use plx_appkit::menu_visibility::Visibility;

pub(super) const GROUP: GroupId = GroupId(1);
pub(super) const FOOTER_GROUP: GroupId = GroupId(2);

/// Modern channel/category pages expose the same contextual actions at the bottom edge.
pub(super) fn modern_footer(measure: &dyn Measure, width: f32, actions: &[Action],
    keys: &std::collections::HashMap<String, u32>, snapshot: &OttView, header_keys: &[u32]) -> Option<HeaderRow> {
    let mut controls: Vec<(u32, String)> = if matches!(snapshot.page_id.split(':').next(), Some("channels" | "favorites")) {
        classic_controls(snapshot, actions, keys)
    } else if snapshot.page_id.split(':').next() == Some("categories") {
        ["nav:categories", "nav:actions", "nav:description", "nav:search"].iter().filter_map(|id| {
            let action = actions.iter().find(|action| action.id == *id)?;
            Some((*keys.get(*id)?, action.row.label.clone()))
        }).collect()
    } else { return None; };
    controls.retain(|(key, _)| !header_keys.contains(key));
    if controls.is_empty() { return None; }
    let left = controls.iter().map(|(_, label)| HeaderItem { label: label.clone(), focusable: true, ..Default::default() }).collect();
    let page = PageHeader::new(PageHeaderSpec { left, ..Default::default() }, &[], measure);
    let mut layout = page.layout(width, measure);
    let top = (Rect::FULL.h - page_header::HEIGHT - page_header::TOP).max(page_header::TOP);
    for item in &mut layout.items { item.text.y = top; item.plate.y = top; }
    layout.glyph = None;
    let stops = controls.iter().enumerate().map(|(index, (key, _))| (*key, index)).collect();
    Some(HeaderRow::new(page, layout, stops))
}

pub(super) fn header(measure: &dyn Measure, width: f32, browse: u32, search: Option<u32>,
    favorite: Option<(u32, &str)>, description: Option<u32>, open: bool, caption: &str) -> HeaderRow {
    let mut controls = Vec::new();
    let mut primary = 0;
    if !open {
        if let Some(key) = search { controls.push((key, msg::ott_search().to_owned())); }
        if let Some((key, label)) = favorite { controls.push((key, label.to_owned())); }
        primary = controls.len();
        if let Some(key) = description { controls.push((key, msg::ott_description().to_owned())); }
    }
    build_header(measure, width, browse, &controls, primary, open, Some(caption))
}

/// Classic uses the same action identities as Browse, but the web footer's
/// inline key hints instead of the Modern library header's split clusters.
pub(super) fn context_header_classic(measure: &dyn Measure, width: f32, browse: u32,
    labels: &[(u32, String)], open: bool, shortcuts: &[(u32, Vec<page_header::FooterKey>)], size: i32) -> HeaderRow {
    let mut controls = labels.to_vec();
    controls.sort_by_key(|(id, _)| shortcuts.iter().find(|(key, _)| key == id)
        .and_then(|(_, keys)| keys.first()).map_or(3, |key| match key.tone {
            page_header::FooterTone::Red => 0, page_header::FooterTone::Blue => 1,
            page_header::FooterTone::Yellow => 2, page_header::FooterTone::Plain => 3 }));
    loop {
        let mut left: Vec<_> = controls.iter().map(|(_, label)| HeaderItem { label: label.clone(),
            focusable: true, ..Default::default() }).collect();
        left.push(HeaderItem { label: msg::ott_browse().into(), focusable: true,
            selected_plate: open, ..Default::default() });
        let spec = PageHeaderSpec {
            left, ..Default::default()
        };
        let page = PageHeader::new(spec, &[], measure);
        let layout = page.layout(width, measure);
        let stops = layout.items.iter().enumerate().filter_map(|(index, item)| {
            let id = match item.slot { Slot::Left(i) if i == controls.len() => browse,
                Slot::Left(i) => controls.get(i)?.0, _ => return None };
            Some((id, index))
        }).collect();
        let row = HeaderRow::new(page, layout, stops).with_classic_footer(width, size, shortcuts, measure);
        if controls.is_empty() || row.classic_footer_fits(width) { return row; }
        // Dropped controls retain their existing routes in Browse.
        controls.pop();
    }
}

/// Advertise only implemented Classic shortcuts with a currently exposed action.
/// Element IDs are matched through the owner's key map, never translated labels.
pub(super) fn classic_footer_keys(snapshot: &OttView, controls: &[(u32, String)],
    keys: &std::collections::HashMap<String, u32>) -> Vec<(u32, Vec<page_header::FooterKey>)> {
    use page_header::{FooterKey, FooterTone};
    let exposed = |id: &str| keys.get(id).copied().filter(|key| controls.iter().any(|(id, _)| id == key));
    // Match remote_input's existing page scope. A clickable action elsewhere
    // does not imply that its colored remote key is handled on that page.
    let list_keys = matches!(snapshot.page_id.split(':').next(), Some("channels" | "favorites" | "categories"));
    let mut out = Vec::new();
    if list_keys {
        if let Some(key) = (snapshot.page_id.split(':').next() == Some("categories")).then(|| exposed("nav:catalog")).flatten()
            .or_else(|| exposed("nav:guide")).or_else(|| exposed("nav:description")) {
            out.push((key, vec![FooterKey::color(FooterTone::Red)]));
        }
    }
    if let Some(key) = exposed("nav:categories") {
        let mut hints = vec![FooterKey::color(FooterTone::Blue)];
        if matches!(snapshot.page_id.split(':').next(), Some("channels" | "favorites")) {
            hints.push(FooterKey::plain("▶Ⅱ"));
        }
        out.push((key, hints));
    }
    let yellow = if snapshot.page_id.split(':').next() == Some("favorites") { "nav:favorite" } else { "nav:actions" };
    if list_keys {
        if let Some(key) = exposed(yellow) {
            out.push((key, vec![FooterKey::color(FooterTone::Yellow)]));
        }
    }
    // Native digits remain channel tuning. Description is a focusable real
    // footer action; OK is its established activation, rather than a fictitious
    // INFO scancode or stealing the television's number 2.
    if let Some(key) = exposed("nav:description").filter(|key| !out.iter().any(|(id, _)| id == key)) {
        out.push((key, vec![FooterKey::plain("OK")]));
    }
    out
}

fn build_header(measure: &dyn Measure, width: f32, browse: u32,
    labels: &[(u32, String)], primary: usize, open: bool, caption: Option<&str>) -> HeaderRow {
    let mut left = vec![HeaderItem { label: msg::ott_browse().into(), focusable: true,
        fold_glyph: true, selected_plate: open, ..Default::default() }];
    if let Some(caption) = caption.filter(|caption| !caption.is_empty()) {
        left.push(HeaderItem { label: caption.into(), title: true, ..Default::default() });
    }
    let mut controls: Vec<_> = labels.iter().map(|(key, label)| (*key, HeaderItem {
        label: label.clone(), focusable: true, ..Default::default()
    })).collect();
    // Measure the actual shared component. Overflow controls remain in Browse;
    // do not shrink translated labels or permit overlapping hit targets.
    loop {
        let spec = PageHeaderSpec { left: left.clone(),
            right: controls.iter().map(|(_, item)| item.clone()).collect(), ..Default::default() };
        let page = PageHeader::new(spec, &[], measure);
        let layout = page.layout(width, measure);
        let left_end = layout.items.iter().filter(|item| matches!(item.slot, Slot::Left(_)))
            .map(|item| item.plate.x + item.plate.w).fold(0.0, f32::max);
        let right_start = layout.items.iter().filter(|item| matches!(item.slot, Slot::Right(_)))
            .map(|item| item.plate.x).fold(f32::INFINITY, f32::min);
        // Description can move into Browse before shortening the category.
        // Search and Favorite take priority over a long read-only caption.
        if left_end + page_header::CLUSTER_GAP > right_start && controls.len() <= primary {
            if let Some(title) = layout.items.iter().find(|item| item.slot == Slot::Left(1)) {
                let trailing = title.plate.x + title.plate.w - title.text.x - title.text.w;
                let budget = (right_start - page_header::CLUSTER_GAP - title.text.x - trailing).max(0.0);
                let fitted = if budget > 0.0 {
                    measure.fit_line(&left[1].label, budget, plx_ui::theme::size::BODY, false)
                        .to_string_lossy().into_owned()
                } else { String::new() };
                if fitted != left[1].label {
                    left[1].label = fitted;
                    continue;
                }
            }
        }
        if controls.is_empty() {
            // Keep the shared header's trailing text margin. A long source
            // category is a readout, not another focus target or a second row.
            if let Some(title) = layout.items.iter().find(|item| item.slot == Slot::Left(1)) {
                let right = page_header::TEXT_X1 + (width - Rect::FULL.w);
                let budget = (right - title.text.x).max(0.0);
                if title.text.w > budget {
                    let fitted = if budget > 0.0 {
                        measure.fit_line(&left[1].label, budget, plx_ui::theme::size::BODY, false)
                            .to_string_lossy().into_owned()
                    } else { String::new() };
                    if fitted != left[1].label { left[1].label = fitted; continue; }
                }
            }
        }
        if controls.is_empty() || left_end + page_header::CLUSTER_GAP <= right_start {
            let stops = layout.items.iter().enumerate().filter_map(|(index, item)| {
                let key = match item.slot { Slot::Left(0) => Some(browse),
                    Slot::Right(i) => controls.get(i).map(|(key, _)| *key), _ => None }?;
                Some((key, index))
            }).collect();
            return HeaderRow::new(page, layout, stops);
        }
        controls.pop();
    }
}

// These are identities of existing Browse rows, not a second input mapping.
// Their actions below are also the form's actions and are rebuilt from the current selection.
pub(super) const CONTEXT_IDS: [&str; 15] = ["nav:providers", "nav:profiles", "nav:catalog", "nav:categories",
    "nav:favorites", "nav:settings", "nav:search", "nav:clear-search", "nav:actions", "nav:random",
    "nav:description", "nav:play", "nav:guide", "nav:records", "nav:favorite"];
const CONTEXT_CONTROLS: [&str; 9] = ["nav:catalog", "nav:categories", "nav:play", "nav:description", "nav:guide",
    "nav:actions", "nav:favorites", "nav:favorite", "nav:search"];
pub(super) fn is_context_control(id: &str) -> bool { CONTEXT_CONTROLS.contains(&id) }

pub(super) struct Action {
    pub id: &'static str,
    pub row: Row,
    pub action: FormAction,
}

pub(super) fn page_action(page: &str) -> &'static str {
    match page.split(':').next().unwrap_or("") {
        "profiles" | "usb-folders" => "nav:profiles", "channels" => "nav:catalog",
        "categories" => "nav:categories", "favorites" => "nav:favorites",
        "settings" => "nav:settings", _ => "nav:providers",
    }
}

#[cfg(test)]
pub(super) fn available(snapshot: &OttView, selected: Option<usize>, visible: Visibility) -> Vec<Action> {
    available_at(snapshot, selected, visible, None)
}

// Torrent selections are ephemeral protocol identities, not durable Favorites.
// Keep this same exclusion for Browse and the Modern long-press shortcut.
pub(super) fn favorite_available(page: &str, play_id: &str) -> bool {
    !(matches!(page, "torrent-results" | "torrent-files")
        && (play_id.starts_with("torrent:") || play_id.starts_with("torrent.file:")))
}

pub(super) fn available_at(snapshot: &OttView, selected: Option<usize>, visible: Visibility,
    now: Option<i64>) -> Vec<Action> {
    let mut out = Vec::new();
    let mut add = |id, row, action| {
        if visible.navigation(id) { out.push(Action { id, row, action }); }
    };
    for (id, title, action) in [
        ("nav:providers", msg::ott_providers(), OttAction::ShowProviders),
        ("nav:profiles", msg::ott_profiles(), OttAction::ShowProfiles),
        ("nav:catalog", msg::ott_channel_list(), OttAction::ShowCatalog),
        ("nav:categories", msg::ott_categories(), OttAction::ShowCategories),
        ("nav:favorites", msg::ott_favorites(), OttAction::ShowFavorites),
        ("nav:settings", msg::settings_title(), OttAction::ShowSettings),
    ] { add(id, Row::new(title).chevron(true), FormAction::Forward(action)); }
    if let Some(search) = &snapshot.search {
        let detail = if search.query.is_empty() { search.scope.clone() } else { format!("{} · {}", search.scope, search.query) };
        let favorites = super::favorites_page(&snapshot.page_id);
        let row = Row::new(search.title(favorites)).detail(detail).server_detail().chevron(true);
        let row = if !favorites && !search.scope.is_empty() { row.server_label() } else { row };
        add("nav:search", row, FormAction::Forward(OttAction::BeginSearch));
        if !search.query.is_empty() { add("nav:clear-search", Row::new(msg::ott_search_clear()), FormAction::Forward(OttAction::ClearSearch)); }
    }
    if let Some(context) = &snapshot.order_context {
        let action = selected.and_then(|index| snapshot.rows.get(index))
            .filter(|row| row.enabled).and_then(|row| match &row.action {
                OttAction::Play(id) => Some(OttAction::ShowActionsAt { context: context.clone(), id: id.clone() }),
                _ => None,
            }).unwrap_or_else(|| OttAction::ShowActions(context.clone()));
        add("nav:actions", Row::new(msg::ott_order_actions()).chevron(true), FormAction::Forward(action));
    }
    if let Some(context) = &snapshot.random_context {
        add("nav:random", Row::new(msg::ott_random_play()), FormAction::Forward(OttAction::Random(context.clone())));
    }
    if let Some(row) = selected.and_then(|index| snapshot.rows.get(index)).filter(|row| row.enabled) {
        let description = super::metadata::description(row, now);
        if !description.is_empty() { add("nav:description", Row::new(msg::ott_description()).chevron(true), FormAction::Details); }
        if let OttAction::Play(id) = &row.action {
            add("nav:play", Row::new(msg::browse_detail_play()).detail(row.title.clone()).server_detail(), FormAction::Forward(OttAction::Play(id.clone())));
            if row.guide_available { add("nav:guide", Row::new(msg::ott_guide()).chevron(true), FormAction::Forward(OttAction::OpenGuide(id.clone()))); }
            if row.records_available { add("nav:records", Row::new(msg::ott_records()).chevron(true), FormAction::Forward(OttAction::OpenRecords(id.clone()))); }
            if favorite_available(&snapshot.page_id, id) {
                add("nav:favorite", Row::new(if row.favorite { msg::ott_favorite_remove() } else { msg::ott_favorite_add() })
                    .detail(row.title.clone()).server_detail(), FormAction::Forward(OttAction::ToggleFavorite(id.clone())));
            }
        }
    }
    // Opening Browse keeps focus on the current page. Put its common list
    // operations immediately below that stable identity, before global tools.
    let mut primary = Vec::new();
    for id in ["nav:search", "nav:favorite"] {
        if let Some(index) = out.iter().position(|item| item.id == id) {
            primary.push(out.remove(index));
        }
    }
    let insert = out.iter().position(|item| item.id == page_action(&snapshot.page_id))
        .map_or(0, |index| index + 1);
    out.splice(insert..insert, primary);
    out
}

pub(super) fn classic_controls(snapshot: &OttView, actions: &[Action], keys: &std::collections::HashMap<String, u32>) -> Vec<(u32, String)> {
    let page = snapshot.page_id.split(':').next().unwrap_or("");
    let catalogue = matches!(page, "channels" | "favorites" | "torrent-results" | "torrent-files");
    let primary = if actions.iter().any(|item| item.id == "nav:guide") { "nav:guide" } else { "nav:description" };
    let context = if page == "favorites" { "nav:favorite" } else { "nav:actions" };
    let compact = [primary, "nav:categories", context,
        if primary == "nav:guide" { "nav:description" } else { "" },
        "nav:search", if page == "favorites" { "" } else { "nav:favorite" }];
    // Additional operations keep their ordinary Browse routes. This is only
    // the channel footer's presentation, not a new action/capability policy.
    let categories = ["nav:catalog", "nav:actions", "nav:search", "nav:favorites"];
    let ids = if catalogue { compact.as_slice() } else if page == "categories" { categories.as_slice() }
        else { CONTEXT_CONTROLS.as_slice() };
    ids.iter().filter_map(|&id| {
        let item = actions.iter().find(|item| item.id == id)?;
        let label = match id {
            "nav:guide" if catalogue => msg::ott_classic_epg().to_owned(),
            "nav:categories" if catalogue => msg::ott_classic_category().to_owned(),
            "nav:search" => msg::ott_search().to_owned(),
            _ => item.row.label.clone(),
        };
        Some((*keys.get(id)?, label))
    }).collect()
}

pub(super) fn caption(view: &OttView) -> String {
    match view.category_label.as_deref().filter(|label| !label.trim().is_empty() && *label != view.title) {
        Some(category) => format!("{} · {category}", view.title),
        None => view.title.clone(),
    }
}

/// Classic uses the projected category label without interpreting source or route IDs.
pub(super) fn classic_caption(view: &OttView) -> String {
    match view.page_id.split(':').next() {
        Some("categories") => msg::ott_classic_category_selection().into(),
        Some("channels" | "favorites") => {
            match view.category_label.as_deref().filter(|label| !label.trim().is_empty()) {
                Some(category) => msg::ott_classic_channel_category(category),
                None => caption(view),
            }
        }
        _ => caption(view),
    }
}

pub(super) fn route() -> RouteLayout {
    let mut layout = RouteLayout::screen();
    for frame in [&mut layout.narrative, &mut layout.content] {
        let bottom = frame.y + frame.h;
        frame.y = page_header::GRID_TOP;
        frame.h = (bottom - frame.y).max(0.0);
    }
    layout
}

pub(super) fn clip() -> Rect { Rect::FULL }

// Catalogue arrows consume the shared form's paging result, never maintain a
// second cursor. Header, editor, Browse, reader and Modern retain normal focus.
impl super::OttScreen {
    pub(super) fn classic_arrow_input<H: super::AppLike<Memory = super::PageMemory>>(&mut self,
        input: &plx_machine::machine::InputEvent<u32>, cx: &super::Cx<'_, H>,
        fx: &mut super::Effects<'_, H>) -> Option<super::Handled> {
        use super::{Edge, InputKind, Key, OttMediaKind};
        use plx_machine::machine::{Delivery, FocusKey, Fx, MachineId};
        use plx_ui::screen::{Enter, FocusTarget, ScreenEvent};
        let InputKind::Key { key, edge, at_edge, .. } = input.kind else { return None; };
        let direction = match key { Key::Left => -1, Key::Right => 1, _ => return None };
        if !self.classic_mode || !self.classic_visible || self.editor.is_some() || self.reading || self.browse
            || !self.form.table.is_classic_catalogue()
        { return None; }
        let current = cx.focus.current.filter(|focus| focus.entry == self.entry
            && self.catalog_keys.contains(&focus.elem))?;
        if edge == Edge::Up || at_edge { return Some(super::Handled::Yes); }
        let mode = self.snapshot.classic_preferences.arrow_mode();
        if mode == 1 && self.snapshot.volume_available {
            self.send(OttAction::VolumeStep(direction), fx);
            return Some(super::Handled::Yes);
        }
        if mode == 2 {
            let selected = self.media_keys.get(&current.elem).and_then(|index| self.snapshot.rows.get(*index));
            if let Some(row) = selected.filter(|row| row.enabled && matches!(&row.action, OttAction::Play(_))) {
                // Only typed finite metadata selects media Back. Missing metadata
                // does not turn a channel into an invented folder/media owner.
                let finite = row.media.as_ref().is_some_and(|media| !matches!(media.kind, OttMediaKind::LiveChannel));
                if edge == Edge::Down {
                    if direction < 0 {
                        if finite { self.send(OttAction::Back, fx); }
                        else { self.activate_shortcut("nav:categories", fx); }
                    } else if finite || !self.activate_shortcut("nav:guide", fx) {
                        self.activate_shortcut("nav:description", fx);
                    }
                }
                return Some(super::Handled::Yes);
            }
        }
        // Legacy 3 has the same paging fallback as Classic 0. Category rows also
        // page in contextual mode; only their explicit OK selects a category.
        if let Some(target) = self.remote_page_key(current.elem, i32::from(direction)) {
            self.restore_memory = None;
            if target != current.elem {
                self.search_return.pending = None;
                self.search_return.from_actions = None;
            }
            fx.push(Fx::Deliver(MachineId::Instance(self.instance), Delivery::Screen(
                ScreenEvent::Enter(Enter::Fresh { focus: FocusTarget::Elem(FocusKey {
                    entry: self.entry, elem: target }) }))));
        }
        Some(super::Handled::Yes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plx_machine::machine::{FontScale, TypographyMeasure};

    #[test]
    fn compact_header_keeps_browse_and_never_overlaps_at_large_fonts() {
        for language in plx_platform::i18n::SHIPPED {
            let _locale = plx_platform::i18n::language_on_this_thread_for_test(language);
            for scale in [50, 100, 150] {
                let mut measure = TypographyMeasure::new(plx_ui::fixture::FixtureMeasure);
                measure.set_font_scale(FontScale::from_percent(scale).unwrap());
                for width in [1280.0, 1920.0] {
                    let row = header(&measure, width, 1, Some(2), Some((4, msg::ott_favorite_add())), Some(3), false, "");
                    assert!(row.stops.iter().any(|(key, _)| *key == 1), "Browse is always reachable");
                    for (i, a) in row.layout.items.iter().enumerate() {
                        assert!(a.plate.x >= 0.0 && a.plate.x + a.plate.w <= width,
                            "{} at {scale}%: {:?}", language.tag(), a.plate);
                        for b in row.layout.items.iter().skip(i + 1) {
                            assert!(a.plate.x + a.plate.w <= b.plate.x || b.plate.x + b.plate.w <= a.plate.x,
                                "{} at {scale}% overlaps", language.tag());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn both_content_columns_clear_the_header_without_moving_the_action_band() {
        let before = RouteLayout::screen();
        let after = route();
        assert_eq!(after.narrative.y, page_header::GRID_TOP);
        assert_eq!(after.content.y, page_header::GRID_TOP);
        assert_eq!(after.content.y + after.content.h, before.content.y + before.content.h);
        assert_eq!(after.action, before.action);
    }
    #[test]
    fn classic_action_bar_uses_shared_measured_nonoverlapping_controls_in_every_locale() {
        for language in plx_platform::i18n::SHIPPED {
            let _locale = plx_platform::i18n::language_on_this_thread_for_test(language);
            let labels = vec![(2, msg::ott_categories().to_owned()), (3, msg::browse_detail_play().to_owned()),
                (4, msg::ott_description().to_owned()), (5, msg::ott_guide().to_owned()),
                (6, msg::ott_order_actions().to_owned()), (7, msg::ott_favorites().to_owned())];
            for scale in [20, 100, 200] {
                let mut measure = TypographyMeasure::new(plx_ui::fixture::FixtureMeasure);
                measure.set_font_scale(FontScale::from_percent(scale).unwrap());
                for width in [1280.0, 1920.0] {
                    let shortcuts = vec![(2, vec![page_header::FooterKey::color(page_header::FooterTone::Blue)]),
                        (4, vec![page_header::FooterKey::color(page_header::FooterTone::Red)])];
                    let row = context_header_classic(&measure, width, 1, &labels, false, &shortcuts, 33);
                    assert!(row.stops.iter().any(|(key, _)| *key == 1));
                    for (i, a) in row.layout.items.iter().enumerate() {
                        assert!(a.plate.x >= 0.0 && a.plate.x + a.plate.w <= width);
                        for b in row.layout.items.iter().skip(i+1) {
                            assert!(a.plate.x+a.plate.w <= b.plate.x || b.plate.x+b.plate.w <= a.plate.x,
                                "{} at {scale}%", language.tag());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn classic_footer_advertises_only_exposed_exact_shortcuts() {
        use page_header::FooterTone;
        let keys = [("nav:guide", 1), ("nav:description", 2), ("nav:categories", 3),
            ("nav:actions", 4), ("nav:favorite", 5)].into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
        let all = (1..=5).map(|id| (id, format!("control {id}"))).collect::<Vec<_>>();
        let mut snapshot = OttView { page_id: "channels".into(), ..Default::default() };
        let tone = |items: Vec<(u32, Vec<page_header::FooterKey>)>| items.into_iter()
            .map(|(id, keys)| (id, keys[0].tone)).collect::<Vec<_>>();
        assert_eq!(tone(classic_footer_keys(&snapshot, &all, &keys)),
            vec![(1, FooterTone::Red), (3, FooterTone::Blue), (4, FooterTone::Yellow), (2, FooterTone::Plain)]);
        snapshot.page_id = "favorites".into();
        assert_eq!(tone(classic_footer_keys(&snapshot, &all, &keys)),
            vec![(1, FooterTone::Red), (3, FooterTone::Blue), (5, FooterTone::Yellow), (2, FooterTone::Plain)]);
        let only_description = vec![(2, "Description".to_owned())];
        assert_eq!(tone(classic_footer_keys(&snapshot, &only_description, &keys)), vec![(2, FooterTone::Red)]);
        assert!(classic_footer_keys(&snapshot, &[], &keys).is_empty());
    }

}
