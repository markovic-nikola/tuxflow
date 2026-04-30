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
}

#[derive(Clone, Debug)]
pub struct DetectedPort {
    pub port: u16,
    pub url: Option<String>,
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
        }
    }

    /// Returns true once a port has been locked for this process.
    /// Callers can use this to skip expensive scans.
    pub fn has_port(&self, process_name: &str) -> bool {
        self.ports.get(process_name).is_some_and(|v| !v.is_empty())
    }

    /// Forget the port for a process so the next scan re-detects.
    /// Call on stop/restart.
    pub fn clear(&mut self, process_name: &str) {
        self.ports.remove(process_name);
    }

    pub fn scan_output(&mut self, process_name: &str, text: &str) {
        // Stickiness: once a port is chosen, keep it until clear().
        if self.has_port(process_name) {
            return;
        }

        let mut local: Vec<DetectedPort> = Vec::new();
        let mut remote: Vec<DetectedPort> = Vec::new();

        for line in text.lines() {
            if is_tool_line(line) {
                continue;
            }
            scan_line(line, &mut local, &mut remote);
        }

        let chosen = if !local.is_empty() {
            local
        } else if !remote.is_empty() {
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
                let detected = DetectedPort {
                    port,
                    url: Some(word.to_string()),
                };
                if is_local_host(&host) {
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
