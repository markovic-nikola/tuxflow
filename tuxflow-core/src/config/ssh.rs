use std::fs;
use std::path::PathBuf;

use crate::config::schema::{ProcessCategory, ProcessConfig};

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

        if let Some(port) = self.port
            && port != 22
        {
            parts.push("-p".to_string());
            parts.push(port.to_string());
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

/// The Add SSH Connection form as typed: one string per field, so a shell
/// can bind text boxes to it directly. `port` stays text because the field
/// is editable — a parse failure falls back to 22 on submit, GTK's rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshConnectionFields {
    pub name: String,
    pub host: String,
    pub user: String,
    pub port: String,
    pub identity_file: String,
}

impl Default for SshConnectionFields {
    /// The "Custom..." pick: everything empty but the port.
    fn default() -> Self {
        Self {
            name: String::new(),
            host: String::new(),
            user: String::new(),
            port: String::from("22"),
            identity_file: String::new(),
        }
    }
}

impl SshConnectionFields {
    /// What picking a `~/.ssh/config` alias fills in. The host is the
    /// RESOLVED hostname, so the command works with the alias' resolved
    /// address even outside this machine's config (the add-project flow
    /// keeps the alias instead, since it needs ProxyJump to keep working).
    pub fn from_host(host: &SshHost) -> Self {
        Self {
            name: host.name.clone(),
            host: host.hostname.clone().unwrap_or_else(|| host.name.clone()),
            user: host.user.clone().unwrap_or_default(),
            port: host.port.unwrap_or(22).to_string(),
            identity_file: host.identity_file.clone().unwrap_or_default(),
        }
    }

    pub fn host_given(&self) -> bool {
        !self.host.trim().is_empty()
    }

    /// The process this form describes under `name`, or `None` while the
    /// host is empty (the one required field). The sidebar shows
    /// `display_name` — the typed name, else `user@host`, else the host —
    /// while `name` stays the identifier the saved file is keyed on.
    pub fn to_process_config(
        &self,
        name: String,
        auto_connect: bool,
        auto_reconnect: bool,
    ) -> Option<ProcessConfig> {
        let host = self.host.trim();
        if host.is_empty() {
            return None;
        }
        let user = self.user.trim();
        let identity = self.identity_file.trim();
        let port: u16 = self.port.trim().parse().unwrap_or(22);
        let ssh_host = SshHost {
            name: host.to_string(),
            hostname: Some(host.to_string()),
            user: (!user.is_empty()).then(|| user.to_string()),
            port: (port != 22).then_some(port),
            identity_file: (!identity.is_empty()).then(|| identity.to_string()),
        };
        let typed = self.name.trim();
        let display = if !typed.is_empty() {
            typed.to_string()
        } else if user.is_empty() {
            host.to_string()
        } else {
            format!("{user}@{host}")
        };
        Some(ProcessConfig {
            name,
            command: ssh_host.to_ssh_command(),
            working_dir: None,
            start_with_project: auto_connect,
            auto_restart: auto_reconnect,
            open_in_browser: false,
            restart_when_changed: Vec::new(),
            env: std::collections::BTreeMap::new(),
            category: ProcessCategory::SSH,
            auto_named: false,
            display_name: Some(display),
        })
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
        assert_eq!(
            host.to_ssh_command(),
            "ssh -p 2222 -i ~/.ssh/mykey admin@192.168.1.100"
        );
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
    #[test]
    fn fields_from_host_and_blank() {
        let host = SshHost {
            name: "dev".into(),
            hostname: Some("10.0.1.50".into()),
            user: Some("devuser".into()),
            port: Some(2222),
            identity_file: None,
        };
        let f = SshConnectionFields::from_host(&host);
        assert_eq!(f.name, "dev");
        assert_eq!(f.host, "10.0.1.50");
        assert_eq!(f.user, "devuser");
        assert_eq!(f.port, "2222");
        assert_eq!(f.identity_file, "");
        // No HostName: the alias itself is the host.
        let bare = SshHost {
            name: "box".into(),
            hostname: None,
            user: None,
            port: None,
            identity_file: None,
        };
        let f = SshConnectionFields::from_host(&bare);
        assert_eq!(f.host, "box");
        assert_eq!(f.port, "22");
        assert_eq!(SshConnectionFields::default().port, "22");
        assert!(!SshConnectionFields::default().host_given());
    }

    #[test]
    fn process_config_from_fields() {
        let mut f = SshConnectionFields::default();
        assert!(f.to_process_config("ssh".into(), false, false).is_none());
        f.host = " example.com ".into();
        f.user = "me".into();
        f.port = "2222".into();
        f.identity_file = "~/.ssh/k".into();
        let c = f.to_process_config("ssh".into(), true, false).unwrap();
        assert_eq!(c.command, "ssh -p 2222 -i ~/.ssh/k me@example.com");
        assert_eq!(c.display_name.as_deref(), Some("me@example.com"));
        assert_eq!(c.name, "ssh");
        assert!(c.start_with_project && !c.auto_restart);
        assert_eq!(c.category, ProcessCategory::SSH);
        // A typed name wins the label; a bad port falls back to 22; no user
        // means the bare host is the label.
        f.name = "prod".into();
        f.port = "abc".into();
        f.user.clear();
        let c = f.to_process_config("ssh-2".into(), false, true).unwrap();
        assert_eq!(c.command, "ssh -i ~/.ssh/k example.com");
        assert_eq!(c.display_name.as_deref(), Some("prod"));
        assert!(c.auto_restart);
        f.name.clear();
        let c = f.to_process_config("ssh-3".into(), false, false).unwrap();
        assert_eq!(c.display_name.as_deref(), Some("example.com"));
    }
}
