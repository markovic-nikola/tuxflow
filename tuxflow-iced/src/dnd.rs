//! Sidebar drag-and-drop — the iced half of GTK's `sidebar/dnd.rs`.
//!
//! iced has no DragSource/DropTarget. A drag here is a press that travels,
//! held as app state, and the app needs three things no stock widget
//! reports: WHERE a press landed (the row's button captures it, and a
//! wrapper that waits its turn — `mouse_area` included — never hears a
//! captured event), WHICH HALF of a row the pointer is over
//! (`mouse_area::on_move` gives the point but not the row's height), and
//! how close the pointer is to the sidebar's visible edges (auto-scroll).
//! [`DragArea`] senses all three by peeking at the event BEFORE handing it
//! to its content, then delegates everything else untouched.
//!
//! The permutation math lives here too, pure and unit-tested: GTK's
//! `reorder_in_box` / `reorder_process` in index terms, plus the remap that
//! index-keyed state (the selection, an open edit form) needs to follow
//! its row.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Clipboard, Shell, Widget, overlay, renderer};
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector, mouse};

/// GTK's default `gtk-dnd-drag-threshold`: a press becomes a drag once the
/// pointer has travelled this far on either axis (GTK checks the axes
/// separately, not the distance).
pub const DRAG_THRESHOLD: f32 = 8.0;

/// How close to the sidebar's visible top/bottom edge the pointer has to be
/// for the list to start scrolling under a drag, in px.
pub const EDGE_ZONE: f32 = 32.0;

/// Auto-scroll speed at the very edge, in px per frame; ramps linearly to
/// zero across [`EDGE_ZONE`].
pub const MAX_STEP: f32 = 10.0;

/// What a press on a row hands the app: the pointer's offset inside the row
/// and the row's size — the ghost is placed at `pointer - offset`, so it
/// appears exactly where the row was and stays under the grab point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grab {
    pub offset: Vector,
    pub size: Size,
}

/// The pointer's distance to the sidebar viewport's top and bottom edges.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Edges {
    pub top: f32,
    pub bottom: f32,
}

/// What a row reports while a drag is over it: which half the pointer is
/// in, plus the pointer's offset inside the row and the row's size — so the
/// drop rule can be drawn in window space (the ghost's layer) from the
/// pointer position alone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Over {
    pub before: bool,
    pub offset: Vector,
    pub size: Size,
}

pub fn past_threshold(origin: Point, now: Point) -> bool {
    (now.x - origin.x).abs() > DRAG_THRESHOLD || (now.y - origin.y).abs() > DRAG_THRESHOLD
}

/// Where `src` lands when dropped before/after `target` — GTK's
/// remove-then-insert-relative-to-sibling, so the target's own index
/// shifts once `src` is out. `None` when nothing would move: the source
/// itself, or the slot it already occupies (dropping a row just above the
/// row below it).
pub fn drop_index(src: usize, target: usize, before: bool) -> Option<usize> {
    if src == target {
        return None;
    }
    let target = if target > src { target - 1 } else { target };
    let dst = if before { target } else { target + 1 };
    (dst != src).then_some(dst)
}

/// Move `v[src]` to `dst`, an index in the list WITHOUT `src` — what
/// [`drop_index`] returns.
pub fn reorder<T>(v: &mut Vec<T>, src: usize, dst: usize) {
    let item = v.remove(src);
    v.insert(dst, item);
}

/// The new index of what was at `i` before `reorder(v, src, dst)`.
pub fn remap(i: usize, src: usize, dst: usize) -> usize {
    if i == src {
        dst
    } else if src < i && i <= dst {
        i - 1
    } else if dst <= i && i < src {
        i + 1
    } else {
        i
    }
}

