//! Physical remote list actions reuse the ordinary form and focus owners.
use super::*;
use plx_ui::consts::{classify_input, remote_button, Key as RemoteKey, RemoteButton};
use plx_ui::screen::{Enter, FocusTarget};

impl OttScreen {
    pub(super) fn settings_save_action(&self) -> Option<OttAction> {
        // Both identifiers belong to the published draft. Never construct an
        // action from a label or infer a nonce from a child picker.
        let save_key = |key: &str| key.starts_with("settings.draft:") && key.ends_with(":save");
        let mut rows = self.snapshot.rows.iter().filter(|row| save_key(&row.id)
            || matches!(&row.action, OttAction::EditSetting(key) if save_key(key)));
        let row = rows.next()?;
        if rows.next().is_some() || !row.enabled { return None; }
        let OttAction::EditSetting(key) = &row.action else { return None; };
        if key != &row.id { return None; }
        let nonce = key.strip_prefix("settings.draft:")?.strip_suffix(":save")?;
        let value = nonce.parse::<u64>().ok()?;
        if value == 0 || value.to_string() != nonce { return None; }
        Some(row.action.clone())
    }

    pub(super) fn remote_page_key(&self, key: u32, direction: i32) -> Option<u32> {
        if self.grid_active() && !self.shelves.is_empty() { return self.shelf_page_key(key, direction); }
        let current = self.catalog_keys.iter().position(|candidate| *candidate == key)?;
        let target = if self.grid_active() {
            plx_ui::screen::grid_page_index(current, self.catalog_keys.len(),
                media::columns(self.card_presentation?), 2, direction)?
        } else {
            let selected = self.form.index_of_key(RowKey(key))?;
            let index = self.form.table.catalogue_next(selected as i32, direction, Some(self.table_frame().h))?;
            let target = self.form.key_at(usize::try_from(index).ok()?)?.0;
            // A page jump stays within the catalogue, not its footer controls.
            self.catalog_keys.iter().position(|key| *key == target)
                .unwrap_or(if direction > 0 { self.catalog_keys.len() - 1 } else { 0 })
        };
        // Disabled rows remain focusable for ordinary navigation, but cannot
        // swallow every repeated page key at the same calculated destination.
        // Prefer the next enabled row in the requested direction; at an edge,
        // fall back toward the current row without wrapping or moving past it.
        let target = if direction > 0 { target.max(current) } else { target.min(current) };
        // Installation preserves catalogue order in the form, but can move
        // interleaved snapshot controls into a separate section. Match its
        // actual keys in one pass instead of indexing snapshot rows or doing
        // a linear key lookup for each disabled candidate.
        let mut catalog = self.catalog_keys.iter().copied().enumerate().peekable();
        let mut preferred = None;
        let mut fallback = None;
        for row in 0..self.form.table.n_rows() {
            let Some(binding) = self.form.binding_at(row as usize) else { continue; };
            let Some(&(index, key)) = catalog.peek() else { break; };
            if binding.key.0 != key { continue; }
            catalog.next();
            if binding.disabled { continue; }
            if direction > 0 {
                if index >= target { return Some(key); }
                if index >= current { fallback = Some(key); }
            } else if direction < 0 {
                if index <= target { preferred = Some(key); }
                else if index <= current && fallback.is_none() { fallback = Some(key); }
            } else if index == current { return Some(key); }
        }
        preferred.or(fallback)
    }

    pub(super) fn remote_input<H: AppLike<Memory = PageMemory>>(&mut self,
        input: &plx_machine::machine::InputEvent<u32>, cx: &Cx<'_, H>, fx: &mut Effects<'_, H>) -> Option<Handled> {
        let InputKind::Key { key, sym, wcode, edge, at_edge } = input.kind else { return None; };
        if self.editor.is_some() || self.reading || self.browse { return None; }
        if self.classic_mode && self.snapshot.page_id == "settings" {
            let classified = classify_input(key, sym, wcode);
            // Digits use their ASCII sym, not the unrelated SDL scancode.
            let save = matches!(classified, RemoteKey::Play | RemoteKey::Pause)
                || (classified == RemoteKey::Other && (sym == u32::from(b'0')
                    || remote_button(sym, wcode) == Some(RemoteButton::Green)));
            if save {
                if edge == Edge::Down && !at_edge {
                    if let Some(action) = self.settings_save_action() { self.send(action, fx); }
                }
                return Some(Handled::Yes);
            }
        }
        // The Classic TV list uses the same direct category shortcut as the
        // desktop player: Play/Pause opens Category selection, not a second
        // navigation form and not transport for the video behind the list.
        if self.classic_mode && matches!(self.snapshot.page_id.split(':').next(), Some("channels" | "favorites"))
            && matches!(classify_input(key, sym, wcode), RemoteKey::Play | RemoteKey::Pause | RemoteKey::PlayPause)
        {
            if edge == Edge::Down && !at_edge { self.activate_shortcut("nav:categories", fx); }
            return Some(Handled::Yes);
        }
        let button = remote_button(sym, wcode)?;
        // Blue follows the shared Categories action advertised by the footer,
        // including provider screens. Channel paging stays catalogue-only.
        if button != RemoteButton::Blue
            && !matches!(self.snapshot.page_id.split(':').next(), Some("channels" | "favorites" | "categories")) {
            return None;
        }
        if edge == Edge::Up || at_edge { return Some(Handled::Yes); }
        let direction = match button {
            RemoteButton::ChannelUp => -1,
            RemoteButton::ChannelDown => 1,
            RemoteButton::Red => {
                if edge == Edge::Down && self.snapshot.page_id.split(':').next() == Some("categories") {
                    self.activate_shortcut("nav:catalog", fx);
                } else if edge == Edge::Down && !self.activate_shortcut("nav:guide", fx) {
                    self.activate_shortcut("nav:description", fx);
                }
                return Some(Handled::Yes);
            }
            RemoteButton::Yellow | RemoteButton::Blue => {
                if edge == Edge::Down {
                    self.activate_shortcut(if button == RemoteButton::Yellow {
                        if self.classic_mode && self.snapshot.page_id.split(':').next() == Some("favorites") {
                            "nav:favorite"
                        } else { "nav:actions" }
                    } else { "nav:categories" }, fx);
                }
                return Some(Handled::Yes);
            }
            RemoteButton::Green => return Some(Handled::Yes),
        };
        if let Some(key) = cx.focus.current.filter(|key| key.entry == self.entry)
            .and_then(|key| self.remote_page_key(key.elem, direction)) {
            self.restore_memory = None;
            if cx.focus.current.is_some_and(|current| current.elem != key) {
                self.search_return.pending = None;
                self.search_return.from_actions = None;
            }
            fx.push(Fx::Deliver(MachineId::Instance(self.instance), Delivery::Screen(
                ScreenEvent::Enter(Enter::Fresh { focus: FocusTarget::Elem(FocusKey { entry: self.entry, elem: key }) }))));
        }
        Some(Handled::Yes)
    }
}
