//! RAM navigation-stack memory only, never a cold-start checkpoint.
//! Runtime remains the catalogue and persisted-profile owner.
use super::*;

#[derive(Clone, Debug)]
pub struct Memory {
    pub(super) page_id: String,
    pub(super) source: Option<String>,
    pub(super) shelves: Vec<(media::Section, f32)>,
    pub(super) focus: Option<String>,
    pub(super) selected: Option<String>,
    pub(super) scroll: f32,
    pub(super) keys: HashMap<String, u32>,
    pub(super) next_key: u32,
}
impl Memory {
    pub(super) fn capture(screen: &OttScreen, focus: Option<FocusKey<u32>>) -> Self {
        let focus = focus.filter(|key| key.entry == screen.entry).and_then(|key|
            screen.keys.iter().find(|(_, elem)| **elem == key.elem).map(|(id, _)| id.clone()));
        Self { page_id: screen.snapshot.page_id.clone(), source: (!screen.shelves.is_empty()).then(|| screen.snapshot.shelf_context.clone()).flatten(),
            shelves: screen.card_stack.memory().shelves, focus,
            selected: screen.media.and_then(|index| screen.snapshot.rows.get(index)).map(|row| row.id.clone()),
            scroll: screen.card_stack.scroll(), keys: screen.keys.clone(), next_key: screen.next_key }
    }
    pub(crate) fn write(&self, out: &mut Canon) {
        out.option(self.source.as_ref(), |out, source| { out.str(source); });
        out.seq(self.shelves.len());
        for (section, scroll) in &self.shelves {
            if let media::Section::Shelf(group) = section { out.u32(*group).f32(*scroll); }
        }
        out.str(&self.page_id).option(self.focus.as_ref(), |out, id| { out.str(id); });
        out.option(self.selected.as_ref(), |out, id| { out.str(id); });
        out.f32(self.scroll).u32(self.next_key).seq(self.keys.len());
        let mut keys: Vec<_> = self.keys.iter().collect();
        keys.sort_unstable_by_key(|(id, _)| *id);
        for (id, key) in keys { out.str(id).u32(*key); }
    }
}
