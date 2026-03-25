use std::time::Duration;

pub struct UpdateInfo {
    pub latest_version: String,
    pub release_url: String,
}

pub fn check_for_update() -> Option<UpdateInfo> {
    let current = env!("CARGO_PKG_VERSION");

    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(5)))
        .build();
    let agent: ureq::Agent = config.into();

    let mut response = agent
        .get("https://api.github.com/repos/markovic-nikola/tuxflow/releases/latest")
        .header("Accept", "application/vnd.github.v3+json")
        .header("User-Agent", &format!("tuxflow/{current}"))
        .call()
        .ok()?;

    let body: serde_json::Value = response.body_mut().read_json().ok()?;
    let tag = body.get("tag_name")?.as_str()?;
    let latest = tag.strip_prefix('v').unwrap_or(tag);
    let url = body
        .get("html_url")
        .and_then(|v: &serde_json::Value| v.as_str())
        .unwrap_or("https://github.com/markovic-nikola/tuxflow/releases")
        .to_string();

    if is_newer(latest, current) {
        Some(UpdateInfo {
            latest_version: latest.to_string(),
            release_url: url,
        })
    } else {
        None
    }
}

fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |v: &str| -> Vec<u32> {
        v.split('.').filter_map(|s| s.parse().ok()).collect()
    };
    let l = parse(latest);
    let c = parse(current);
    l > c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_newer() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
    }
}
