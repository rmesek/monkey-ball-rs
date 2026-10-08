//! Menu building blocks shared by the menus and the pause screen: a vertical, scrollable list of big buttons
//! that works with keys / gamepad (step and confirm), mouse (hover, click, wheel) and touch (tap, drag to
//! scroll). Pure layout and input maths plus a draw helper; no state beyond [`ListState`].

use crate::input::{Action, InputState};
use crate::text::{with_alpha, Align, Color, FontId, Painter, WHITE};
use crate::ui::{Layout, Rect};

/// Pointer travel (in pixels, scaled by the display density) after which a press is a drag, not a tap.
const DRAG_SLOP_DP: f32 = 10.0;

/// Where a list sits and how big its rows are.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ListLayout {
    /// Visible window of the list; rows outside it are not drawn and cannot be hit.
    pub area: Rect,
    pub item_h: f32,
    pub gap: f32,
    pub count: usize,
    pub drag_slop: f32,
}

impl ListLayout {
    /// A column of `count` rows below a title block, centred in the safe area. `title_h` is the fraction of
    /// the safe height reserved above the list.
    pub fn column(layout: &Layout, count: usize, title_h: f32) -> Self {
        let safe = layout.safe_rect();
        let scale = layout.hud_scale();
        let item_h = (72.0 * scale).max(layout.min_tap() * 1.15);
        let gap = item_h * 0.18;
        let w = (safe.w * 0.92).min(760.0 * scale);
        let top = safe.y + safe.h * title_h;
        let bottom_margin = safe.h * 0.04;
        Self {
            area: Rect {
                x: safe.x + (safe.w - w) / 2.0,
                y: top,
                w,
                h: (safe.h - (top - safe.y) - bottom_margin).max(item_h),
            },
            item_h,
            gap,
            count,
            drag_slop: DRAG_SLOP_DP * layout.dpi_scale,
        }
    }

    fn pitch(&self) -> f32 {
        self.item_h + self.gap
    }

    pub fn content_height(&self) -> f32 {
        (self.count as f32 * self.pitch() - self.gap).max(0.0)
    }

    pub fn max_scroll(&self) -> f32 {
        (self.content_height() - self.area.h).max(0.0)
    }

    /// Row rectangle at `scroll`. Short lists are centred vertically in the area.
    pub fn item_rect(&self, index: usize, scroll: f32) -> Rect {
        let slack = (self.area.h - self.content_height()).max(0.0);
        Rect {
            x: self.area.x,
            y: self.area.y + slack * 0.35 + index as f32 * self.pitch() - scroll,
            w: self.area.w,
            h: self.item_h,
        }
    }

    /// True when the row is fully inside the visible window.
    pub fn is_visible(&self, index: usize, scroll: f32) -> bool {
        let r = self.item_rect(index, scroll);
        r.y >= self.area.y - 0.5 && r.y + r.h <= self.area.y + self.area.h + 0.5
    }

    pub fn item_at(&self, pos: (f32, f32), scroll: f32) -> Option<usize> {
        (0..self.count)
            .find(|i| self.is_visible(*i, scroll) && self.item_rect(*i, scroll).contains(pos))
    }
}

#[derive(Clone, Copy, Debug)]
struct Press {
    start: (f32, f32),
    scroll: f32,
    item: Option<usize>,
    dragged: bool,
}

/// Selection and scroll position of one list.
#[derive(Clone, Debug, Default)]
pub struct ListState {
    pub selected: usize,
    pub scroll: f32,
    press: Option<Press>,
    last_pointer: Option<(f32, f32)>,
}

impl ListState {
    pub fn new(selected: usize) -> Self {
        Self {
            selected,
            ..Self::default()
        }
    }

    fn step(&mut self, l: &ListLayout, dir: i32, enabled: &dyn Fn(usize) -> bool) {
        if l.count == 0 {
            return;
        }
        let n = l.count as i32;
        let mut i = self.selected as i32;
        for _ in 0..n {
            i = (i + dir).rem_euclid(n);
            if enabled(i as usize) {
                self.selected = i as usize;
                break;
            }
        }
        self.reveal(l);
    }

    /// Scroll just enough to show the selected row.
    pub fn reveal(&mut self, l: &ListLayout) {
        let r = l.item_rect(self.selected, self.scroll);
        if r.y < l.area.y {
            self.scroll -= l.area.y - r.y;
        } else if r.y + r.h > l.area.y + l.area.h {
            self.scroll += r.y + r.h - (l.area.y + l.area.h);
        }
        self.scroll = self.scroll.clamp(0.0, l.max_scroll());
    }