/// Signed scroll step for the pointer's edge distances: negative near the
/// top, positive near the bottom, zero in the middle or off the sidebar.
pub fn autoscroll_step(edges: Option<Edges>) -> f32 {
    let Some(e) = edges else { return 0.0 };
    if e.top < EDGE_ZONE {
        -(1.0 - e.top.max(0.0) / EDGE_ZONE) * MAX_STEP
    } else if e.bottom < EDGE_ZONE {
        (1.0 - e.bottom.max(0.0) / EDGE_ZONE) * MAX_STEP
    } else {
        0.0
    }
}

/// A transparent wrapper reporting presses, hovered halves and edge
/// distances for its content. Layout, drawing, overlays and the events
/// themselves pass straight through — the content never knows it is
/// wrapped, and nothing here captures.
pub struct DragArea<'a, Message> {
    content: Element<'a, Message>,
    on_press: Option<Box<dyn Fn(Grab) -> Message + 'a>>,
    on_over: Option<Box<dyn Fn(Over) -> Message + 'a>>,
    on_track: Option<Box<dyn Fn(Option<Edges>) -> Message + 'a>>,
}

/// The pointer and bounds at the last look. A scroll moves the content
/// under a still pointer, so the half it is over can change with no
/// CursorMoved to say so — the same change detection `mouse_area` keeps
/// for its enter/exit, and the reason this widget has state at all.
#[derive(Default)]
struct State {
    cursor: Option<Point>,
    bounds: Rectangle,
}

impl<'a, Message> DragArea<'a, Message> {
    pub fn new(content: impl Into<Element<'a, Message>>) -> Self {
        Self {
            content: content.into(),
            on_press: None,
            on_over: None,
            on_track: None,
        }
    }

    /// A left press landed on the content: the start of a possible drag.
    /// Peeked before the content sees the event, so the button inside
    /// still gets its click.
    pub fn on_press(mut self, f: impl Fn(Grab) -> Message + 'a) -> Self {
        self.on_press = Some(Box::new(f));
        self
    }

    /// The pointer is over the content — reported with which half and
    /// where, whenever that can have changed. Set only while a drag is
    /// active; the wrapper stays in the tree either way, so wiring it up
    /// never changes the tree's shape.
    pub fn on_over(mut self, f: impl Fn(Over) -> Message + 'a) -> Self {
        self.on_over = Some(Box::new(f));
        self
    }

    /// On every pointer move: how far the pointer is from the visible
    /// viewport's top and bottom edges, or None when it is not over the
    /// content at all. For the scrollable's content root, whose viewport
    /// is the sidebar's visible window.
    pub fn on_track(mut self, f: impl Fn(Option<Edges>) -> Message + 'a) -> Self {
        self.on_track = Some(Box::new(f));
        self
    }

