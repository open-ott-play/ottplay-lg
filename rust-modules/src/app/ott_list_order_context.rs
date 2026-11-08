//! Contextual list operations use the captured row and the parent's single
//! Saved writer. No browser focus, playback selection or provider IO is read.
use super::*;

fn key(menu: &Menu, operation: &str) -> OttAction {
    OttAction::EditSetting(format!("order.{}.{operation}", menu.revision))
}
fn durable_source(menu: &Menu) -> bool {
    menu.context.source.durable().is_some()
}
fn selected<'a>(runtime: &Runtime, menu: &'a Menu) -> Option<&'a str> {
    let id = menu.item.as_deref()?;
    runtime.organization_projection()?.ordered_ids(&menu.context.category)
        .iter().any(|member| member == id).then_some(id)
}
fn alphabetical(runtime: &Runtime, menu: &Menu) -> Result<bool, &'static str> {
    let source = Source::new(&menu.context.profile).map_err(invalid)?;
    Ok(Organization::read(&runtime.saved.settings).map_err(invalid)?.alphabetical(&source))
}

pub(super) fn rows(runtime: &Runtime, menu: &Menu, usable: bool) -> Vec<OttRow> {
    if !durable_source(menu) { return Vec::new(); }
    let mut rows = Vec::new();
    if selected(runtime, menu).is_some() {
        let named = category(runtime, &menu.context).is_ok_and(|id|
            !id.is_empty() && id != catalog_organization::ALL_ID);
        let movable = named && runtime.organization_rows(&menu.context.category).len() > 1
            && alphabetical(runtime, menu).is_ok_and(|enabled| !enabled);
        for (operation, title, available) in [
            ("up", msg::ott_organization_up(), movable),
            ("down", msg::ott_organization_down(), movable),
            ("remove", msg::ott_organization_remove(), named),
            ("add", msg::ott_order_add_to_category(), true),
        ] {
            let mut item = row(format!("order.{operation}"), title.into(), String::new(), key(menu, operation), false);
            item.enabled = usable && available;
            rows.push(item);
        }
    }
    let mut sort = row("order.sort".into(),
        if alphabetical(runtime, menu).unwrap_or(false) { msg::ott_order_sort_as_is() }
        else { msg::ott_order_sort_alphabetical() }.into(),
        String::new(), key(menu, "sort"), false);
    sort.enabled = usable;
    rows.push(sort);
    rows
}

pub(super) fn destinations(runtime: &Runtime, menu: &Menu, usable: bool) -> Vec<OttRow> {
    let Some(projection) = runtime.organization_projection() else { return Vec::new(); };
    let item = selected(runtime, menu);
    projection.categories.iter().filter(|category| category.id != catalog_organization::ALL_ID)
        .enumerate().map(|(index, category)| {
            let mut row = row(format!("order.category:{}", category.id), category.label.clone(),
                String::new(), key(menu, &format!("category:{index}")), false);
            row.enabled = usable && item.is_some_and(|id| !category.members.iter().any(|member| member == id));
            row
        }).collect()
}

fn picker(runtime: &mut Runtime, mut menu: Menu, choosing: bool) -> Result<(), &'static str> {
    menu.revision = runtime.organization_next_revision()?;
    menu.choosing_category = choosing;
    menu.rebase = true;
    runtime.organization.order_menu = Some(menu);
    runtime.status.clear();
    runtime.changed();
    Ok(())
}

