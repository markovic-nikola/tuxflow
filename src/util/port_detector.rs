use std::collections::HashMap;

/// Bracket-prefix tags (concurrently-style `[name]`) that identify a build-tool line.
const TOOL_PREFIXES: &[&str] = &[
    "vite",
    "webpack",
    "parcel",
    "esbuild",
    "turbopack",
    "rollup",
    "snowpack",
];

/// Line labels that mark a URL as the process's primary user-facing page.
/// Matched case-insensitively as substrings. A labeled URL wins the badge
/// even over local URLs on other lines — `shopify app dev` prints a public
/// admin "Preview URL:" alongside a localhost GraphiQL URL, and
/// open-in-browser should open the preview. Public URLs open locally as-is;
/// the localhost ports on the other lines still get harvested for tunnels.
const PREFERRED_URL_LABELS: &[&str] = &["preview url"];

/// Phrases marking a line whose port belongs to someone *else* — a neighbour
/// already holding it, or one this process just failed to take. Such lines are
/// skipped outright: they must not badge, and unlike tool lines their ports
/// must not harvest for tunnelling either. Forwarding a port another project's
/// server owns is worse than missing it — the badge opens, and silently serves
/// the wrong app under this project's name.
///
/// Both dev servers announce the retry *and* the port they retried onto, so
/// dropping the failure line loses nothing: the line that follows carries the
/// real port.
const FOREIGN_PORT_PHRASES: &[&str] = &[
    // Covers every phrasing of "someone already holds this port":
    //   vite:  "Port 5173 is in use, trying another one..."
    //   bun:   "error: Failed to start server. Is port 3000 in use?"
    //   PHP:   "... (reason: Address already in use)"
    // Match the bare phrase, not each dev server's sentence around it —
    // "is in use" missed bun's, whose word order puts the port in between.
    "in use",
    // PHP's built-in server, as `php artisan serve` walks --tries ports:
    // "Failed to listen on 127.0.0.1:8000 (reason: Address already in use)".
    // Needed on its own: the multiplex TUI clips the line before the reason.
    "failed to listen",
    // bun/node again, for phrasings that name the port without "in use".
    // A server that failed to start owns no port, whatever the reason.
    "failed to start server",
    "eaddrinuse",
];

/// Content phrases that identify a build-tool line even without a bracket prefix.
/// Matched case-insensitively as substrings.
const TOOL_CONTENT_PHRASES: &[&str] = &[
    "vite v",
    "[hmr]",
    "webpack-dev-server",
    // (vite's "is in use, trying another" lives in FOREIGN_PORT_PHRASES —
    // those lines are skipped before tool-line classification is consulted)
    // (vite's arrowed "➜  Local:" is matched by `is_arrowed_label`, which
    // doesn't care which arrow glyph or how much padding vite chose)
    // `shopify app dev` infra lines: its proxy/graphiql servers start
    // seconds before the "Preview URL:" panel prints, and must not lock
    // the badge (their ports are still harvested for tunnelling).
    "proxy server started",
    "graphiql server started",
    // Error lines quoting an unreachable URL are not the app's address.
    "econnrefused",
    "unreachable target",
];

pub struct PortDetector {
    ports: HashMap<String, Vec<DetectedPort>>,
    /// Every distinct local port seen in a process's output — including
    /// build-tool lines (vite &c.) that are excluded from the badge choice.
    /// Remote projects tunnel all of these: the app port alone isn't enough
    /// when e.g. a theme proxy on one port loads assets from vite on another.
    seen_local: HashMap<String, Vec<u16>>,
}

#[derive(Clone, Debug)]
pub struct DetectedPort {
    pub port: u16,
    pub url: Option<String>,
    /// Whether the URL/port points at the machine the process runs on
    /// (localhost/127.0.0.1/0.0.0.0) rather than some other host.
    pub local: bool,
    /// Whether the URL came from a line labeled as the primary page
    /// (see PREFERRED_URL_LABELS) — beats local detections for the badge.
    pub preferred: bool,
    /// Whether this came off a build-tool line (vite's `➜ Local:` &c.). Such a
    /// port badges only as a last resort, and never locks the badge — the app's
    /// own port, printed later, still replaces it.
    pub tool: bool,
}