    fn sense(
        &self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        shell: &mut Shell<'_, Message>,
    ) {
        let bounds = layout.bounds();
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(on_press) = &self.on_press
                    && let Some(p) = cursor.position_in(bounds)
                {
                    shell.publish(on_press(Grab {
                        offset: Vector::new(p.x, p.y),
                        size: bounds.size(),
                    }));
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { .. } | mouse::Event::CursorLeft) => {
                if let Some(on_track) = &self.on_track {
                    // Cursor and viewport are both in the scrollable's
                    // content space here (it translates the cursor by the
                    // scroll offset and the viewport with it), so the
                    // distances come out in plain pixels.
                    let edges = cursor.position_over(bounds).map(|p| Edges {
                        top: p.y - viewport.y,
                        bottom: viewport.y + viewport.height - p.y,
                    });
                    shell.publish(on_track(edges));
                }
            }
            _ => {}
        }
        if let Some(on_over) = &self.on_over {
            let state: &mut State = tree.state.downcast_mut();
            let position = cursor.position();
            if state.cursor != position || state.bounds != bounds {
                state.cursor = position;
                state.bounds = bounds;
                if let Some(p) = cursor.position_in(bounds) {
                    shell.publish(on_over(Over {
                        before: p.y < bounds.height / 2.0,
                        offset: Vector::new(p.x, p.y),
                        size: bounds.size(),
                    }));
                }
            }
        }
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for DragArea<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.sense(tree, event, layout, cursor, viewport, shell);
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a, Message: 'a> From<DragArea<'a, Message>> for Element<'a, Message> {
    fn from(area: DragArea<'a, Message>) -> Self {
        Element::new(area)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_is_per_axis_and_strict() {
        let o = Point::new(10.0, 10.0);
        assert!(!past_threshold(o, Point::new(18.0, 18.0)));
        assert!(past_threshold(o, Point::new(18.5, 10.0)));
        assert!(past_threshold(o, Point::new(10.0, 1.0)));
    }

    /// GTK's `reorder_in_box` on a four-row box, every direction.
    #[test]
    fn drop_index_matches_gtk_remove_then_insert() {
        // Dragging down: the target shifts up once the source is out.
        assert_eq!(drop_index(0, 2, true), Some(1));
        assert_eq!(drop_index(0, 2, false), Some(2));
        // Dragging up: the target keeps its index.
        assert_eq!(drop_index(3, 1, true), Some(1));
        assert_eq!(drop_index(3, 1, false), Some(2));
        // No-ops: onto itself, or into the slot it already fills.
        assert_eq!(drop_index(2, 2, true), None);
        assert_eq!(drop_index(1, 2, true), None);
        assert_eq!(drop_index(2, 1, false), None);
    }

    #[test]
    fn reorder_and_remap_agree() {
        for (src, dst) in [(0, 2), (3, 1), (1, 3), (2, 0), (1, 1)] {
            let mut v: Vec<usize> = (0..4).collect();
            reorder(&mut v, src, dst);
            for (new_index, &old) in v.iter().enumerate() {
                assert_eq!(
                    remap(old, src, dst),
                    new_index,
                    "src {src} dst {dst} old {old}"
                );
            }
        }
    }

    /// The pointer rests on the target's slot through the drop; the entry
    /// drawn there afterwards is whatever now holds the target's OLD flat
    /// index, even with other categories interleaved — so the hovered row
    /// needs no remap. Pinned because the sidebar draws per category over
    /// the flat list.
    #[test]
    fn target_slot_keeps_its_flat_index_across_categories() {
        // c = command, a = agent; drag within c only.
        let list = ["c0", "a0", "c1", "a1", "c2"];
        fn visible<'a>(v: &[&'a str]) -> Vec<&'a str> {
            v.iter().copied().filter(|s| s.starts_with('c')).collect()
        }
        for src in [0usize, 2, 4] {
            for tgt in [0usize, 2, 4] {
                for before in [true, false] {
                    let Some(dst) = drop_index(src, tgt, before) else {
                        continue;
                    };
                    let mut v = list.to_vec();
                    reorder(&mut v, src, dst);
                    // The target's visual slot among the commands...
                    let slot = visible(&list).iter().position(|&s| s == list[tgt]).unwrap();
                    // ...is now drawn from flat index `tgt`.
                    assert_eq!(
                        visible(&v)[slot],
                        v[tgt],
                        "src {src} tgt {tgt} before {before}"
                    );
                }
            }
        }
    }

    #[test]
    fn autoscroll_ramps_toward_the_edges() {
        assert_eq!(autoscroll_step(None), 0.0);
        let mid = Some(Edges {
            top: 200.0,
            bottom: 200.0,
        });
        assert_eq!(autoscroll_step(mid), 0.0);
        let at_top = Some(Edges {
            top: 0.0,
            bottom: 400.0,
        });
        assert_eq!(autoscroll_step(at_top), -MAX_STEP);
        let near_bottom = Some(Edges {
            top: 400.0,
            bottom: EDGE_ZONE / 2.0,
        });
        assert_eq!(autoscroll_step(near_bottom), MAX_STEP / 2.0);
    }
}