pub(super) fn action(runtime: &mut Runtime, action: &OttAction) -> Option<Result<(), &'static str>> {
    if matches!(action, OttAction::Back | OttAction::CloseActions)
        && runtime.page == "actions"
        && runtime.organization.order_menu.as_ref().is_some_and(|menu| menu.choosing_category)
    {
        let menu = runtime.organization.order_menu.clone()?;
        // Cleanup never requires mutation authority. A stale/pending picker
        // falls through to the parent's ordinary close without reviving it.
        if !current(runtime, &menu.context, false) { return None; }
        return Some(picker(runtime, menu, false));
    }
    let OttAction::EditSetting(value) = action else { return None; };
    let value = value.strip_prefix("order.")?;
    Some((|| {
        let (revision, operation) = value.split_once('.').ok_or(CHANGED)?;
        let revision = revision.parse::<u64>().map_err(|_| CHANGED)?;
        let menu = runtime.organization.order_menu.clone().filter(|menu|
            menu.revision == revision && runtime.page == "actions"
                && current(runtime, &menu.context, false) && durable_source(menu)
        ).ok_or(CHANGED)?;
        runtime.require_parental(parental::Target::Settings)?;
        let item = selected(runtime, &menu).map(str::to_owned);
        let category = category(runtime, &menu.context)?;
        if menu.choosing_category {
            let index = operation.strip_prefix("category:").ok_or(CHANGED)?
                .parse::<usize>().map_err(|_| CHANGED)?;
            let projection = runtime.organization_projection().ok_or(CHANGED)?;
            let destination = projection.categories.iter()
                .filter(|category| category.id != catalog_organization::ALL_ID)
                .nth(index).ok_or(CHANGED)?.id.clone();
            return apply_context(runtime, menu, Operation::AddMember {
                category: destination, item: item.ok_or(CHANGED)?,
            });
        }
        let operation = match operation {
            "up" | "down" => {
                if category.is_empty() || category == catalog_organization::ALL_ID
                    || alphabetical(runtime, &menu)? { return Err(UNAVAILABLE); }
                Operation::MoveMember {
                    category, item: item.ok_or(CHANGED)?,
                    direction: if operation == "up" { Direction::Up } else { Direction::Down },
                }
            }
            "remove" => {
                if category.is_empty() || category == catalog_organization::ALL_ID { return Err(UNAVAILABLE); }
                Operation::RemoveMember { category, item: item.ok_or(CHANGED)? }
            }
            "add" => {
                item.ok_or(CHANGED)?;
                return picker(runtime, menu, true);
            }
            "sort" => Operation::SetAlphabetical { enabled: !alphabetical(runtime, &menu)? },
            _ => return Err(CHANGED),
        };
        apply_context(runtime, menu, operation)
    })())
}

// Context actions use the ordinary EditSetting navigation boundary. Authorize
// only that action's one epoch rebase, including its matching pending write.
fn apply_context(runtime: &mut Runtime, mut menu: Menu, operation: Operation) -> Result<(), &'static str> {
    menu.rebase = true;
    let revision = menu.revision;
    apply_saved(runtime, menu, operation)?;
    if runtime.organization.order_pending.as_ref().is_some_and(|pending| pending.menu.revision == revision) {
        if let Some(menu) = runtime.organization.order_menu.as_mut().filter(|menu| menu.revision == revision) {
            menu.rebase = true;
        }
    }
    Ok(())
}

/// Preserve the captured item across a reorder/add; removal uses the next
/// surviving visible item, or the preceding one at the end. This is only a
/// proposed focus until the parent's exact, still-addressed Saved ACK succeeds.
pub(super) fn focus_after(runtime: &Runtime, menu: &Menu,
    projection: &catalog_organization::Projection) -> Option<String>
{
    let selected = menu.item.as_ref()?;
    let after = projection.ordered_ids(&menu.context.category);
    if after.contains(selected) { return Some(selected.clone()); }
    let search = runtime.search_filter();
    let before: Vec<&str> = runtime.organization_rows(&menu.context.category).into_iter()
        .filter(|(_, entry)| search(&entry.title)).map(|(_, entry)| entry.id.as_str()).collect();
    let index = before.iter().position(|id| *id == selected.as_str())?;
    before[index + 1..].iter().copied().chain(before[..index].iter().rev().copied())
        .find(|id| after.iter().any(|member| member.as_str() == *id)).map(str::to_owned)
}
