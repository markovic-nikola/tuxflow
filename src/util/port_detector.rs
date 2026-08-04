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

/// Content phrases that identify a build-tool line even without a bracket prefix.
/// Matched case-insensitively as substrings.
const TOOL_CONTENT_PHRASES: &[&str] = &[
    "vite v",
    "[hmr]",
    "webpack-dev-server",
    "is in use, trying another",
    "→ local:",
    "➜  local:",
    "➜ local:",
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

    /// Returns true once a *local* port is locked — the final state; callers
    /// can stop scanning. A remote-URL fallback doesn't count: it stays
    /// provisional so a later local URL can replace it.
    pub fn has_local_port(&self, process_name: &str) -> bool {
        self.ports
            .get(process_name)
            .and_then(|v| v.first())
            .is_some_and(|p| p.local)
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

    pub fn scan_output(&mut self, process_name: &str, text: &str) {
        // Badge stickiness: a local detection is final; a remote-URL
        // fallback is provisional (an OAuth link printed during startup
        // must not shadow the real dev-server URL appearing later). But
        // port *harvesting* into seen_local is monotonic — a scan arriving
        // after the badge locked (e.g. the reattach history seed racing a
        // partial-redraw screen scan) must still register secondary ports,
        // or their tunnels never come up.
        let badge_locked = self.has_local_port(process_name);

        let mut local: Vec<DetectedPort> = Vec::new();
        let mut remote: Vec<DetectedPort> = Vec::new();

        for line in text.lines() {
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

            if !is_tool_line(line) {
                local.extend(line_local);
                remote.extend(line_remote);
            }
        }

        if badge_locked {
            return; // seen_local harvested above; badge already final
        }

        let chosen = if !local.is_empty() {
            local
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

    let lower = trimmed.to_ascii_lowercase();
    for phrase in TOOL_CONTENT_PHRASES {
        if lower.contains(phrase) {
            return true;
        }
    }

    false
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
            });
        }
    }
}

fn is_local_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "0.0.0.0")
}

fn extract_host_port_from_url(url: &str) -> Option<(String, u16)> {
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
