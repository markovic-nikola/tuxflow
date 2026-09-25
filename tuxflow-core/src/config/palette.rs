//! Shared color data for both shells: the accent palette (app accent plus
//! the local/remote sidebar hues), the fixed-meaning status hues, and the
//! terminal color schemes. Everything is a hex string — the GTK shell
//! parses into `gdk::RGBA`/CSS, the iced shell into `iced::Color` or
//! `iced_term::ColorPalette` — so this file is the single authoritative
//! representation and a palette edit lands in both shells at once.
//!
//! The contrast tests live here too: the numbers are properties of the
//! data, not of either toolkit.

pub struct AccentColor {
    pub name: &'static str,
    pub label: &'static str,
    pub bg: &'static str,
    pub fg: &'static str,
    /// Text-weight accent, used against dark surfaces. Doubles as the
    /// sidebar hue for local/remote projects, which is why a few entries
    /// are tuned brighter than their `bg`: the sidebar reads them as text
    /// and thin borders, not as filled buttons.
    pub accent: &'static str,
    /// Same hue darkened for light surfaces. `accent` is tuned for dark mode
    /// and lands at 2-3:1 against libadwaita's light sidebar (#ebebeb) — these
    /// hit ~4.2:1 while keeping the hue recognisable. `bg`/`fg` need no
    /// variant: they are a filled button, not text, and already pair.
    pub accent_light: &'static str,
}

pub const ACCENT_COLORS: &[AccentColor] = &[
    AccentColor {
        name: "green",
        label: "Green",
        bg: "#2ea043",
        fg: "#ffffff",
        // The sidebar's running-green since day one, and 6.7:1 on the dark
        // sidebar where a button-weight #3fb950 only manages 4.2:1.
        accent: "#73c991",
        accent_light: "#1a7f37",
    },
    AccentColor {
        name: "blue",
        label: "Blue",
        bg: "#3584e4",
        fg: "#ffffff",
        accent: "#5d9de9",
        accent_light: "#1c6dcf",
    },
    AccentColor {
        name: "purple",
        label: "Purple",
        bg: "#9141ac",
        fg: "#ffffff",
        accent: "#bd83d0",
        accent_light: "#9141ac",
    },
    AccentColor {
        name: "teal",
        label: "Teal",
        bg: "#2190a4",
        fg: "#ffffff",
        accent: "#27a8c0",
        accent_light: "#1b7888",
    },
    AccentColor {
        name: "orange",
        label: "Orange",
        bg: "#e66100",
        fg: "#ffffff",
        accent: "#ff6c00",
        accent_light: "#b84e00",
    },
    AccentColor {
        name: "red",
        label: "Red",
        bg: "#e01b24",
        fg: "#ffffff",
        accent: "#ee7379",
        accent_light: "#db1a23",
    },
    AccentColor {
        name: "pink",
        label: "Pink",
        bg: "#d56199",
        fg: "#ffffff",
        accent: "#db79a9",
        accent_light: "#c5347a",
    },
    AccentColor {
        name: "yellow",
        label: "Yellow",
        bg: "#c88800",
        fg: "#ffffff",
        // TuxFlow's logo gold — the default remote-project accent. On light
        // surfaces it measures 1.24:1, hence the very different companion.
        accent: "#ffce5c",
        accent_light: "#9a6700",
    },
    AccentColor {
        name: "slate",
        label: "Slate",
        bg: "#6e8898",
        fg: "#ffffff",
        accent: "#869ca9",
        accent_light: "#5b7280",
    },
];

/// Palette names used when a settings file omits or misspells a choice.
/// `AppearanceSettings::default` carries the same names.
pub const FALLBACK_LOCAL: &str = "green";
pub const FALLBACK_REMOTE: &str = "yellow";

/// Status-dot hues that carry a fixed meaning rather than a chosen one,
/// as (name, dark, light). They still need the light twin: both ambers
/// measure ~2.2:1 on the light sidebar, under even the 3:1 that a dot has
/// to clear. Running/stopped/crashed aren't here — running follows the
/// project's accent, and the other two read in both schemes as they are.
pub const STATUS_COLORS: &[(&str, &str, &str)] = &[
    ("status_working", "#e0a030", "#b06a00"),
    ("status_restarting", "#cca700", "#8a6f00"),
];

/// An accent by name, falling back to `fallback` for unknown names
/// (hand-edited settings, a palette entry we dropped) rather than to
/// nothing — a consumer needs *some* color or its rules drop entirely.
pub fn accent_by_name(name: &str, fallback: &str) -> &'static AccentColor {
    let by_name = |n: &str| ACCENT_COLORS.iter().find(|c| c.name == n);
    by_name(name)
        .or_else(|| by_name(fallback))
        .expect("fallback accent is in the palette")
}