    /// Process this frame's input. Returns the row that was activated (confirm, click or tap), if any.
    /// Rows for which `enabled` is false can be neither selected nor activated.
    pub fn update(
        &mut self,
        input: &InputState,
        l: &ListLayout,
        enabled: &dyn Fn(usize) -> bool,
    ) -> Option<usize> {
        if l.count == 0 {
            return None;
        }
        self.selected = self.selected.min(l.count - 1);
        if !enabled(self.selected) {
            self.step(l, 1, enabled);
        }
        let (_, dy) = input.menu_nav();
        if dy != 0 {
            self.step(l, dy, enabled);
        }
        if input.scroll() != 0.0 {
            self.scroll = (self.scroll - input.scroll()).clamp(0.0, l.max_scroll());
        }
        self.scroll = self.scroll.clamp(0.0, l.max_scroll());

        let pointer = input.pointer();
        let mut activated = None;
        if let Some(pos) = pointer {
            if input.pointer_pressed() {
                self.press = Some(Press {
                    start: pos,
                    scroll: self.scroll,
                    item: l.item_at(pos, self.scroll),
                    dragged: false,
                });
            }
            if let Some(press) = &mut self.press {
                let dy = pos.1 - press.start.1;
                if dy.abs() > l.drag_slop {
                    press.dragged = true;
                }
                if press.dragged && l.max_scroll() > 0.0 {
                    self.scroll = (press.scroll - dy).clamp(0.0, l.max_scroll());
                }
            }
            // Hovering selects, but only when the pointer actually moved, so it does not fight the keys.
            let moved = self.last_pointer != Some(pos);
            let dragging = self.press.is_some_and(|p| p.dragged);
            if (moved || input.pointer_pressed()) && !dragging {
                if let Some(i) = l.item_at(pos, self.scroll).filter(|i| enabled(*i)) {
                    self.selected = i;
                }
            }
            if input.pointer_released() {
                if let Some(press) = self.press.take() {
                    let hit = l.item_at(pos, self.scroll);
                    if !press.dragged && hit.is_some() && hit == press.item {
                        activated = hit.filter(|i| enabled(*i));
                    }
                }
            }
        }
        if !input.pointer_held() {
            self.press = None;
        }
        self.last_pointer = pointer;

        if activated.is_none() && input.pressed(Action::Confirm) && enabled(self.selected) {
            activated = Some(self.selected);
        }
        if let Some(i) = activated {
            self.selected = i;
        }
        activated
    }
}

/// How one row looks.
pub struct Row<'a> {
    pub label: &'a str,
    /// Right-aligned second column (a value, a time, "LOCKED").
    pub detail: Option<&'a str>,
    pub enabled: bool,
}

const PANEL: Color = [0.05, 0.08, 0.22, 0.78];
const PANEL_SELECTED: Color = [0.12, 0.38, 0.85, 0.92];
const OUTLINE: Color = [1.0, 0.86, 0.25, 1.0];
const DISABLED: Color = [0.6, 0.6, 0.66, 0.7];

/// Largest text scale at which `text` fits `width` and `height` in `font`.
pub fn fit_scale(p: &Painter, font: FontId, text: &str, width: f32, height: f32) -> f32 {
    let f = p.ui.font(font).p;
    let by_height = height / f.line_height;
    let natural = f.measure(text, 1.0).max(1.0);
    by_height.min(width / natural)
}