impl Default for PortDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl PortDetector {
    pub fn new() -> Self {
        Self {
            ports: HashMap::new(),
            seen_local: HashMap::new(),
        }
    }

    /// Returns true once any port (local or provisional remote) is known.
    pub fn has_port(&self, process_name: &str) -> bool {
        self.ports.get(process_name).is_some_and(|v| !v.is_empty())
    }

    /// Returns true when the badge points at a genuinely local port —
    /// the gate for tunnelling and the port badge. A public-URL badge
    /// (provisional or preferred) has nothing to forward to.
    pub fn has_local_port(&self, process_name: &str) -> bool {
        self.ports
            .get(process_name)
            .and_then(|v| v.first())
            .is_some_and(|p| p.local)
    }

    /// Returns true once the badge is final — a local port locked, or a
    /// labeled preferred URL won. Callers can stop scanning. A plain
    /// remote-URL fallback doesn't count: it stays provisional so a later
    /// local URL can replace it, and neither does a build-tool port: vite
    /// prints its address before the app server prints its own.
    pub fn badge_final(&self, process_name: &str) -> bool {
        self.ports
            .get(process_name)
            .and_then(|v| v.first())
            .is_some_and(|p| p.preferred || (p.local && !p.tool))
    }

    /// All distinct local ports seen in this process's output, in first-seen
    /// order — the full set a remote project needs tunnelled.
    pub fn all_local_ports(&self, process_name: &str) -> Vec<u16> {
        self.seen_local
            .get(process_name)
            .cloned()
            .unwrap_or_default()
    }

    /// Forget the port for a process so the next scan re-detects.
    /// Call on stop/restart.
    pub fn clear(&mut self, process_name: &str) {
        self.ports.remove(process_name);
        self.seen_local.remove(process_name);
    }

    /// Scan complete logical lines. Local VTE text extraction joins its own
    /// soft wraps, and tmux *history* is captured with `-J` — but a live
    /// tmux screen redraw emits wrapped long lines as separate hard rows;
    /// use [`Self::scan_output_wrapped`] for that case.
    pub fn scan_output(&mut self, process_name: &str, text: &str) {
        self.scan_output_wrapped(process_name, text, usize::MAX);
    }