pub fn accent_choices() -> Vec<&'static str> {
    ACCENT_COLORS.iter().map(|c| c.label).collect()
}

pub fn accent_index(name: &str) -> u32 {
    ACCENT_COLORS
        .iter()
        .position(|c| c.name == name)
        .unwrap_or(0) as u32
}

pub fn accent_name(index: u32) -> &'static str {
    ACCENT_COLORS
        .get(index as usize)
        .map(|c| c.name)
        .unwrap_or(FALLBACK_LOCAL)
}

// ── Terminal color schemes ──────────────────────────────────────────────

pub struct TerminalTheme {
    pub name: &'static str,
    pub label: &'static str,
    pub foreground: &'static str,
    pub background: &'static str,
    pub cursor: &'static str,
    /// The 16 ANSI colors, normal 0-7 then bright 8-15.
    pub palette: [&'static str; 16],
}

pub const TERMINAL_THEMES: &[TerminalTheme] = &[
    // Index 0 is the fallback for unknown names; the rest run dark A–Z, then light A–Z.
    TerminalTheme {
        name: "catppuccin-mocha",
        label: "Catppuccin Mocha (Default)",
        foreground: "#CDD6F4",
        background: "#1E1E2E",
        cursor: "#F5E0DC",
        palette: [
            "#45475A", "#F38BA8", "#A6E3A1", "#F9E2AF", "#89B4FA", "#F5C2E7", "#94E2D5", "#BAC2DE",
            "#585B70", "#F38BA8", "#A6E3A1", "#F9E2AF", "#89B4FA", "#F5C2E7", "#94E2D5", "#A6ADC8",
        ],
    },
    // -- Dark themes --
    TerminalTheme {
        name: "adwaita-dark",
        label: "Adwaita Dark",
        foreground: "#FFFFFF",
        background: "#1C1C1F",
        cursor: "#FFFFFF",
        palette: [
            "#241F31", "#C01C28", "#2EC27E", "#F5C211", "#1E78E4", "#9841BB", "#0AB9DC", "#C0BFBC",
            "#5E5C64", "#ED333B", "#57E389", "#F8E45C", "#51A1FF", "#C061CB", "#4FD2FD", "#F6F5F4",
        ],
    },
    TerminalTheme {
        name: "ayu-dark",
        label: "Ayu Dark",
        foreground: "#BFBDB6",
        background: "#0B0E14",
        cursor: "#E6B450",
        palette: [
            "#11151C", "#EA6C73", "#7FD962", "#F9AF4F", "#53BDFA", "#CDA1FA", "#90E1C6", "#C7C7C7",
            "#686868", "#F07178", "#AAD94C", "#FFB454", "#59C2FF", "#D2A6FF", "#95E6CB", "#FFFFFF",
        ],
    },
    TerminalTheme {
        name: "ayu-mirage",
        label: "Ayu Mirage",
        foreground: "#CCCAC2",
        background: "#1F2430",
        cursor: "#FFCC66",
        palette: [
            "#171B24", "#ED8274", "#87D96C", "#FACC6E", "#6DCBFA", "#DABAFA", "#90E1C6", "#C7C7C7",
            "#686868", "#F28779", "#D5FF80", "#FFD173", "#73D0FF", "#DFBFFF", "#95E6CB", "#FFFFFF",
        ],
    },
    TerminalTheme {
        name: "carbonfox",
        label: "Carbonfox",
        foreground: "#F2F4F8",
        background: "#161616",
        cursor: "#F2F4F8",
        palette: [
            "#282828", "#EE5396", "#25BE6A", "#08BDBA", "#78A9FF", "#BE95FF", "#33B1FF", "#DFDFE0",
            "#484848", "#F16DA6", "#46C880", "#2DC7C4", "#8CB6FF", "#C8A5FF", "#52BDFF", "#E4E4E5",
        ],
    },
    TerminalTheme {
        name: "catppuccin-frappe",
        label: "Catppuccin Frapp\u{e9}",
        foreground: "#C6D0F5",
        background: "#303446",
        cursor: "#F2D5CF",
        palette: [
            "#51576D", "#E78284", "#A6D189", "#E5C890", "#8CAAEE", "#F4B8E4", "#81C8BE", "#B5BFE2",
            "#626880", "#EDA0A2", "#B9DBA2", "#ECD7AE", "#ADC2F3", "#F38ED8", "#98D2CA", "#A5ADCE",
        ],
    },
    TerminalTheme {
        name: "catppuccin-macchiato",
        label: "Catppuccin Macchiato",
        foreground: "#CAD3F5",
        background: "#24273A",
        cursor: "#F4DBD6",
        palette: [
            "#494D64", "#ED8796", "#A6DA95", "#EED49F", "#8AADF4", "#F5BDE6", "#8BD5CA", "#B8C0E0",
            "#5B6078", "#F2A7B2", "#BDE3B0", "#F4E3C1", "#ADC5F7", "#F493DA", "#A5DED6", "#A5ADCB",
        ],
    },
    TerminalTheme {
        name: "dracula",
        label: "Dracula",
        foreground: "#F8F8F2",
        background: "#282A36",
        cursor: "#F8F8F2",
        palette: [
            "#21222C", "#FF5555", "#50FA7B", "#F1FA8C", "#BD93F9", "#FF79C6", "#8BE9FD", "#F8F8F2",
            "#6272A4", "#FF6E6E", "#69FF94", "#FFFFA5", "#D6ACFF", "#FF92DF", "#A4FFFF", "#FFFFFF",
        ],
    },
    TerminalTheme {
        name: "everforest-dark",
        label: "Everforest Dark",
        foreground: "#D3C6AA",
        background: "#2D353B",
        cursor: "#D3C6AA",
        palette: [
            "#475258", "#E67E80", "#A7C080", "#DBBC7F", "#7FBBB3", "#D699B6", "#83C092", "#D3C6AA",
            "#475258", "#E67E80", "#A7C080", "#DBBC7F", "#7FBBB3", "#D699B6", "#83C092", "#D3C6AA",
        ],
    },
    TerminalTheme {
        name: "flexoki-dark",
        label: "Flexoki Dark",
        foreground: "#CECDC3",
        background: "#100F0F",
        cursor: "#CECDC3",
        palette: [
            "#100F0F", "#D14D41", "#879A39", "#D0A215", "#4385BE", "#CE5D97", "#3AA99F", "#878580",
            "#575653", "#AF3029", "#66800B", "#AD8301", "#205EA6", "#A02F6F", "#24837B", "#CECDC3",
        ],
    },
    TerminalTheme {
        name: "github-dark",
        label: "GitHub Dark",
        foreground: "#E6EDF3",
        background: "#0D1117",
        cursor: "#2F81F7",
        palette: [
            "#484F58", "#FF7B72", "#3FB950", "#D29922", "#58A6FF", "#BC8CFF", "#39C5CF", "#B1BAC4",
            "#6E7681", "#FFA198", "#56D364", "#E3B341", "#79C0FF", "#D2A8FF", "#56D4DD", "#FFFFFF",
        ],
    },
    TerminalTheme {
        name: "gruvbox-dark",
        label: "Gruvbox Dark",
        foreground: "#EBDBB2",
        background: "#282828",
        cursor: "#EBDBB2",
        palette: [
            "#282828", "#CC241D", "#98971A", "#D79921", "#458588", "#B16286", "#689D6A", "#A89984",
            "#928374", "#FB4934", "#B8BB26", "#FABD2F", "#83A598", "#D3869B", "#8EC07C", "#EBDBB2",
        ],
    },
    TerminalTheme {
        name: "gruvbox-dark-hard",
        label: "Gruvbox Dark Hard",
        foreground: "#EBDBB2",
        background: "#1D2021",
        cursor: "#EBDBB2",
        palette: [
            "#1D2021", "#CC241D", "#98971A", "#D79921", "#458588", "#B16286", "#689D6A", "#A89984",
            "#928374", "#FB4934", "#B8BB26", "#FABD2F", "#83A598", "#D3869B", "#8EC07C", "#EBDBB2",
        ],
    },
    TerminalTheme {
        name: "iceberg-dark",
        label: "Iceberg Dark",
        foreground: "#C6C8D1",
        background: "#161821",
        cursor: "#C6C8D1",
        palette: [
            "#1E2132", "#E27878", "#B4BE82", "#E2A478", "#84A0C6", "#A093C7", "#89B8C2", "#C6C8D1",
            "#6B7089", "#E98989", "#C0CA8E", "#E9B189", "#91ACD1", "#ADA0D3", "#95C4CE", "#D2D4DE",
        ],
    },
    TerminalTheme {
        name: "kanagawa-dragon",
        label: "Kanagawa Dragon",
        foreground: "#C5C9C5",
        background: "#181616",
        cursor: "#C8C093",
        palette: [
            "#0D0C0C", "#C4746E", "#8A9A7B", "#C4B28A", "#8BA4B0", "#A292A3", "#8EA4A2", "#C8C093",
            "#A6A69C", "#E46876", "#87A987", "#E6C384", "#7FB4CA", "#938AA9", "#7AA89F", "#C5C9C5",
        ],
    },
    TerminalTheme {
        name: "kanagawa-wave",
        label: "Kanagawa Wave",
        foreground: "#DCD7BA",
        background: "#1F1F28",
        cursor: "#DCD7BA",
        palette: [
            "#090618", "#C34043", "#76946A", "#C0A36E", "#7E9CD8", "#957FB8", "#6A9589", "#C8C093",
            "#727169", "#E82424", "#98BB6C", "#E6C384", "#7FB4CA", "#938AA9", "#7AA89F", "#DCD7BA",
        ],
    },
    TerminalTheme {
        name: "modus-vivendi",
        label: "Modus Vivendi",
        foreground: "#FFFFFF",
        background: "#000000",
        cursor: "#FFFFFF",
        palette: [
            "#000000", "#FF5F59", "#44BC44", "#D0BC00", "#2FAFFF", "#FEACD0", "#00D3D0", "#A6A6A6",
            "#595959", "#FF7F9F", "#00C06F", "#FEC43F", "#79A8FF", "#B6A0FF", "#6AE4B9", "#FFFFFF",
        ],
    },
    TerminalTheme {
        name: "monokai-classic",
        label: "Monokai Classic",
        foreground: "#FDFFF1",
        background: "#272822",
        cursor: "#C0C1B5",
        palette: [
            "#272822", "#F92672", "#A6E22E", "#E6DB74", "#FD971F", "#AE81FF", "#66D9EF", "#FDFFF1",
            "#6E7066", "#F92672", "#A6E22E", "#E6DB74", "#FD971F", "#AE81FF", "#66D9EF", "#FDFFF1",
        ],
    },
    TerminalTheme {
        name: "monokai-pro",
        label: "Monokai Pro",
        foreground: "#FCFCFA",
        background: "#2D2A2E",
        cursor: "#C1C0C0",
        palette: [
            "#2D2A2E", "#FF6188", "#A9DC76", "#FFD866", "#FC9867", "#AB9DF2", "#78DCE8", "#FCFCFA",
            "#727072", "#FF6188", "#A9DC76", "#FFD866", "#FC9867", "#AB9DF2", "#78DCE8", "#FCFCFA",
        ],
    },
    TerminalTheme {
        name: "night-owl",
        label: "Night Owl",
        foreground: "#D6DEEB",
        background: "#011627",
        cursor: "#7E57C2",
        palette: [
            "#011627", "#EF5350", "#22DA6E", "#ADDB67", "#82AAFF", "#C792EA", "#21C7A8", "#FFFFFF",
            "#575656", "#EF5350", "#22DA6E", "#FFEB95", "#82AAFF", "#C792EA", "#7FDBCA", "#FFFFFF",
        ],
    },
    TerminalTheme {
        name: "nightfox",
        label: "Nightfox",
        foreground: "#CDCECF",
        background: "#192330",
        cursor: "#CDCECF",
        palette: [
            "#393B44", "#C94F6D", "#81B29A", "#DBC074", "#719CD6", "#9D79D6", "#63CDCF", "#DFDFE0",
            "#575860", "#D16983", "#8EBAA4", "#E0C989", "#86ABDC", "#BAA1E2", "#7AD5D6", "#E4E4E5",
        ],
    },
    TerminalTheme {
        name: "nord",
        label: "Nord",
        foreground: "#D8DEE9",
        background: "#2E3440",
        cursor: "#D8DEE9",
        palette: [
            "#3B4252", "#BF616A", "#A3BE8C", "#EBCB8B", "#81A1C1", "#B48EAD", "#88C0D0", "#E5E9F0",
            "#4C566A", "#BF616A", "#A3BE8C", "#EBCB8B", "#81A1C1", "#B48EAD", "#8FBCBB", "#ECEFF4",
        ],
    },
    TerminalTheme {
        name: "one-dark",
        label: "One Dark",
        foreground: "#ABB2BF",
        background: "#282C34",
        cursor: "#528BFF",
        palette: [
            "#282C34", "#E06C75", "#98C379", "#E5C07B", "#61AFEF", "#C678DD", "#56B6C2", "#ABB2BF",
            "#545862", "#E06C75", "#98C379", "#E5C07B", "#61AFEF", "#C678DD", "#56B6C2", "#BE5046",
        ],
    },
    TerminalTheme {
        name: "rose-pine",
        label: "Ros\u{e9} Pine",
        foreground: "#E0DEF4",
        background: "#191724",
        cursor: "#E0DEF4",
        palette: [
            "#26233A", "#EB6F92", "#31748F", "#F6C177", "#9CCFD8", "#C4A7E7", "#EBBCBA", "#E0DEF4",
            "#6E6A86", "#EB6F92", "#31748F", "#F6C177", "#9CCFD8", "#C4A7E7", "#EBBCBA", "#E0DEF4",
        ],
    },
    TerminalTheme {
        name: "rose-pine-moon",
        label: "Ros\u{e9} Pine Moon",
        foreground: "#E0DEF4",
        background: "#232136",
        cursor: "#E0DEF4",
        palette: [
            "#393552", "#EB6F92", "#3E8FB0", "#F6C177", "#9CCFD8", "#C4A7E7", "#EA9A97", "#E0DEF4",
            "#6E6A86", "#EB6F92", "#3E8FB0", "#F6C177", "#9CCFD8", "#C4A7E7", "#EA9A97", "#E0DEF4",
        ],
    },
    TerminalTheme {
        name: "solarized-dark",
        label: "Solarized Dark",
        foreground: "#839496",
        background: "#002B36",
        cursor: "#93A1A1",
        palette: [
            "#073642", "#DC322F", "#859900", "#B58900", "#268BD2", "#D33682", "#2AA198", "#EEE8D5",
            "#002B36", "#CB4B16", "#586E75", "#657B83", "#839496", "#6C71C4", "#93A1A1", "#FDF6E3",
        ],
    },
    TerminalTheme {
        name: "tokyo-night",
        label: "Tokyo Night",
        foreground: "#C0CAF5",
        background: "#1A1B26",
        cursor: "#C0CAF5",
        palette: [
            "#15161E", "#F7768E", "#9ECE6A", "#E0AF68", "#7AA2F7", "#BB9AF7", "#7DCFFF", "#A9B1D6",
            "#414868", "#F7768E", "#9ECE6A", "#E0AF68", "#7AA2F7", "#BB9AF7", "#7DCFFF", "#C0CAF5",
        ],
    },
    TerminalTheme {
        name: "tokyo-night-moon",
        label: "Tokyo Night Moon",
        foreground: "#C8D3F5",
        background: "#222436",
        cursor: "#C8D3F5",
        palette: [
            "#1B1D2B", "#FF757F", "#C3E88D", "#FFC777", "#82AAFF", "#C099FF", "#86E1FC", "#828BB8",
            "#444A73", "#FF757F", "#C3E88D", "#FFC777", "#82AAFF", "#C099FF", "#86E1FC", "#C8D3F5",
        ],
    },
    TerminalTheme {
        name: "tokyo-night-storm",
        label: "Tokyo Night Storm",
        foreground: "#C0CAF5",
        background: "#24283B",
        cursor: "#C0CAF5",
        palette: [
            "#1D202F", "#F7768E", "#9ECE6A", "#E0AF68", "#7AA2F7", "#BB9AF7", "#7DCFFF", "#A9B1D6",
            "#4E5575", "#F7768E", "#9ECE6A", "#E0AF68", "#7AA2F7", "#BB9AF7", "#7DCFFF", "#C0CAF5",
        ],
    },
    TerminalTheme {
        name: "tomorrow-night",
        label: "Tomorrow Night",
        foreground: "#C5C8C6",
        background: "#1D1F21",
        cursor: "#C5C8C6",
        palette: [
            "#000000", "#CC6666", "#B5BD68", "#F0C674", "#81A2BE", "#B294BB", "#8ABEB7", "#FFFFFF",
            "#4C4C4C", "#CC6666", "#B5BD68", "#F0C674", "#81A2BE", "#B294BB", "#8ABEB7", "#FFFFFF",
        ],
    },
    // -- Light themes --
    TerminalTheme {
        name: "adwaita",
        label: "Adwaita",
        foreground: "#1D1D20",
        background: "#FFFFFF",
        cursor: "#1D1D20",
        palette: [
            "#1D1D20", "#C01C28", "#26A269", "#A2734C", "#12488B", "#A347BA", "#2AA1B3", "#C2C2C2",
            "#5D5D5D", "#F66151", "#33D17A", "#E9AD0C", "#2A7BDE", "#C061CB", "#33C7DE", "#FFFFFF",
        ],
    },
    TerminalTheme {
        name: "ayu-light",
        label: "Ayu Light",
        foreground: "#5C6166",
        background: "#F8F9FA",
        cursor: "#FFAA33",
        palette: [
            "#000000", "#EA6C6D", "#6CBF43", "#ECA944", "#3199E1", "#9E75C7", "#46BA94", "#BABABA",
            "#686868", "#F07171", "#86B300", "#F2AE49", "#399EE6", "#A37ACC", "#4CBF99", "#D1D1D1",
        ],
    },
    TerminalTheme {
        name: "catppuccin-latte",
        label: "Catppuccin Latte",
        foreground: "#4C4F69",
        background: "#EFF1F5",
        cursor: "#DC8A78",
        palette: [
            "#5C5F77", "#D20F39", "#40A02B", "#DF8E1D", "#1E66F5", "#EA76CB", "#179299", "#ACB0BE",
            "#6C6F85", "#D20F39", "#40A02B", "#DF8E1D", "#1E66F5", "#EA76CB", "#179299", "#4C4F69",
        ],
    },
    TerminalTheme {
        name: "dayfox",
        label: "Dayfox",
        foreground: "#3D2B5A",
        background: "#F6F2EE",
        cursor: "#3D2B5A",
        palette: [
            "#352C24", "#A5222F", "#396847", "#AC5402", "#2848A9", "#6E33CE", "#287980", "#BFB6AE",
            "#534C45", "#B3434E", "#577F63", "#B86E28", "#4863B6", "#8452D5", "#488D93", "#F4ECE6",
        ],
    },
    TerminalTheme {
        name: "everforest-light",
        label: "Everforest Light",
        foreground: "#5C6A72",
        background: "#FDF6E3",
        cursor: "#5C6A72",
        palette: [
            "#5C6A72", "#F85552", "#8DA101", "#DFA000", "#3A94C5", "#DF69BA", "#35A77C", "#E0DCC7",
            "#5C6A72", "#F85552", "#8DA101", "#DFA000", "#3A94C5", "#DF69BA", "#35A77C", "#E0DCC7",
        ],
    },
    TerminalTheme {
        name: "flexoki-light",
        label: "Flexoki Light",
        foreground: "#100F0F",
        background: "#FFFCF0",
        cursor: "#100F0F",
        palette: [
            "#100F0F", "#AF3029", "#66800B", "#AD8301", "#205EA6", "#A02F6F", "#24837B", "#6F6E69",
            "#B7B5AC", "#D14D41", "#879A39", "#D0A215", "#4385BE", "#CE5D97", "#3AA99F", "#CECDC3",
        ],
    },
    TerminalTheme {
        name: "github-light",
        label: "GitHub Light",
        foreground: "#1F2328",
        background: "#FFFFFF",
        cursor: "#0969DA",
        palette: [
            "#24292F", "#CF222E", "#116329", "#4D2D00", "#0969DA", "#8250DF", "#1B7C83", "#6E7781",
            "#57606A", "#A40E26", "#1A7F37", "#633C01", "#218BFF", "#A475F9", "#3192AA", "#8C959F",
        ],
    },
    TerminalTheme {
        name: "gruvbox-light",
        label: "Gruvbox Light",
        foreground: "#3C3836",
        background: "#FBF1C7",
        cursor: "#3C3836",
        palette: [
            "#FBF1C7", "#CC241D", "#98971A", "#D79921", "#458588", "#B16286", "#689D6A", "#7C6F64",
            "#928374", "#9D0006", "#79740E", "#B57614", "#076678", "#8F3F71", "#427B58", "#3C3836",
        ],
    },
    TerminalTheme {
        name: "iceberg-light",
        label: "Iceberg Light",
        foreground: "#33374C",
        background: "#E8E9EC",
        cursor: "#33374C",
        palette: [
            "#DCDFE7", "#CC517A", "#668E3D", "#C57339", "#2D539E", "#7759B4", "#3F83A6", "#33374C",
            "#8389A3", "#CC3768", "#598030", "#B6662D", "#22478E", "#6845AD", "#327698", "#262A3F",
        ],
    },
    TerminalTheme {
        name: "kanagawa-lotus",
        label: "Kanagawa Lotus",
        foreground: "#545464",
        background: "#F2ECBC",
        cursor: "#43436C",
        palette: [
            "#1F1F28", "#C84053", "#6F894E", "#77713F", "#4D699B", "#B35B79", "#597B75", "#545464",
            "#8A8980", "#D7474B", "#6E915F", "#836F4A", "#6693BF", "#624C83", "#5E857A", "#43436C",
        ],
    },
    TerminalTheme {
        name: "modus-operandi",
        label: "Modus Operandi",
        foreground: "#000000",
        background: "#FFFFFF",
        cursor: "#000000",
        palette: [
            "#000000", "#A60000", "#006800", "#6F5500", "#0031A9", "#721045", "#005E8B", "#A6A6A6",
            "#595959", "#972500", "#00663F", "#884900", "#3548CF", "#531AB6", "#005F5F", "#595959",
        ],
    },
    TerminalTheme {
        name: "one-light",
        label: "One Light",
        foreground: "#2A2C33",
        background: "#F9F9F9",
        cursor: "#BBBBBB",
        palette: [
            "#000000", "#DE3E35", "#3F953A", "#D2B67C", "#2F5AF3", "#950095", "#3F953A", "#BBBBBB",
            "#000000", "#DE3E35", "#3F953A", "#D2B67C", "#2F5AF3", "#A00095", "#3F953A", "#FFFFFF",
        ],
    },
    TerminalTheme {
        name: "rose-pine-dawn",
        label: "Ros\u{e9} Pine Dawn",
        foreground: "#575279",
        background: "#FAF4ED",
        cursor: "#575279",
        palette: [
            "#F2E9E1", "#B4637A", "#286983", "#EA9D34", "#56949F", "#907AA9", "#D7827E", "#575279",
            "#9893A5", "#B4637A", "#286983", "#EA9D34", "#56949F", "#907AA9", "#D7827E", "#575279",
        ],
    },
    TerminalTheme {
        name: "solarized-light",
        label: "Solarized Light",
        foreground: "#657B83",
        background: "#FDF6E3",
        cursor: "#586E75",
        palette: [
            "#073642", "#DC322F", "#859900", "#B58900", "#268BD2", "#D33682", "#2AA198", "#EEE8D5",
            "#002B36", "#CB4B16", "#586E75", "#657B83", "#839496", "#6C71C4", "#93A1A1", "#FDF6E3",
        ],
    },
    TerminalTheme {
        name: "tokyo-night-day",
        label: "Tokyo Night Day",
        foreground: "#3760BF",
        background: "#E1E2E7",
        cursor: "#3760BF",
        palette: [
            "#E9E9ED", "#F52A65", "#587539", "#8C6C3E", "#2E7DE9", "#9854F1", "#007197", "#6172B0",
            "#A1A6C5", "#F52A65", "#587539", "#8C6C3E", "#2E7DE9", "#9854F1", "#007197", "#3760BF",
        ],
    },
    TerminalTheme {
        name: "tomorrow",
        label: "Tomorrow",
        foreground: "#4D4D4C",
        background: "#FFFFFF",
        cursor: "#4D4D4C",
        palette: [
            "#000000", "#C82829", "#718C00", "#EAB700", "#4271AE", "#8959A8", "#3E999F", "#BFBFBF",
            "#000000", "#C82829", "#718C00", "#EAB700", "#4271AE", "#8959A8", "#3E999F", "#FFFFFF",
        ],
    },
];

pub fn terminal_theme(name: &str) -> &'static TerminalTheme {
    TERMINAL_THEMES
        .iter()
        .find(|t| t.name == name)
        .unwrap_or(&TERMINAL_THEMES[0])
}

