//! Classic presentation of the existing More form. No duplicate action model.
use super::MoreMenuState;
use plx_machine::machine::{Measure, UiMode};
use plx_platform::i18n::msg;
use plx_ui::{classic::Layout, page_header::{FooterKey, HeaderItem, HeaderRow, PageHeader, PageHeaderSpec, SharedRow, Slot},
    page_stack::TITLE_KEY, screen::ClipScope, text_view::TextView, theme, Painter, Rect};
use std::hash::{Hash, Hasher};

#[derive(Default)]
pub(super) struct Chrome { footer: SharedRow<HeaderRow> }

impl MoreMenuState {
    fn classic_footer(&self, measure: &dyn Measure) -> std::rc::Rc<HeaderRow> {
        let label = if self.pages.is_empty() { msg::settings_login_close() } else { msg::settings_back() };
        let width = Rect::FULL.w;
        let size = theme::size::BODY;
        let mut key = std::collections::hash_map::DefaultHasher::new();
        (label, width.to_bits(), size, measure.font_scale()).hash(&mut key);
        measure.width_str(label, size, false).to_bits().hash(&mut key);
        self.classic_chrome.footer.get(key.finish(), || {
            let page = PageHeader::new(PageHeaderSpec { left: vec![HeaderItem {
                label: label.into(), focusable: true, ..Default::default()
            }], ..Default::default() }, &[], measure);
            let layout = page.layout(width, measure);
            let stops = layout.items.iter().enumerate().filter_map(|(index, item)|
                (item.slot == Slot::Left(0)).then_some((TITLE_KEY, index))).collect();
            HeaderRow::new(page, layout, stops).with_classic_footer(width, size,
                &[(TITLE_KEY, vec![FooterKey::plain("✖")])], measure)
        })
    }

    pub(super) fn classic_footer_rect(&self, measure: &dyn Measure) -> Option<Rect> {
        let layout = Layout::new(Rect::FULL, measure, false);
        let (dx, dy) = layout.header_offset();
        self.classic_footer(measure).plate(TITLE_KEY)
            .map(|r| Rect::new(r.x + dx, r.y + dy, r.w, r.h))
    }

    pub(super) fn paint_classic(&self, p: Painter, measure: &dyn Measure) {
        self.panel_rect_for_mode(UiMode::Classic, measure);
        let layout = Layout::new(Rect::FULL, measure, false);
        plx_ui::classic::ground(p, Rect::FULL);
        plx_ui::classic::caption(p, layout, self.form.table.title().unwrap_or_else(|| msg::classic_player_menu()), measure);
        self.form.table.draw(p, layout.list, measure);
        {
            let _clip = ClipScope::open_in(p, layout.details);
            let selected = usize::try_from(self.form.table.sel).ok()
                .and_then(|index| self.form.table.sections.iter().flat_map(|section| &section.rows).nth(index));
            let mut flow = plx_ui::vstack::VFlow::new(layout.details.y, theme::space::MD);
            if let Some(row) = selected {
                for (text, size, ink) in [
                    (row.label.as_str(), theme::size::BODY, plx_ui::palette::color(plx_ui::palette::Role::Text, theme::CLASSIC_TEXT)),
                    (row.detail.as_str(), theme::size::CAPTION, plx_ui::palette::color(plx_ui::palette::Role::Muted, theme::CLASSIC_SECONDARY)),
                ] {
                    if text.is_empty() { continue; }
                    let view = TextView::new(text, size, ink).with_measure(measure).break_long_words();
                    if let Some(span) = flow.push(plx_ui::hstack::Item::new(view.measure_h(layout.details.w))) {
                        view.draw(p, Rect::new(layout.details.x, span.y, layout.details.w, span.h));
                    }
                }
            }
            // Geometry acknowledgements previously lived in a section heading.
            // The menu omits heading chrome, not the pending/refused outcome.
            let status = if self.geometry_pending { msg::widgets_picture_applying() }
                else if self.geometry_failed { msg::widgets_picture_failed() } else { "" };
            if !status.is_empty() {
                let view = TextView::new(status, theme::size::CAPTION, plx_ui::palette::color(plx_ui::palette::Role::Muted, theme::CLASSIC_SECONDARY)).with_measure(measure);
                if let Some(span) = flow.push(plx_ui::hstack::Item::new(view.measure_h(layout.details.w))) {
                    view.draw(p, Rect::new(layout.details.x, span.y, layout.details.w, span.h));
                }
            }
        }
        let (dx, dy) = layout.header_offset();
        self.classic_footer(measure).draw(p.translate(dx, dy), 0.0, &plx_ui::widgets::TabStrip::new(), 1.0, None);
    }
}
