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
        let current = self.catalog_keys.iter().position(|candidate| *candidate == key)?;
        if self.grid_active() {
            let index = plx_ui::screen::grid_page_index(current, self.catalog_keys.len(),
                media::columns(self.card_presentation?), 2, direction)?;
            return self.snapshot.rows.get(index).filter(|row| row.enabled)
                .and_then(|_| self.catalog_keys.get(index)).copied();
        }
        let selected = self.form.index_of_key(RowKey(key))?;
        let index = self.form.table.catalogue_next(selected as i32, direction, Some(self.table_frame().h))?;
        let target = self.form.key_at(usize::try_from(index).ok()?)?.0;
        // A page jump stays within the catalogue, not its footer controls.
        let target = if self.catalog_keys.contains(&target) { target }
            else if direction > 0 { *self.catalog_keys.last()? }
            else { *self.catalog_keys.first()? };
        self.form.index_of_key(RowKey(target)).and_then(|i| self.form.activate(i)).map(|_| target)
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
                if edge == Edge::Down && !self.activate_shortcut("nav:guide", fx) {
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
