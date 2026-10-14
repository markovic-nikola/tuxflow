//! A widget wearing a corner badge that may spill past its edges.
//!
//! iced lays every child out inside its parent: a badge in a `stack` or a
//! `pin` over a 26 px avatar is clamped to those 26 px, so one wider than
//! the corner leaves gets squeezed into a pill. Growing the avatar's box
//! to make room would move the title beside it and heighten every card.
//! [`Badged`] lays out exactly as its content does and lays the badge out
//! on its own, centred [`Badged::inset`] in from the top-right corner,
//! drawn in a layer above the content (pictures included) and over the
//! gap around it.
//! It is paint only: every event, operation and overlay goes to the
//! content (the rail's toggle is a button with a tooltip).

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Clipboard, Renderer as _, Shell, Widget, overlay, renderer};
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector, mouse};

pub struct Badged<'a, Message> {
    content: Element<'a, Message>,
    badge: Element<'a, Message>,
    inset: f32,
}

impl<'a, Message> Badged<'a, Message> {
    /// `badge` over `content`'s top-right corner; an empty `Space` when
    /// there is none, so the tree keeps its shape either way.
    pub fn new(
        content: impl Into<Element<'a, Message>>,
        badge: impl Into<Element<'a, Message>>,
    ) -> Self {
        Self {
            content: content.into(),
            badge: badge.into(),
            inset: 5.0,
        }
    }

    /// How far in from the top and right edges the badge's centre sits.
    pub fn inset(mut self, inset: f32) -> Self {
        self.inset = inset;
        self
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for Badged<'_, Message> {
    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content), Tree::new(&self.badge)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&[&self.content, &self.badge]);
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
        let content = self
            .content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        let badge = self.badge.as_widget_mut().layout(
            &mut tree.children[1],
            renderer,
            &layout::Limits::new(Size::ZERO, Size::INFINITE),
        );
        let (outer, mark) = (content.size(), badge.size());
        let badge = badge.move_to(Point::new(
            outer.width - self.inset - mark.width / 2.0,
            self.inset - mark.height / 2.0,
        ));
        layout::Node::with_children(outer, vec![content, badge])
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
            content(layout),
            renderer,
            operation,
        );
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
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            content(layout),
            cursor,
            renderer,
            clipboard,
            shell,
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
            content(layout),
            cursor,
            viewport,
            renderer,
        )
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
        let mut children = layout.children();
        let (content, badge) = (children.next().unwrap(), children.next().unwrap());
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            content,
            cursor,
            viewport,
        );
        // A layer of its own: within one layer the renderer paints images
        // after every quad, so an avatar picture would cover the badge
        // drawn after it. A layer doesn't inherit the clip around it, so
        // it is cut to the viewport here — a card scrolled out of the
        // sidebar must not leave its badge over the toolbar.
        let Some(clip) = badge.bounds().intersection(viewport) else {
            return;
        };
        renderer.with_layer(clip, |renderer| {
            self.badge.as_widget().draw(
                &tree.children[1],
                renderer,
                theme,
                style,
                badge,
                mouse::Cursor::Unavailable,
                &clip,
            );
        });
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
            content(layout),
            renderer,
            viewport,
            translation,
        )
    }
}

fn content(layout: Layout<'_>) -> Layout<'_> {
    layout.children().next().unwrap()
}

impl<'a, Message: 'a> From<Badged<'a, Message>> for Element<'a, Message> {
    fn from(badged: Badged<'a, Message>) -> Self {
        Element::new(badged)
    }
}