pub fn theme_choices() -> Vec<&'static str> {
    TERMINAL_THEMES.iter().map(|t| t.label).collect()
}

pub fn theme_index(name: &str) -> u32 {
    TERMINAL_THEMES
        .iter()
        .position(|t| t.name == name)
        .unwrap_or(0) as u32
}

pub fn theme_name(index: u32) -> &'static str {
    TERMINAL_THEMES
        .get(index as usize)
        .map(|t| t.name)
        .unwrap_or("catppuccin-mocha")
}

/// Parse "#rrggbb" into 0.0-1.0 channels. Data in this file is compile-time
/// constant and well-formed; unknown input gets black rather than a panic.
pub fn hex_rgb(hex: &str) -> (f32, f32, f32) {
    let h = hex.trim_start_matches('#');
    let chan = |i: usize| {
        u8::from_str_radix(h.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0) as f32 / 255.0
    };
    (chan(0), chan(2), chan(4))
}

pub fn is_dark_theme(name: &str) -> bool {
    let (r, g, b) = hex_rgb(terminal_theme(name).background);
    // Luminance approximation
    (0.299 * r + 0.587 * g + 0.114 * b) < 0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Relative luminance / WCAG contrast, so the palette's readability is
    /// asserted rather than eyeballed.
    fn luminance(hex: &str) -> f64 {
        let h = hex.trim_start_matches('#');
        let chan = |i: usize| {
            let v = u8::from_str_radix(&h[i..i + 2], 16).expect("hex pair") as f64 / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * chan(0) + 0.7152 * chan(2) + 0.0722 * chan(4)
    }

    fn contrast(a: &str, b: &str) -> f64 {
        let (x, y) = (luminance(a), luminance(b));
        let (hi, lo) = if x > y { (x, y) } else { (y, x) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// The surface these are read against. `.sidebar` is
    /// `alpha(@window_bg_color, 0.97)`, which screenshots measure as
    /// #fafafa light / #222226 dark — both further from the foreground
    /// than the values below, so asserting against these keeps a margin.
    const LIGHT_SIDEBAR: &str = "#ebebeb";
    const DARK_SIDEBAR: &str = "#303030";
    /// Text-sized UI needs 4.5:1; allow a hair under for the derived hues.
    const MIN_CONTRAST: f64 = 4.0;
    /// A status dot is a graphic, not text — WCAG asks 3:1 of it.
    const MIN_DOT_CONTRAST: f64 = 3.0;

    #[test]
    fn every_accent_is_readable_in_both_schemes() {
        for c in ACCENT_COLORS {
            let dark = contrast(c.accent, DARK_SIDEBAR);
            let light = contrast(c.accent_light, LIGHT_SIDEBAR);
            assert!(
                dark >= MIN_CONTRAST,
                "{} dark accent {} is {dark:.2}:1 on {DARK_SIDEBAR}",
                c.name,
                c.accent
            );
            assert!(
                light >= MIN_CONTRAST,
                "{} light accent {} is {light:.2}:1 on {LIGHT_SIDEBAR}",
                c.name,
                c.accent_light
            );
        }
    }

    /// The fixed status hues have no picker to escape a bad scheme with,
    /// so they carry the same burden of proof as the palette.
    #[test]
    fn status_dots_are_visible_in_both_schemes() {
        for (name, dark_hex, light_hex) in STATUS_COLORS {
            let dark = contrast(dark_hex, DARK_SIDEBAR);
            let light = contrast(light_hex, LIGHT_SIDEBAR);
            assert!(
                dark >= MIN_DOT_CONTRAST,
                "{name} dark {dark_hex} is {dark:.2}:1 on {DARK_SIDEBAR}"
            );
            assert!(
                light >= MIN_DOT_CONTRAST,
                "{name} light {light_hex} is {light:.2}:1 on {LIGHT_SIDEBAR}"
            );
        }
    }

    #[test]
    fn theme_lookup_falls_back_to_default() {
        assert_eq!(terminal_theme("no-such-theme").name, "catppuccin-mocha");
        assert!(is_dark_theme("catppuccin-mocha"));
        assert!(!is_dark_theme("solarized-light"));
    }

    /// `hex_rgb` turns a malformed colour into black rather than failing,
    /// and the picker maps labels back to names, so a typo in the table or
    /// a duplicate would ship silently.
    #[test]
    fn terminal_themes_are_well_formed() {
        let hex = |s: &str| {
            s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit())
        };
        let mut names = std::collections::HashSet::new();
        let mut labels = std::collections::HashSet::new();
        for t in TERMINAL_THEMES {
            assert!(names.insert(t.name), "duplicate name {}", t.name);
            assert!(labels.insert(t.label), "duplicate label {}", t.label);
            for c in [t.foreground, t.background, t.cursor]
                .into_iter()
                .chain(t.palette)
            {
                assert!(hex(c), "{}: bad colour {c}", t.name);
            }
            let fg = contrast(t.foreground, t.background);
            assert!(fg >= 4.0, "{}: foreground is {fg:.2}:1", t.name);
        }
    }

    /// The picker is a flat list: default first, then dark, then light.
    #[test]
    fn terminal_themes_run_dark_then_light() {
        let first_light = TERMINAL_THEMES
            .iter()
            .position(|t| !is_dark_theme(t.name))
            .expect("a light theme");
        assert!(
            TERMINAL_THEMES[first_light..]
                .iter()
                .all(|t| !is_dark_theme(t.name))
        );
    }

    #[test]
    fn hex_parses() {
        assert_eq!(hex_rgb("#ffffff"), (1.0, 1.0, 1.0));
        assert_eq!(hex_rgb("#000000"), (0.0, 0.0, 0.0));
    }
}