    /// Like [`Self::scan_output`], but first re-joins lines hard-wrapped at
    /// `cols` — a row filled to the last column with non-space content is
    /// a fragment of a longer logical line. Without this, the admin
    /// "Preview URL:" of `shopify app dev` (which wraps on any normal-width
    /// terminal) is detected truncated and opens a broken page.
    pub fn scan_output_wrapped(&mut self, process_name: &str, text: &str, cols: usize) {
        let text = join_hard_wraps(text, cols);
        let text = text.as_ref();
        // Badge stickiness: a local detection or a labeled preferred URL
        // is final; a plain remote-URL fallback is provisional (an OAuth
        // link printed during startup must not shadow the real dev-server
        // URL appearing later). But
        // port *harvesting* into seen_local is monotonic — a scan arriving
        // after the badge locked (e.g. the reattach history seed racing a
        // partial-redraw screen scan) must still register secondary ports,
        // or their tunnels never come up.
        let badge_locked = self.badge_final(process_name);

        let mut preferred: Vec<DetectedPort> = Vec::new();
        let mut local: Vec<DetectedPort> = Vec::new();
        let mut tool: Vec<DetectedPort> = Vec::new();
        let mut remote: Vec<DetectedPort> = Vec::new();

        for line in text.lines() {
            let lower = line.to_ascii_lowercase();
            if FOREIGN_PORT_PHRASES.iter().any(|p| lower.contains(p)) {
                continue;
            }

            let mut line_local: Vec<DetectedPort> = Vec::new();
            let mut line_remote: Vec<DetectedPort> = Vec::new();
            scan_line(line, &mut line_local, &mut line_remote);

            // Tool lines don't compete for the badge, but their local ports
            // (vite asset server &c.) still count for tunnelling.
            let seen = self.seen_local.entry(process_name.to_string()).or_default();
            for d in &line_local {
                if !seen.contains(&d.port) {
                    seen.push(d.port);
                }
            }

            if is_preferred_line(line) {
                // Post-join line: if this logs truncated, the wrap join
                // didn't fire — compare the char count against the cols
                // the caller passed.
                log::debug!(
                    "preferred-label line for {process_name} ({} chars, cols={cols}): {line:?}",
                    line.chars().count()
                );
                for d in line_local.iter().chain(line_remote.iter()) {
                    preferred.push(DetectedPort {
                        preferred: true,
                        ..d.clone()
                    });
                }
            }

            if is_tool_line(line) {
                // Last-resort tier: when the app never announces a port of its
                // own, vite's is the only address the user can open. It stays
                // upgradeable (see `badge_final`), so the app port still wins
                // whenever it does arrive.
                //
                // Only *address* lines qualify — a dev server saying where to
                // point a browser. The rest of the tool class is plumbing
                // (`shopify app dev`'s proxy/graphiql servers, HMR chatter),
                // and auto-opening those is the exact bug this tier must not
                // reintroduce.
                if is_local_address_line(line) {
                    tool.extend(
                        line_local
                            .into_iter()
                            .map(|d| DetectedPort { tool: true, ..d }),
                    );
                }
            } else {
                local.extend(line_local);
                remote.extend(line_remote);
            }
        }

        if badge_locked {
            return; // seen_local harvested above; badge already final
        }

        let chosen = if !preferred.is_empty() {
            preferred
        } else if !local.is_empty() {
            local
        } else if !tool.is_empty() {
            tool
        } else if !remote.is_empty() && !self.has_port(process_name) {
            remote
        } else {
            return;
        };

        self.ports.insert(process_name.to_string(), chosen);
    }

    pub fn get_port(&self, process_name: &str) -> Option<u16> {
        self.ports
            .get(process_name)
            .and_then(|ports| ports.first())
            .map(|p| p.port)
    }

    pub fn get_url(&self, process_name: &str) -> Option<&str> {
        self.ports
            .get(process_name)
            .and_then(|ports| ports.first())
            .and_then(|p| p.url.as_deref())
    }
}

/// Re-join lines that were hard-wrapped at the terminal width. A row of
/// `cols` — or `cols - 1`: Ink-based CLIs (shopify) wrap one short of the
/// width, with a hard newline that even tmux `capture-pane -J` can't
/// rejoin — ending in non-space content continues on the next row. A wrap
/// that split at a space leaves the row shorter (or space-terminated), so
/// no token spanned the boundary and the lines stay separate. False joins
/// (a genuinely full row followed by an unrelated line) only merge the two
/// words at the junction — every other word on both lines still scans
/// normally. Lines *longer* than `cols` (already joined by `-J`) never
/// continue.
/// Whether a row ends on a box-drawing glyph — a frame edge, not a wrap.
///
/// Measuring width alone cannot tell the two apart, and full-screen TUIs make
/// that fatal: `php artisan dev` runs `@laravel/multiplex`, which borders the
/// pane, so *every* row is exactly full width. Joining them fuses the whole
/// screen into one logical line, and a single "Failed to listen" there
/// swallows the real port announced three rows below — the badge comes back
/// empty and nothing is tunnelled. Ink's hard wraps break mid-token and never
/// land on a border, so they still join.
fn ends_on_frame_edge(line: &str) -> bool {
    // Box Drawing (U+2500–U+257F) and Block Elements (U+2580–U+259F).
    matches!(line.chars().next_back(), Some(c) if ('\u{2500}'..='\u{259F}').contains(&c))
}

