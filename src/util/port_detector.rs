use std::collections::HashMap;

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

    pub fn scan_output(&mut self, process_name: &str, text: &str) {
        let mut found = Vec::new();

        // Match patterns like:
        // localhost:3000, 127.0.0.1:8080, 0.0.0.0:5000
        // http://localhost:3000, https://...
        for raw_word in text.split_whitespace() {
            // Strip surrounding brackets, parens, quotes, punctuation
            let word = raw_word.trim_matches(|c: char| "[](){}\"'`,;.!".contains(c));

            // Full URL pattern
            if word.starts_with("http://") || word.starts_with("https://") {
                if let Some(port) = extract_port_from_url(word) {
                    found.push(DetectedPort {
                        port,
                        url: Some(word.to_string()),
                    });
                }
            }
            // localhost:PORT pattern
            else if let Some(port_str) = word.strip_prefix("localhost:") {
                if let Ok(port) = port_str
                    .trim_matches(|c: char| !c.is_numeric())
                    .parse::<u16>()
                {
                    found.push(DetectedPort {
                        port,
                        url: Some(format!("http://localhost:{port}")),
                    });
                }
            }
            // 0.0.0.0:PORT or 127.0.0.1:PORT
            else if word.starts_with("0.0.0.0:") || word.starts_with("127.0.0.1:") {
                let parts: Vec<&str> = word.splitn(2, ':').collect();
                if parts.len() == 2
                    && let Ok(port) = parts[1]
                        .trim_matches(|c: char| !c.is_numeric())
                        .parse::<u16>()
                {
                    found.push(DetectedPort {
                        port,
                        url: Some(format!("http://localhost:{port}")),
                    });
                }
            }
            // "port NNNN" or "Port NNNN"
            else if word.eq_ignore_ascii_case("port") {
                // Port number might be next word — handled below
            }
        }

        // Also check "port NNNN" patterns
        let lower = text.to_lowercase();
        if let Some(idx) = lower.find("port ") {
            let after = &text[idx + 5..];
            let port_str: String = after.chars().take_while(|c| c.is_numeric()).collect();
            if let Ok(port) = port_str.parse::<u16>()
                && port > 0
                && !found.iter().any(|f| f.port == port)
            {
                found.push(DetectedPort {
                    port,
                    url: Some(format!("http://localhost:{port}")),
                });
            }
        }

        if !found.is_empty() {
            self.ports.insert(process_name.to_string(), found);
        }
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

fn extract_port_from_url(url: &str) -> Option<u16> {
    // Find port after host:port pattern
    let without_scheme = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;

    // Split host:port/path
    let host_port = without_scheme.split('/').next()?;
    let parts: Vec<&str> = host_port.rsplitn(2, ':').collect();
    if parts.len() == 2 {
        parts[0].parse::<u16>().ok()
    } else {
        None
    }
}
