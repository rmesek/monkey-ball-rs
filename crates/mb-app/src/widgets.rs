//! Menu building blocks shared by the menus and the pause screen: a vertical, scrollable list of big buttons
//! and a Back button. Everything works with keys / gamepad (step and confirm), mouse (hover, click, wheel,
//! click-drag) and touch alone (tap, drag to scroll with a short glide, tappable page arrows).
//!
//! Gestures, for mouse and touch through the same code:
//! * a **tap** is a press and release on the same row with little movement ([`DRAG_SLOP_DP`]), however long
//!   the press lasts (people press slowly on a phone); only a tap activates a row;
//! * a press that moves further is a **drag**: the list follows the finger and the release activates
//!   nothing; a quick drag glides on briefly after the release;
//! * a list that does not fit shows a **rail** on its right: a scroll bar between two arrow buttons that
//!   page the list (and repeat while held).
//!
//! Pure layout and input maths plus draw helpers; no state beyond [`ListState`] and [`Tap`].

use crate::input::{Action, InputState};
use crate::text::{Align, Color, FontId, Painter, WHITE, with_alpha};
use crate::ui::{Layout, Rect};

/// Pointer travel (in density-independent pixels) after which a press is a drag, not a tap.
const DRAG_SLOP_DP: f32 = 10.0;
/// Glide after a drag: speed below which it stops (pixels per second per density unit) and how fast it
/// decays (per second).
const GLIDE_MIN_SPEED_DP: f32 = 40.0;
const GLIDE_DECAY: f32 = 5.0;
/// Fastest glide, in list heights per second.
const GLIDE_MAX_AREAS_PER_SECOND: f32 = 6.0;
/// A held page arrow repeats after this long, then at this interval.
const RAIL_REPEAT_DELAY: f32 = 0.4;
const RAIL_REPEAT_INTERVAL: f32 = 0.18;

/// Where a list sits and how big its rows are.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ListLayout {
    /// Visible window of the list; rows are hit and drawn only inside it.
    pub area: Rect,
    pub item_h: f32,
    pub gap: f32,
    pub count: usize,
    pub drag_slop: f32,
    /// Column right of the rows holding the page arrows and the scroll bar; present when the rows do not
    /// all fit.
    pub rail: Option<Rect>,
    /// Pixels per density-independent pixel (glide thresholds).
    dpi: f32,
}

impl ListLayout {
    /// A column of `count` rows below a title block, centred in the safe area. `title_h` is the fraction of
    /// the safe height reserved above the list.
    pub fn column(layout: &Layout, count: usize, title_h: f32) -> Self {
        let safe = layout.safe_rect();
        let scale = layout.hud_scale();
        let item_h = (72.0 * scale).max(layout.min_tap() * 1.15);
        let gap = item_h * 0.18;
        let top = safe.y + safe.h * title_h;
        let bottom_margin = safe.h * 0.04;
        let h = (safe.h - (top - safe.y) - bottom_margin).max(item_h);
        let content = (count as f32 * (item_h + gap) - gap).max(0.0);
        let full_w = (safe.w * 0.92).min(760.0 * scale);
        let scrollable = content > h + 0.5;
        // The rail is a full tap target wide and takes its room from the rows.
        let rail_w = layout.min_tap().min(full_w * 0.25);
        let rail_gap = rail_w * 0.15;
        let rows_w = if scrollable {
            full_w - rail_w - rail_gap
        } else {
            full_w
        };
        let x = safe.x + (safe.w - full_w) / 2.0;
        Self {
            area: Rect {
                x,
                y: top,
                w: rows_w,
                h,
            },
            item_h,
            gap,
            count,
            drag_slop: DRAG_SLOP_DP * layout.dpi_scale,
            rail: scrollable.then_some(Rect {
                x: x + rows_w + rail_gap,
                y: top,
                w: rail_w,
                h,
            }),
            dpi: layout.dpi_scale,
        }
    }

