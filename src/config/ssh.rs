use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct SshHost {
    pub name: String,
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
}

impl SshHost {
    pub fn to_ssh_command(&self) -> String {
        let mut parts = vec!["ssh".to_string()];

        if let Some(port) = self.port {
            if port != 22 {
                parts.push("-p".to_string());
                parts.push(port.to_string());
            }
        }

        if let Some(ref identity) = self.identity_file {
            parts.push("-i".to_string());
            parts.push(identity.clone());
        }

        // Use hostname if available, otherwise fall back to the alias name
        let host = self.hostname.as_deref().unwrap_or(&self.name);
        if let Some(ref user) = self.user {
            parts.push(format!("{user}@{host}"));
        } else {
            parts.push(host.to_string());
        }

        parts.join(" ")
    }
}

fn ssh_config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".ssh").join("config"))
}

pub fn parse_ssh_config() -> Vec<SshHost> {
    let Some(path) = ssh_config_path() else {
        return Vec::new();
    };
    let Ok(content) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    parse_ssh_config_from_str(&content)
}

pub fn parse_ssh_config_from_str(content: &str) -> Vec<SshHost> {
    let mut hosts = Vec::new();
    let mut current: Option<SshHost> = None;

    for line in content.lines() {
        let line = line.trim();

        // Skip comments and empty lines
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Split on first whitespace or '='
        let (key, value) = match line.split_once(|c: char| c.is_whitespace() || c == '=') {
            Some((k, v)) => (k.trim(), v.trim()),
            None => continue,
        };

        match key.to_lowercase().as_str() {
            "host" => {
                // Save previous host if any
                if let Some(host) = current.take() {
                    hosts.push(host);
                }
                // Skip wildcard patterns
                if value.contains('*') || value.contains('?') {
                    continue;
                }
                current = Some(SshHost {
                    name: value.to_string(),
                    hostname: None,
                    user: None,
                    port: None,
                    identity_file: None,
                });
            }
            "hostname" => {
                if let Some(ref mut host) = current {
                    host.hostname = Some(value.to_string());
                }
            }
            "user" => {
                if let Some(ref mut host) = current {
                    host.user = Some(value.to_string());
                }
            }
            "port" => {
                if let Some(ref mut host) = current {
                    host.port = value.parse().ok();
                }
            }
            "identityfile" => {
                if let Some(ref mut host) = current {
                    host.identity_file = Some(value.to_string());
                }
            }
            _ => {}
        }
    }

    // Don't forget the last host
    if let Some(host) = current {
        hosts.push(host);
    }

    hosts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_hosts() {
        let config = r#"
Host dev-server
    HostName 10.0.1.50
    User devuser
    Port 2222
    IdentityFile ~/.ssh/dev_key

Host production
    HostName prod.example.com
    User deploy
"#;
        let hosts = parse_ssh_config_from_str(config);
        assert_eq!(hosts.len(), 2);

        assert_eq!(hosts[0].name, "dev-server");
        assert_eq!(hosts[0].hostname.as_deref(), Some("10.0.1.50"));
        assert_eq!(hosts[0].user.as_deref(), Some("devuser"));
        assert_eq!(hosts[0].port, Some(2222));
        assert_eq!(hosts[0].identity_file.as_deref(), Some("~/.ssh/dev_key"));

        assert_eq!(hosts[1].name, "production");
        assert_eq!(hosts[1].hostname.as_deref(), Some("prod.example.com"));
        assert_eq!(hosts[1].user.as_deref(), Some("deploy"));
        assert_eq!(hosts[1].port, None);
    }

    #[test]
    fn skip_wildcard_hosts() {
        let config = r#"
Host *
    ServerAliveInterval 60

Host dev
    HostName dev.example.com

Host *.internal
    User admin
"#;
        let hosts = parse_ssh_config_from_str(config);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "dev");
    }

    #[test]
    fn to_command_with_all_fields() {
        let host = SshHost {
            name: "myserver".to_string(),
            hostname: Some("192.168.1.100".to_string()),
            user: Some("admin".to_string()),
            port: Some(2222),
            identity_file: Some("~/.ssh/mykey".to_string()),
        };
        assert_eq!(host.to_ssh_command(), "ssh -p 2222 -i ~/.ssh/mykey admin@192.168.1.100");
    }

    #[test]
    fn to_command_minimal() {
        let host = SshHost {
            name: "myserver".to_string(),
            hostname: None,
            user: None,
            port: None,
            identity_file: None,
        };
        assert_eq!(host.to_ssh_command(), "ssh myserver");
    }

    #[test]
    fn to_command_default_port_omitted() {
        let host = SshHost {
            name: "myserver".to_string(),
            hostname: Some("example.com".to_string()),
            user: Some("root".to_string()),
            port: Some(22),
            identity_file: None,
        };
        assert_eq!(host.to_ssh_command(), "ssh root@example.com");
    }
}