/// Draw the visible rows of a list.
pub fn draw_list<'a>(
    p: &mut Painter,
    l: &ListLayout,
    state: &ListState,
    row: &dyn Fn(usize) -> Row<'a>,
) {
    for i in 0..l.count {
        if !l.is_visible(i, state.scroll) {
            continue;
        }
        let r = l.item_rect(i, state.scroll);
        let item = row(i);
        let selected = i == state.selected && item.enabled;
        p.rect(r, if selected { PANEL_SELECTED } else { PANEL });
        if selected {
            p.frame(r, (r.h * 0.05).max(2.0), OUTLINE);
        }
        let color = if item.enabled { WHITE } else { DISABLED };
        let pad = r.h * 0.35;
        let text_h = r.h * 0.46;
        let detail_w = item
            .detail
            .map(|d| {
                let s = fit_scale(p, FontId::Asc20, d, r.w * 0.4, text_h * 0.8);
                p.text(
                    FontId::Asc20,
                    d,
                    r.x + r.w - pad,
                    r.y + r.h / 2.0,
                    s,
                    s,
                    with_alpha(color, 0.9),
                    Align::Right,
                );
                p.measure(FontId::Asc20, d, s) + pad
            })
            .unwrap_or(0.0);
        let avail = r.w - 2.0 * pad - detail_w;
        let s = fit_scale(p, FontId::Asc32, item.label, avail, text_h);
        if item.detail.is_some() {
            p.text(
                FontId::Asc32,
                item.label,
                r.x + pad,
                r.y + r.h / 2.0,
                s,
                s,
                color,
                Align::Left,
            );
        } else {
            p.text(
                FontId::Asc32,
                item.label,
                r.x + r.w / 2.0,
                r.y + r.h / 2.0,
                s,
                s,
                color,
                Align::Center,
            );
        }
    }
    // Scroll hints: a thin bar on the right edge.
    if l.max_scroll() > 0.0 {
        let track = Rect {
            x: l.area.x + l.area.w + l.item_h * 0.08,
            y: l.area.y,
            w: (l.item_h * 0.06).max(3.0),
            h: l.area.h,
        };
        p.rect(track, [1.0, 1.0, 1.0, 0.15]);
        let frac = l.area.h / l.content_height();
        let pos = state.scroll / l.max_scroll();
        p.rect(
            Rect {
                y: track.y + (track.h - track.h * frac) * pos,
                h: track.h * frac,
                ..track
            },
            [1.0, 1.0, 1.0, 0.6],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Axis;
    use crate::ui::Insets;

    fn layout(count: usize) -> ListLayout {
        ListLayout::column(
            &Layout::new((1280, 720), Insets::default(), 1.0),
            count,
            0.3,
        )
    }

    const ALL: &dyn Fn(usize) -> bool = &|_| true;

    #[test]
    fn rows_are_tap_sized_and_inside_the_safe_area() {
        for (size, dpi) in [((1280, 720), 1.0), ((720, 1280), 3.0), ((320, 240), 1.0)] {
            let lay = Layout::new(size, Insets::default(), dpi);
            let l = ListLayout::column(&lay, 4, 0.3);
            assert!(l.item_h >= lay.min_tap());
            let r = l.item_rect(0, 0.0);
            assert!(r.x >= 0.0 && r.x + r.w <= size.0 as f32);
        }
    }

    #[test]
    fn keys_step_wrap_and_skip_disabled_rows() {
        let l = layout(4);
        let mut s = ListState::new(0);
        let mut input = InputState::new();
        input.set_action(Action::LookDown, true);
        let enabled: &dyn Fn(usize) -> bool = &|i| i != 1;
        assert_eq!(s.update(&input, &l, enabled), None);
        assert_eq!(s.selected, 2);
        input.end_frame();
        input.set_action(Action::LookDown, false);
        input.set_action(Action::LookUp, true);
        s.update(&input, &l, enabled);
        assert_eq!(s.selected, 0);
        input.end_frame();
        input.set_action(Action::LookUp, false);
        input.set_action(Action::MoveForward, true);
        s.update(&input, &l, enabled);
        assert_eq!(s.selected, 3, "wraps to the end");
        input.end_frame();
        input.set_action(Action::Confirm, true);
        assert_eq!(s.update(&input, &l, enabled), Some(3));
    }

    #[test]
    fn stick_flick_moves_one_row() {
        let l = layout(3);
        let mut s = ListState::new(0);
        let mut input = InputState::new();
        input.set_axis(Axis::MoveY, -1.0);
        s.update(&input, &l, ALL);
        input.end_frame();
        s.update(&input, &l, ALL);
        assert_eq!(s.selected, 1);
    }

    #[test]
    fn tap_activates_and_drag_scrolls_instead() {
        let l = layout(30);
        assert!(l.max_scroll() > 0.0);
        let mut s = ListState::new(0);
        let mut input = InputState::new();
        let c = l.item_rect(2, 0.0).center();
        input.set_pointer(Some(c));
        input.set_pointer_down(true);
        assert_eq!(s.update(&input, &l, ALL), None);
        input.end_frame();
        input.set_pointer_down(false);
        assert_eq!(s.update(&input, &l, ALL), Some(2));
        input.end_frame();

        // Press, drag up by two rows, release: scrolled, nothing activated.
        input.set_pointer_down(true);
        s.update(&input, &l, ALL);
        input.end_frame();
        input.set_pointer(Some((c.0, c.1 - 2.0 * l.pitch())));
        s.update(&input, &l, ALL);
        input.end_frame();
        input.set_pointer_down(false);
        assert_eq!(s.update(&input, &l, ALL), None);
        assert!((s.scroll - 2.0 * l.pitch()).abs() < 1.0);
    }

    #[test]
    fn selection_scrolls_into_view_and_wheel_scrolls() {
        let l = layout(30);
        let mut s = ListState::new(0);
        let mut input = InputState::new();
        input.set_action(Action::LookUp, true);
        s.update(&input, &l, ALL);
        assert_eq!(s.selected, 29);
        assert!(l.is_visible(29, s.scroll));
        input.end_frame();
        input.add_scroll(10_000.0);
        s.update(&input, &l, ALL);
        assert_eq!(s.scroll, 0.0);
    }

    #[test]
    fn disabled_rows_cannot_be_clicked() {
        let l = layout(3);
        let mut s = ListState::new(0);
        let mut input = InputState::new();
        input.set_pointer(Some(l.item_rect(1, 0.0).center()));
        input.set_pointer_down(true);
        let enabled: &dyn Fn(usize) -> bool = &|i| i != 1;
        s.update(&input, &l, enabled);
        input.end_frame();
        input.set_pointer_down(false);
        assert_eq!(s.update(&input, &l, enabled), None);
        assert_eq!(s.selected, 0);
    }
}