fn join_hard_wraps(text: &str, cols: usize) -> std::borrow::Cow<'_, str> {
    if cols < 2 || cols == usize::MAX {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut last_line_start = 0usize;
    let mut continuation = false;
    let mut cur_indent = 0usize;
    for raw in text.lines() {
        // VTE may pad extracted rows with blank cells out to the terminal
        // width — trim before measuring, or a wrapped row "ends with a
        // space" and never joins, and blank rows below a frame are
        // all-spaces instead of empty.
        let line = raw.trim_end();
        if continuation && line.is_empty() {
            // Blank row right after a full-width row: the content edge of
            // a mid-frame render (or a line that ended exactly at the
            // width). Either way there is no continuation to append —
            // drop the fragment rather than guess; a complete render
            // arrives on a later scan.
            out.truncate(last_line_start);
            continuation = false;
            continue;
        }
        if continuation {
            // A wrap inside a left-padded box (Ink panels indent content
            // by a space) resumes after the padding — strip the parent
            // line's indent from the continuation, or the padding space
            // lands inside the rejoined token and truncates the URL at
            // exactly the wrap point.
            let mut rest = line;
            for _ in 0..cur_indent {
                match rest.strip_prefix(' ') {
                    Some(r) => rest = r,
                    None => break,
                }
            }
            out.push_str(rest);
        } else {
            if !out.is_empty() {
                out.push('\n');
                last_line_start = out.len();
            }
            cur_indent = line.chars().take_while(|c| *c == ' ').count();
            out.push_str(line);
        }
        let width = line.chars().count();
        continuation = (width == cols || width == cols - 1) && !ends_on_frame_edge(line);
    }
    if continuation {
        // The final logical line still ends in a full-width row — its
        // continuation may simply not be rendered/scanned yet (a scan can
        // land mid-frame, and a wrapped URL detected from its first row
        // alone would lock the badge truncated). Drop it; the next scan
        // sees it whole.
        out.truncate(last_line_start);
    }
    std::borrow::Cow::Owned(out)
}

fn is_preferred_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    PREFERRED_URL_LABELS.iter().any(|l| lower.contains(l))
}

/// A line's content with a leading concurrently-style `[tag]` removed, so the
/// same shape matches whether a dev server runs alone or under a multiplexer
/// (`  ➜  Local: …` vs `[vite]   ➜  Local: …`).
fn without_tag(line: &str) -> &str {
    let trimmed = line.trim_start();
    match trimmed
        .strip_prefix('[')
        .and_then(|r| r.find(']').map(|i| i + 1))
    {
        Some(end) => trimmed[end..].trim_start(),
        None => trimmed,
    }
}

/// Whether a line advertises the address a browser should open — vite's
/// `➜  Local:   http://localhost:5173/`. Tool lines don't win the badge, but
/// this subset of them is the fallback when nothing else claims it.
pub(crate) fn is_local_address_line(line: &str) -> bool {
    is_arrowed_label(without_tag(line), "local:")
}

fn is_tool_line(line: &str) -> bool {
    let trimmed = line.trim_start();

    if let Some(rest) = trimmed.strip_prefix('[')
        && let Some(close_idx) = rest.find(']')
    {
        let tag = rest[..close_idx].trim().to_ascii_lowercase();
        if TOOL_PREFIXES.iter().any(|t| *t == tag) {
            return true;
        }
    }

    if is_local_address_line(trimmed) {
        return true;
    }

    let lower = trimmed.to_ascii_lowercase();
    for phrase in TOOL_CONTENT_PHRASES {
        if lower.contains(phrase) {
            return true;
        }
    }

    false
}

/// Whether a line is a dev-server summary row — an arrow glyph, padding, then
/// `label` (`  ➜  Local:   http://localhost:5173/`).
///
/// Spelling these as literal phrases meant carrying one per arrow-and-padding
/// combination, and the set was never complete: vite prints `➜` under a
/// Unicode-capable font and `→` otherwise, so the same project badged 5173 or
/// nothing depending on a glyph. Match the shape instead.
fn is_arrowed_label(trimmed: &str, label: &str) -> bool {
    // Arrows (U+2190–U+21FF), dingbat arrows (U+2794–U+27BF), plus ASCII `->`.
    let rest = match trimmed.chars().next() {
        Some(c)
            if ('\u{2190}'..='\u{21FF}').contains(&c) || ('\u{2794}'..='\u{27BF}').contains(&c) =>
        {
            &trimmed[c.len_utf8()..]
        }
        _ => match trimmed.strip_prefix("->") {
            Some(r) => r,
            None => return false,
        },
    };
    rest.trim_start().to_ascii_lowercase().starts_with(label)
}