    pub fn pitch(&self) -> f32 {
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

    /// True when any part of the row is inside the visible window.
    pub fn intersects(&self, index: usize, scroll: f32) -> bool {
        let r = self.item_rect(index, scroll);
        r.y + r.h > self.area.y && r.y < self.area.y + self.area.h
    }

    /// The row under `pos`, if `pos` is inside the visible window.
    pub fn item_at(&self, pos: (f32, f32), scroll: f32) -> Option<usize> {
        if !self.area.contains(pos) {
            return None;
        }
        (0..self.count).find(|i| self.item_rect(*i, scroll).contains(pos))
    }

    /// The page-up (`-1`) and page-down (`1`) arrow buttons of the rail.
    pub fn rail_button(&self, dir: i32) -> Option<Rect> {
        let rail = self.rail?;
        let size = rail.w.min(rail.h / 3.0);
        Some(Rect {
            x: rail.x,
            y: if dir < 0 {
                rail.y
            } else {
                rail.y + rail.h - size
            },
            w: rail.w,
            h: size,
        })
    }

    /// How far one press of a page arrow scrolls: a window less one row, so a row of context stays.
    pub fn page(&self) -> f32 {
        let rows = (self.area.h / self.pitch()).floor().max(2.0) - 1.0;
        rows * self.pitch()
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
    /// Scroll speed in pixels per second: measured while dragging, spent gliding after the release.
    velocity: f32,
    /// A page arrow is held: its direction and the time until it repeats.
    rail_hold: Option<(i32, f32)>,
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

    /// Scroll just enough to show the selected row. Called when the selection moves by keys and when a
    /// screen opens; never while the player scrolls by hand (the list would snap back).
    pub fn reveal(&mut self, l: &ListLayout) {
        let r = l.item_rect(self.selected, self.scroll);
        if r.y < l.area.y {
            self.scroll -= l.area.y - r.y;
        } else if r.y + r.h > l.area.y + l.area.h {
            self.scroll += r.y + r.h - (l.area.y + l.area.h);
        }
        self.scroll = self.scroll.clamp(0.0, l.max_scroll());
        self.velocity = 0.0;
    }

    /// True while the list is being dragged or is gliding (the picture changes without input edges).
    pub fn in_motion(&self) -> bool {
        self.velocity != 0.0 || self.press.is_some_and(|p| p.dragged)
    }

    fn scroll_by(&mut self, l: &ListLayout, delta: f32) {
        self.scroll = (self.scroll + delta).clamp(0.0, l.max_scroll());
    }

    /// Process this frame's input (`dt` seconds since the last call). Returns the row that was activated
    /// (confirm, click or tap), if any. Rows for which `enabled` is false can be neither selected nor
    /// activated.
    pub fn update(
        &mut self,
        input: &InputState,
        l: &ListLayout,
        enabled: &dyn Fn(usize) -> bool,
        dt: f32,
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
            self.velocity = 0.0;
            self.scroll_by(l, -input.scroll());
        }
        self.scroll = self.scroll.clamp(0.0, l.max_scroll());

        let pointer = input.pointer();
        let mut activated = None;
        if let Some(pos) = pointer {
            if input.pointer_pressed() {
                self.velocity = 0.0;
                self.rail_hold = [-1, 1]
                    .into_iter()
                    .find(|d| l.rail_button(*d).is_some_and(|r| r.contains(pos)))
                    .map(|d| (d, RAIL_REPEAT_DELAY));
                if let Some((dir, _)) = self.rail_hold {
                    self.scroll_by(l, dir as f32 * l.page());
                } else if l.area.contains(pos) || l.rail.is_some_and(|r| r.contains(pos)) {
                    self.press = Some(Press {
                        start: pos,
                        scroll: self.scroll,
                        item: l.item_at(pos, self.scroll),
                        dragged: false,
                    });
                }
            }
            if let Some((dir, timer)) = &mut self.rail_hold {
                let over = l.rail_button(*dir).is_some_and(|r| r.contains(pos));
                if input.pointer_held() && over {
                    *timer -= dt;
                    if *timer <= 0.0 {
                        *timer += RAIL_REPEAT_INTERVAL;
                        let step = *dir as f32 * l.page();
                        self.scroll_by(l, step);
                    }
                }
            }
            if let Some(press) = &mut self.press {
                let (dx, dy) = (pos.0 - press.start.0, pos.1 - press.start.1);
                if dy.abs() > l.drag_slop || dx.abs() > l.drag_slop * 2.0 {
                    press.dragged = true;
                }
                if press.dragged && l.max_scroll() > 0.0 {
                    let target = (press.scroll - dy).clamp(0.0, l.max_scroll());
                    if dt > 0.0 {
                        // Smoothed, so one jittery sample does not decide the glide.
                        let now = (target - self.scroll) / dt;
                        self.velocity = self.velocity * 0.6 + now * 0.4;
                    }
                    self.scroll = target;
                }
            }
            let dragging = self.press.is_some_and(|p| p.dragged);
            // A mouse selects what it hovers over (only when it actually moved, so it does not fight the
            // keys); a finger selects what it presses.
            let moved = self.last_pointer != Some(pos) && !input.touch_active();
            if (moved || input.pointer_pressed())
                && !dragging
                && self.rail_hold.is_none()
                && let Some(i) = l.item_at(pos, self.scroll).filter(|i| enabled(*i))
            {
                self.selected = i;
            }
            if input.pointer_released()
                && let Some(press) = self.press.take()
            {
                let hit = l.item_at(pos, self.scroll);
                // Movement decides, not time: a slow, deliberate press is still a tap.
                if !press.dragged && hit.is_some() && hit == press.item {
                    activated = hit.filter(|i| enabled(*i));
                }
                if !press.dragged {
                    self.velocity = 0.0;
                }
            }
        }
        if !input.pointer_held() {
            self.press = None;
            self.rail_hold = None;
            // Glide on after a drag.
            if self.velocity != 0.0 {
                let limit = GLIDE_MAX_AREAS_PER_SECOND * l.area.h;
                self.velocity = self.velocity.clamp(-limit, limit);
                let before = self.scroll;
                self.scroll_by(l, self.velocity * dt);
                self.velocity *= (1.0 - GLIDE_DECAY * dt).max(0.0);
                let stopped = self.velocity.abs() < GLIDE_MIN_SPEED_DP * l.dpi;
                let at_end = dt > 0.0 && self.scroll == before;
                if stopped || at_end {
                    self.velocity = 0.0;
                }
            }
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

/// A press-and-release detector for one button (the Back button, anything that is not a list row).
#[derive(Clone, Copy, Debug, Default)]
pub struct Tap {
    /// A press began on the button and is still held.
    pressed: bool,
}

impl Tap {
    /// True on the frame a press that began on `rect` is released on it, however long it was held.
    /// (`_dt` is unused: the signature matches the list's.)
    pub fn update(&mut self, input: &InputState, rect: Rect, _dt: f32) -> bool {
        let Some(pos) = input.pointer() else {
            self.pressed = false;
            return false;
        };
        if input.pointer_pressed() {
            self.pressed = rect.contains(pos);
        }
        let tapped =
            input.pointer_released() && std::mem::take(&mut self.pressed) && rect.contains(pos);
        if !input.pointer_held() {
            self.pressed = false;
        }
        tapped
    }

    /// The button is being pressed (draw it lit).
    pub fn down(&self) -> bool {
        self.pressed
    }
}

/// Where the Back button sits: the top-left corner of the safe area, a generous tap target.
pub fn back_button_rect(layout: &Layout) -> Rect {
    let safe = layout.safe_rect();
    let h = (64.0 * layout.hud_scale()).max(layout.min_tap() * 1.1);
    let margin = h * 0.18;
    Rect {
        x: safe.x + margin,
        y: safe.y + margin,
        w: h * 1.25,
        h,
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

/// Largest text scale at which `text` fits `width` and `height` in `font`. Never negative: where there is
/// no room at all (a very narrow target) the text is not drawn rather than mirrored.
pub fn fit_scale(p: &Painter, font: FontId, text: &str, width: f32, height: f32) -> f32 {
    let f = p.ui.font(font).p;
    fit(height, f.line_height, width, f.measure(text, 1.0))
}

/// One scale for several lines that belong together (a wrapped message): the largest at which the
/// longest of them still fits `width`. Fitted one by one, a short last line of a message would come
/// out larger than the long line above it wherever the width is what limits the size (a phone held
/// upright).
pub fn fit_scale_lines<'a>(
    p: &Painter,
    font: FontId,
    lines: impl Iterator<Item = &'a str>,
    width: f32,
    height: f32,
) -> f32 {
    let f = p.ui.font(font).p;
    fit_lines(
        height,
        f.line_height,
        width,
        lines.map(|l| f.measure(l, 1.0)),
    )
}

fn fit_lines(
    height: f32,
    line_height: f32,
    width: f32,
    natural_widths: impl Iterator<Item = f32>,
) -> f32 {
    let widest = natural_widths.fold(0.0, f32::max);
    fit(height, line_height, width, widest)
}

fn fit(height: f32, line_height: f32, width: f32, natural_width: f32) -> f32 {
    let by_height = height / line_height.max(1e-3);
    by_height.min(width / natural_width.max(1.0)).max(0.0)
}

/// A filled arrow head made of bars (the fonts have no arrows): `dir` is `(0, -1)` up, `(0, 1)` down,
/// `(-1, 0)` left.
pub fn draw_arrow(p: &mut Painter, rect: Rect, dir: (i32, i32), color: Color) {
    const BARS: usize = 7;
    let size = rect.w.min(rect.h) * 0.42;
    let (cx, cy) = rect.center();
    let thick = size / BARS as f32;
    for i in 0..BARS {
        // Bar `i` counts from the tip; it is as long as it is far from the tip.
        let t = (i as f32 + 0.5) / BARS as f32;
        let along = (0.5 - t) * size;
        let across = size * 2.0 * t;
        let r = if dir.1 != 0 {
            Rect {
                x: cx - across / 2.0,
                y: cy + along * dir.1 as f32 - thick / 2.0,
                w: across,
                h: thick + 0.5,
            }
        } else {
            Rect {
                x: cx + along * dir.0 as f32 - thick / 2.0,
                y: cy - across / 2.0,
                w: thick + 0.5,
                h: across,
            }
        };
        p.rect(r, color);
    }
}

/// Draw the Back button (lit while pressed).
pub fn draw_back_button(p: &mut Painter, rect: Rect, pressed: bool) {
    p.rect(rect, if pressed { PANEL_SELECTED } else { PANEL });
    p.frame(rect, (rect.h * 0.04).max(2.0), with_alpha(WHITE, 0.55));
    draw_arrow(p, rect, (-1, 0), WHITE);
}

/// Draw the rows of a list and, when it scrolls, its rail. With `partial` rows that are only partly inside
/// the window are drawn too: the caller then paints over what sticks out (the menus cover the header and
/// footer after the list); without it only whole rows are drawn.
pub fn draw_list<'a>(
    p: &mut Painter,
    l: &ListLayout,
    state: &ListState,
    row: &dyn Fn(usize) -> Row<'a>,
    partial: bool,
) {
    for i in 0..l.count {
        let shown = if partial {
            l.intersects(i, state.scroll)
        } else {
            l.is_visible(i, state.scroll)
        };
        if !shown {
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
}

/// Draw the rail of a scrolling list: page arrows (dim at the ends) and the scroll bar between them. Drawn
/// after whatever covers the rows that stick out, so it is never hidden.
pub fn draw_rail(p: &mut Painter, l: &ListLayout, state: &ListState) {
    let (Some(rail), Some(up), Some(down)) = (l.rail, l.rail_button(-1), l.rail_button(1)) else {
        return;
    };
    let max = l.max_scroll();
    for (rect, dir, live) in [
        (up, -1, state.scroll > 0.5),
        (down, 1, state.scroll < max - 0.5),
    ] {
        let held = state.rail_hold.is_some_and(|(d, _)| d == dir);
        p.rect(rect, if held { PANEL_SELECTED } else { PANEL });
        let alpha = if live { 1.0 } else { 0.3 };
        p.frame(
            rect,
            (rect.h * 0.04).max(2.0),
            with_alpha(WHITE, 0.55 * alpha),
        );
        draw_arrow(p, rect, (0, dir), with_alpha(WHITE, alpha));
    }
    let bar_w = (rail.w * 0.22).max(4.0);
    let pad = up.h * 0.2;
    let track = Rect {
        x: rail.x + (rail.w - bar_w) / 2.0,
        y: up.y + up.h + pad,
        w: bar_w,
        h: (down.y - pad - (up.y + up.h + pad)).max(0.0),
    };
    p.rect(track, [1.0, 1.0, 1.0, 0.15]);
    let frac = (l.area.h / l.content_height()).clamp(0.05, 1.0);
    let pos = if max > 0.0 { state.scroll / max } else { 0.0 };
    p.rect(
        Rect {
            y: track.y + (track.h - track.h * frac) * pos,
            h: track.h * frac,
            ..track
        },
        [1.0, 1.0, 1.0, 0.7],
    );
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

    /// A message wrapped into a long and a short line, on a wide and on a narrow screen: both lines
    /// get the size of the long one. (Fitted line by line, the short one was three times as large on
    /// a phone held upright.)
    #[test]
    fn the_lines_of_a_wrapped_message_share_the_size_of_the_longest() {
        // A 32 pixel font; lines of 900 and 260 pixels at scale one; room for 40 pixels of height.
        let (line_height, long, short) = (32.0, 900.0, 260.0);
        for width in [1152.0f32, 370.8] {
            let one_by_one = [long, short].map(|w| fit(40.0, line_height, width, w));
            let shared = fit_lines(40.0, line_height, width, [long, short].into_iter());
            assert_eq!(shared, one_by_one[0].min(one_by_one[1]));
            assert_eq!(shared, fit(40.0, line_height, width, long));
            assert!(long * shared <= width + 1e-3 && line_height * shared <= 40.0 + 1e-3);
        }
        // Wide: the height limits both, so nothing changes. Narrow: the long line is squeezed and the
        // short one no longer stands out.
        assert_eq!(fit(40.0, line_height, 1152.0, short), 1.25);
        assert_eq!(
            fit_lines(40.0, line_height, 1152.0, [long, short].into_iter()),
            1.25
        );
        let narrow = fit_lines(40.0, line_height, 370.8, [long, short].into_iter());
        assert!((narrow - 0.412).abs() < 1e-3, "{narrow}");
        assert!(fit(40.0, line_height, 370.8, short) > 3.0 * narrow);
        // No lines: the height alone.
        assert_eq!(
            fit_lines(40.0, line_height, 370.8, std::iter::empty()),
            1.25
        );
    }

    const ALL: &dyn Fn(usize) -> bool = &|_| true;
    const FRAME: f32 = 1.0 / 60.0;

    /// A touch screen: no hover, the pointer only exists while a finger is down.
    struct Finger {
        input: InputState,
    }

    impl Finger {
        fn new() -> Self {
            let mut input = InputState::new();
            input.set_touch_active(true);
            Self { input }
        }

        fn frame(&mut self, s: &mut ListState, l: &ListLayout) -> Option<usize> {
            let hit = s.update(&self.input, l, ALL, FRAME);
            self.input.end_frame();
            hit
        }

        fn down(&mut self, pos: (f32, f32)) {
            self.input.set_pointer(Some(pos));
            self.input.set_pointer_down(true);
        }

        fn drag_to(&mut self, pos: (f32, f32)) {
            self.input.set_pointer(Some(pos));
        }

        fn up(&mut self) {
            self.input.set_pointer_down(false);
        }

        /// Press, hold for `frames`, release; returns what was activated.
        fn tap(
            &mut self,
            s: &mut ListState,
            l: &ListLayout,
            pos: (f32, f32),
            frames: u32,
        ) -> Option<usize> {
            self.down(pos);
            let mut hit = None;
            for _ in 0..frames {
                hit = hit.or(self.frame(s, l));
            }
            self.up();
            hit.or(self.frame(s, l))
        }

        /// Drag from `from` by `dy` pixels over `frames` frames and release; returns what was activated.
        fn swipe(
            &mut self,
            s: &mut ListState,
            l: &ListLayout,
            from: (f32, f32),
            dy: f32,
            frames: u32,
        ) -> Option<usize> {
            self.down(from);
            let mut hit = self.frame(s, l);
            for i in 1..=frames {
                self.drag_to((from.0, from.1 + dy * i as f32 / frames as f32));
                hit = hit.or(self.frame(s, l));
            }
            self.up();
            hit.or(self.frame(s, l))
        }
    }

    #[test]
    fn rows_are_tap_sized_and_inside_the_safe_area() {
        for (size, dpi) in [((1280, 720), 1.0), ((720, 1280), 3.0), ((320, 240), 1.0)] {
            let lay = Layout::new(size, Insets::default(), dpi);
            for count in [4, 60] {
                let l = ListLayout::column(&lay, count, 0.3);
                assert!(l.item_h >= lay.min_tap());
                let r = l.item_rect(0, 0.0);
                assert!(r.x >= 0.0 && r.x + r.w <= size.0 as f32);
                assert_eq!(l.rail.is_some(), l.max_scroll() > 0.0);
                if let Some(rail) = l.rail {
                    assert!(rail.x >= r.x + r.w && rail.x + rail.w <= size.0 as f32);
                    for dir in [-1, 1] {
                        let b = l.rail_button(dir).unwrap();
                        assert!(b.w >= lay.min_tap().min(l.area.w * 0.2) && b.h > 0.0);
                    }
                }
            }
            let back = back_button_rect(&lay);
            assert!(back.w >= lay.min_tap() && back.h >= lay.min_tap());
            assert!(back.x >= 0.0 && back.y >= 0.0);
        }
    }

    #[test]
    fn the_back_button_stays_inside_the_safe_area() {
        let insets = Insets {
            left: 90.0,
            top: 50.0,
            right: 0.0,
            bottom: 30.0,
        };
        let lay = Layout::new((1830, 824), insets, 2.0);
        let back = back_button_rect(&lay);
        let safe = lay.safe_rect();
        assert!(back.x >= safe.x && back.y >= safe.y);
        assert!(back.x + back.w <= safe.x + safe.w && back.y + back.h <= safe.y + safe.h);
    }

    #[test]
    fn keys_step_wrap_and_skip_disabled_rows() {
        let l = layout(4);
        let mut s = ListState::new(0);
        let mut input = InputState::new();
        input.set_action(Action::LookDown, true);
        let enabled: &dyn Fn(usize) -> bool = &|i| i != 1;
        assert_eq!(s.update(&input, &l, enabled, FRAME), None);
        assert_eq!(s.selected, 2);
        input.end_frame();
        input.set_action(Action::LookDown, false);
        input.set_action(Action::LookUp, true);
        s.update(&input, &l, enabled, FRAME);
        assert_eq!(s.selected, 0);
        input.end_frame();
        input.set_action(Action::LookUp, false);
        input.set_action(Action::MoveForward, true);
        s.update(&input, &l, enabled, FRAME);
        assert_eq!(s.selected, 3, "wraps to the end");
        input.end_frame();
        input.set_action(Action::Confirm, true);
        assert_eq!(s.update(&input, &l, enabled, FRAME), Some(3));
    }

    #[test]
    fn stick_flick_moves_one_row() {
        let l = layout(3);
        let mut s = ListState::new(0);
        let mut input = InputState::new();
        input.set_axis(Axis::MoveY, -1.0);
        s.update(&input, &l, ALL, FRAME);
        input.end_frame();
        s.update(&input, &l, ALL, FRAME);
        assert_eq!(s.selected, 1);
    }

    #[test]
    fn a_tap_activates_the_row_under_the_finger() {
        let l = layout(30);
        let mut s = ListState::new(0);
        let mut f = Finger::new();
        let c = l.item_rect(2, 0.0).center();
        assert_eq!(f.tap(&mut s, &l, c, 3), Some(2));
        assert_eq!(s.selected, 2);
        // A few pixels of wobble is still a tap.
        f.down(c);
        f.frame(&mut s, &l);
        f.drag_to((c.0 + 3.0, c.1 - l.drag_slop * 0.8));
        f.frame(&mut s, &l);
        f.up();
        assert_eq!(f.frame(&mut s, &l), Some(2));
        assert_eq!(s.scroll, 0.0);
    }

    /// People press slowly on a phone: how long the finger rests does not matter, only whether it moved.
    #[test]
    fn a_long_press_still_activates() {
        let l = layout(30);
        let mut s = ListState::new(0);
        let mut f = Finger::new();
        let c = l.item_rect(1, 0.0).center();
        assert_eq!(
            f.tap(&mut s, &l, c, 3 * 60),
            Some(1),
            "a three second press"
        );
        // ... but a long press that wandered off is a drag.
        f.down(c);
        for _ in 0..90 {
            f.frame(&mut s, &l);
        }
        f.drag_to((c.0, c.1 - 3.0 * l.drag_slop));
        f.frame(&mut s, &l);
        f.drag_to(c);
        f.frame(&mut s, &l);
        f.up();
        assert_eq!(
            f.frame(&mut s, &l),
            None,
            "moved away and back: still a drag"
        );
    }

    #[test]
    fn text_never_gets_a_negative_scale() {
        assert_eq!(fit(40.0, 20.0, -15.0, 300.0), 0.0);
        assert_eq!(fit(-4.0, 20.0, 100.0, 300.0), 0.0);
        assert!((fit(40.0, 20.0, 150.0, 300.0) - 0.5).abs() < 1e-6);
        assert!((fit(40.0, 20.0, 1500.0, 300.0) - 2.0).abs() < 1e-6);
    }

    #[test]
    fn a_drag_scrolls_and_never_activates() {
        let l = layout(30);
        assert!(l.max_scroll() > 0.0);
        let mut s = ListState::new(0);
        let mut f = Finger::new();
        let c = l.item_rect(2, 0.0).center();
        // Slow drag up by two rows: the list follows the finger exactly and nothing is activated, even
        // though the finger is released over a row.
        assert_eq!(f.swipe(&mut s, &l, c, -2.0 * l.pitch(), 40), None);
        assert!(s.scroll >= 2.0 * l.pitch() - 1.0, "scroll {}", s.scroll);
        // The selection did not follow the drag, and the list did not snap back to it.
        let after = s.scroll;
        for _ in 0..120 {
            assert_eq!(f.frame(&mut s, &l), None);
        }
        assert!(s.scroll >= after, "snapped back to {}", s.scroll);
        assert!(!s.in_motion(), "the glide ends");
        // Dragging back down past the top stops at the top.
        f.swipe(&mut s, &l, c, 100.0 * l.pitch(), 20);
        for _ in 0..200 {
            f.frame(&mut s, &l);
        }
        assert_eq!(s.scroll, 0.0);
    }

    #[test]
    fn a_fast_swipe_glides_and_a_touch_stops_it() {
        let l = layout(60);
        let mut s = ListState::new(0);
        let mut f = Finger::new();
        let c = l.item_rect(2, 0.0).center();
        f.swipe(&mut s, &l, c, -1.5 * l.pitch(), 4);
        let released_at = s.scroll;
        assert!(s.in_motion());
        f.frame(&mut s, &l);
        f.frame(&mut s, &l);
        assert!(s.scroll > released_at, "glides on after the release");
        // A new touch catches the list where it is: the glide ends at once.
        f.down(c);
        f.frame(&mut s, &l);
        let caught = s.scroll;
        f.frame(&mut s, &l);
        assert_eq!(s.scroll, caught);
        assert!(!s.in_motion());
    }

    #[test]
    fn swipes_reach_the_last_row_of_a_long_list() {
        // The practice list of the longest course on a small phone held upright and sideways.
        for size in [(824, 1830), (1830, 824)] {
            let lay = Layout::new(size, Insets::default(), 2.0);
            let l = ListLayout::column(&lay, 51, 0.24);
            let mut s = ListState::new(0);
            let mut f = Finger::new();
            let from = (l.area.x + l.area.w / 2.0, l.area.y + l.area.h * 0.8);
            for _ in 0..200 {
                if s.scroll >= l.max_scroll() {
                    break;
                }
                f.swipe(&mut s, &l, from, -l.area.h * 0.6, 6);
                for _ in 0..30 {
                    f.frame(&mut s, &l);
                }
            }
            assert_eq!(s.scroll, l.max_scroll(), "{size:?}");
            assert!(l.is_visible(50, s.scroll));
            let last = l.item_rect(50, s.scroll).center();
            assert_eq!(f.tap(&mut s, &l, last, 2), Some(50), "{size:?}");
        }
    }

    #[test]
    fn page_arrows_scroll_and_repeat_while_held() {
        let l = layout(40);
        let mut s = ListState::new(0);
        let mut f = Finger::new();
        let down = l.rail_button(1).unwrap().center();
        let up = l.rail_button(-1).unwrap().center();
        assert_eq!(f.tap(&mut s, &l, down, 2), None);
        assert!((s.scroll - l.page()).abs() < 0.5);
        assert_eq!(s.selected, 0, "arrows do not select");
        // Held: more pages follow.
        f.down(down);
        for _ in 0..90 {
            f.frame(&mut s, &l);
        }
        f.up();
        f.frame(&mut s, &l);
        assert!(s.scroll > 3.0 * l.page() || s.scroll == l.max_scroll());
        for _ in 0..20 {
            f.tap(&mut s, &l, up, 2);
        }
        assert_eq!(s.scroll, 0.0);
        // Enough taps reach the end.
        for _ in 0..40 {
            f.tap(&mut s, &l, down, 2);
        }
        assert_eq!(s.scroll, l.max_scroll());
        assert!(l.is_visible(39, s.scroll));
    }

    #[test]
    fn mouse_wheel_and_click_drag_scroll_the_same_way() {
        let l = layout(30);
        let mut s = ListState::new(0);
        let mut input = InputState::new();
        input.add_scroll(-150.0);
        s.update(&input, &l, ALL, FRAME);
        assert_eq!(s.scroll, 150.0);
        input.end_frame();
        input.add_scroll(10_000.0);
        s.update(&input, &l, ALL, FRAME);
        assert_eq!(s.scroll, 0.0);
        input.end_frame();
        // Click-drag with the mouse (no touch flag): same drag, no activation.
        let c = l.item_rect(3, 0.0).center();
        input.set_pointer(Some(c));
        input.set_pointer_down(true);
        s.update(&input, &l, ALL, FRAME);
        input.end_frame();
        input.set_pointer(Some((c.0, c.1 - l.pitch())));
        s.update(&input, &l, ALL, FRAME);
        input.end_frame();
        input.set_pointer_down(false);
        assert_eq!(s.update(&input, &l, ALL, FRAME), None);
        // Followed the cursor by a row (and may glide on a little).
        assert!(s.scroll >= l.pitch() - 1.0 && s.scroll < 2.0 * l.pitch());
    }

    #[test]
    fn keys_bring_the_selection_into_view() {
        let l = layout(30);
        let mut s = ListState::new(0);
        let mut input = InputState::new();
        input.set_action(Action::LookUp, true);
        s.update(&input, &l, ALL, FRAME);
        assert_eq!(s.selected, 29);
        assert!(l.is_visible(29, s.scroll));
    }

    #[test]
    fn mouse_hover_selects_but_touch_does_not_hover() {
        let l = layout(5);
        let mut s = ListState::new(0);
        let mut input = InputState::new();
        input.set_pointer(Some(l.item_rect(3, 0.0).center()));
        s.update(&input, &l, ALL, FRAME);
        assert_eq!(s.selected, 3);
        input.end_frame();
        // After a touch, the stale pointer position selects nothing.
        input.set_touch_active(true);
        input.set_pointer(Some(l.item_rect(1, 0.0).center()));
        s.update(&input, &l, ALL, FRAME);
        assert_eq!(s.selected, 3);
    }

    #[test]
    fn disabled_rows_cannot_be_tapped() {
        let l = layout(3);
        let mut s = ListState::new(0);
        let mut input = InputState::new();
        input.set_pointer(Some(l.item_rect(1, 0.0).center()));
        input.set_pointer_down(true);
        let enabled: &dyn Fn(usize) -> bool = &|i| i != 1;
        s.update(&input, &l, enabled, FRAME);
        input.end_frame();
        input.set_pointer_down(false);
        assert_eq!(s.update(&input, &l, enabled, FRAME), None);
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn rows_outside_the_window_cannot_be_hit() {
        let l = layout(30);
        // Row 0 scrolled half out of the top: its visible part can be tapped, a point above the window
        // (where the header is) cannot.
        let scroll = l.item_h / 2.0;
        let r = l.item_rect(0, scroll);
        assert_eq!(l.item_at((r.x + 5.0, l.area.y + 2.0), scroll), Some(0));
        assert_eq!(l.item_at((r.x + 5.0, l.area.y - 2.0), scroll), None);
        assert!(l.intersects(0, scroll) && !l.is_visible(0, scroll));
    }

    #[test]
    fn the_back_button_taps_like_a_button() {
        let lay = Layout::new((1280, 720), Insets::default(), 1.0);
        let rect = back_button_rect(&lay);
        let mut tap = Tap::default();
        let mut input = InputState::new();
        input.set_touch_active(true);
        let c = rect.center();
        input.set_pointer(Some(c));
        input.set_pointer_down(true);
        assert!(!tap.update(&input, rect, FRAME));
        assert!(tap.down());
        input.end_frame();
        input.set_pointer_down(false);
        assert!(tap.update(&input, rect, FRAME));
        input.end_frame();
        // Pressed on the button, released elsewhere: nothing. Pressed elsewhere, released on it: nothing.
        for (from, to) in [
            (c, (c.0 + rect.w * 3.0, c.1)),
            ((c.0 + rect.w * 3.0, c.1), c),
        ] {
            input.set_pointer(Some(from));
            input.set_pointer_down(true);
            assert!(!tap.update(&input, rect, FRAME));
            input.end_frame();
            input.set_pointer(Some(to));
            input.set_pointer_down(false);
            assert!(!tap.update(&input, rect, FRAME));
            input.end_frame();
        }
        // Held for two seconds: still a tap.
        input.set_pointer(Some(c));
        input.set_pointer_down(true);
        for _ in 0..120 {
            assert!(!tap.update(&input, rect, FRAME));
            input.end_frame();
        }
        input.set_pointer_down(false);
        assert!(tap.update(&input, rect, FRAME));
        input.end_frame();
        // A press that was cancelled (the system took the touch) is not a tap.
        input.set_pointer_down(true);
        tap.update(&input, rect, FRAME);
        input.end_frame();
        input.cancel_pointer();
        assert!(!tap.update(&input, rect, FRAME));
        assert!(!tap.down());
    }
}
