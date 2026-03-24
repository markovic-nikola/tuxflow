use gtk4::prelude::*;

const DROP_ABOVE: &str = "drop-target-above";
const DROP_BELOW: &str = "drop-target-below";

/// Returns `true` if drop should go before the widget, `false` for after.
pub fn update_drop_indicator(widget: &impl IsA<gtk4::Widget>, y: f64) -> bool {
    let w = widget.upcast_ref::<gtk4::Widget>();
    let height = w.height() as f64;
    let before = y < height / 2.0;
    if before {
        w.add_css_class(DROP_ABOVE);
        w.remove_css_class(DROP_BELOW);
    } else {
        w.add_css_class(DROP_BELOW);
        w.remove_css_class(DROP_ABOVE);
    }
    before
}

pub fn clear_drop_indicator(widget: &impl IsA<gtk4::Widget>) {
    let w = widget.upcast_ref::<gtk4::Widget>();
    w.remove_css_class(DROP_ABOVE);
    w.remove_css_class(DROP_BELOW);
}

/// Attach drag-begin/end handlers that set a WidgetPaintable as the drag icon
/// and toggle a "dragging" CSS class on the source widget.
pub fn setup_drag_icon(drag_source: &gtk4::DragSource, widget: &impl IsA<gtk4::Widget>) {
    let widget_ref = widget.upcast_ref::<gtk4::Widget>().clone();
    drag_source.connect_drag_begin(move |source, _drag| {
        let paintable = gtk4::WidgetPaintable::new(Some(&widget_ref));
        let width = widget_ref.width();
        let height = widget_ref.height();
        source.set_icon(Some(&paintable), width / 2, height / 2);
        widget_ref.add_css_class("dragging");
    });

    let widget_ref2 = widget.upcast_ref::<gtk4::Widget>().clone();
    drag_source.connect_drag_end(move |_, _, _| {
        widget_ref2.remove_css_class("dragging");
    });
}

/// Move `child` in `parent` so it sits before or after `sibling`.
pub fn reorder_in_box(parent: &gtk4::Box, child: &impl IsA<gtk4::Widget>, sibling: &impl IsA<gtk4::Widget>, before: bool) {
    let child = child.upcast_ref::<gtk4::Widget>();
    let sibling = sibling.upcast_ref::<gtk4::Widget>();
    parent.remove(child);
    if before {
        let prev = sibling.prev_sibling();
        parent.insert_child_after(child, prev.as_ref());
    } else {
        parent.insert_child_after(child, Some(sibling));
    }
}