fn scan_line(line: &str, local: &mut Vec<DetectedPort>, remote: &mut Vec<DetectedPort>) {
    for raw_word in line.split_whitespace() {
        let word = raw_word.trim_matches(|c: char| "[](){}\"'`,;.!".contains(c));

        if word.starts_with("http://") || word.starts_with("https://") {
            if let Some((host, port)) = extract_host_port_from_url(word) {
                let is_local = is_local_host(&host);
                let detected = DetectedPort {
                    port,
                    url: Some(word.to_string()),
                    local: is_local,
                    preferred: false,
                    tool: false,
                };
                if is_local {
                    local.push(detected);
                } else {
                    remote.push(detected);
                }
            }
            continue;
        }

        if let Some(port_str) = word.strip_prefix("localhost:")
            && let Ok(port) = port_str
                .trim_matches(|c: char| !c.is_numeric())
                .parse::<u16>()
        {
            local.push(DetectedPort {
                port,
                url: Some(format!("http://localhost:{port}")),
                local: true,
                preferred: false,
                tool: false,
            });
            continue;
        }

        if word.starts_with("0.0.0.0:") || word.starts_with("127.0.0.1:") {
            let parts: Vec<&str> = word.splitn(2, ':').collect();
            if parts.len() == 2
                && let Ok(port) = parts[1]
                    .trim_matches(|c: char| !c.is_numeric())
                    .parse::<u16>()
            {
                local.push(DetectedPort {
                    port,
                    url: Some(format!("http://localhost:{port}")),
                    local: true,
                    preferred: false,
                    tool: false,
                });
            }
            continue;
        }
    }

    let lower = line.to_lowercase();
    if let Some(idx) = lower.find("port ") {
        let after = &line[idx + 5..];
        let port_str: String = after.chars().take_while(|c| c.is_numeric()).collect();
        if let Ok(port) = port_str.parse::<u16>()
            && port > 0
            && !local.iter().any(|f| f.port == port)
            && !remote.iter().any(|f| f.port == port)
        {
            local.push(DetectedPort {
                port,
                url: Some(format!("http://localhost:{port}")),
                local: true,
                preferred: false,
                tool: false,
            });
        }
    }
}

fn is_local_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "0.0.0.0")
}

pub(crate) fn extract_host_port_from_url(url: &str) -> Option<(String, u16)> {
    let (without_scheme, default_port) = if let Some(rest) = url.strip_prefix("http://") {
        (rest, 80u16)
    } else if let Some(rest) = url.strip_prefix("https://") {
        (rest, 443u16)
    } else {
        return None;
    };

    let host_port = without_scheme.split('/').next()?;
    let parts: Vec<&str> = host_port.rsplitn(2, ':').collect();

    if parts.len() == 2 {
        let host = parts[1].to_string();
        let port = parts[0].parse::<u16>().ok()?;
        Some((host, port))
    } else {
        Some((host_port.to_string(), default_port))
    }
}

/// Rewrite `:{from}` to `:{to}` in a URL, but only where the port number
/// actually ends there — a plain string replace would corrupt
/// `http://localhost:8080` when remapping port 80. Used when a remote
/// project's detected port is tunnelled to a different local port.
pub fn remap_url_port(url: &str, from: u16, to: u16) -> String {
    let needle = format!(":{from}");
    let mut out = String::with_capacity(url.len());
    let mut rest = url;
    while let Some(i) = rest.find(&needle) {
        let end = i + needle.len();
        let next_is_digit = rest[end..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit());
        if next_is_digit {
            // Prefix of a longer number ("":80" inside ":8080") — keep it
            out.push_str(&rest[..end]);
        } else {
            out.push_str(&rest[..i]);
            out.push_str(&format!(":{to}"));
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}
