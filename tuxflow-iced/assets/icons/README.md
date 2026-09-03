# Vendored symbolic icons

Icons copied from the GNOME **adwaita-icon-theme**
(`/usr/share/icons/Adwaita/symbolic/`) so this shell renders the same
glyphs as the GTK app without depending on an installed icon theme.
They are recolored at render time via iced's svg tint (the `fill`
color inside the files is ignored).

Header cluster:

- `sidebar-show-symbolic.svg` — toggle sidebar
- `edit-find-symbolic.svg` — filter sidebar
- `emblem-system-symbolic.svg` — settings
- `list-add-symbolic.svg` — add project

Sidebar lifecycle controls (process rows + project headers):

- `media-playback-start-symbolic.svg` — start
- `media-playback-stop-symbolic.svg` — stop
- `view-refresh-symbolic.svg` — restart

Status bar:

- `focus-windows-symbolic.svg` — focus mode (hide the sidebar)
- `edit-clear-symbolic.svg` — clear the terminal
- `external-link-symbolic.svg` — open the detected URL

Two are TuxFlow's own, MIT like the rest of the app, not Adwaita copies:

- `tuxflow-remote-symbolic.svg` (app-namespaced so a user's icon theme
  can't override the glyph, copied from `data/icons/`) marks a project
  that lives on an ssh host.
- `external-link-symbolic.svg` is drawn from scratch because the name is
  a hole in adwaita-icon-theme: GTK resolves it from libadwaita's
  private resources or the desktop theme (Yaru's copy is GPL-3.0+,
  which an MIT binary must not embed).

adwaita-icon-theme licenses the rest under **LGPL-3.0 or CC-BY-SA-3.0**
(dual); they are redistributed here under CC-BY-SA-3.0 with this notice
as attribution. Everything else in this crate remains MIT.
