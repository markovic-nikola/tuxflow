use std::path::PathBuf;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

fn find_socket(project: Option<&str>) -> Result<PathBuf, String> {
    let base = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());

    // If a project name was given, use it directly
    if let Some(name) = project {
        let sanitized: String = name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        return Ok(PathBuf::from(format!(
            "{}/tuxflow-{}.sock",
            base, sanitized
        )));
    }

    // Auto-discover: scan for tuxflow-*.sock files
    let dir = match std::fs::read_dir(&base) {
        Ok(d) => d,
        Err(e) => return Err(format!("Cannot read {base}: {e}")),
    };

    let sockets: Vec<PathBuf> = dir
        .filter_map(|entry| entry.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("tuxflow-") && n.ends_with(".sock"))
                .unwrap_or(false)
        })
        .collect();

    match sockets.len() {
        0 => Err("No TuxFlow MCP sockets found. Make sure TuxFlow is running.".into()),
        1 => Ok(sockets.into_iter().next().unwrap()),
        _ => {
            // Try to match current working directory to a project
            let cwd = std::env::current_dir().ok();
            if let Some(ref cwd) = cwd {
                for sock in &sockets {
                    let dir_file = format!("{}.dir", sock.display());
                    if let Ok(project_dir) = std::fs::read_to_string(&dir_file) {
                        let project_path = PathBuf::from(project_dir.trim());
                        if cwd.starts_with(&project_path) {
                            return Ok(sock.clone());
                        }
                    }
                }
            }

            // No CWD match — connect to first and warn
            let names: Vec<String> = sockets
                .iter()
                .filter_map(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.strip_prefix("tuxflow-").unwrap_or(n))
                        .map(|n| n.strip_suffix(".sock").unwrap_or(n))
                        .map(String::from)
                })
                .collect();
            eprintln!(
                "Multiple TuxFlow projects found: {}. Connecting to '{}'.\n\
                 Tip: run from within a project directory for auto-detection, \
                 or pass the project name: tuxflow-mcp <project-name>",
                names.join(", "),
                names[0]
            );
            Ok(sockets.into_iter().next().unwrap())
        }
    }
}

#[tokio::main]
async fn main() {
    let project = std::env::args().nth(1);
    let path = match find_socket(project.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    let stream = match UnixStream::connect(&path).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "Failed to connect to TuxFlow MCP socket at {}: {e}",
                path.display()
            );
            eprintln!("Make sure TuxFlow is running.");
            std::process::exit(1);
        }
    };

    let (sock_reader, mut sock_writer) = stream.into_split();

    let stdin_to_sock = tokio::spawn(async move {
        let stdin = BufReader::new(tokio::io::stdin());
        let mut lines = stdin.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if sock_writer.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            if sock_writer.write_all(b"\n").await.is_err() {
                break;
            }
            if sock_writer.flush().await.is_err() {
                break;
            }
        }
    });

    let sock_to_stdout = tokio::spawn(async move {
        let reader = BufReader::new(sock_reader);
        let mut lines = reader.lines();
        let mut stdout = tokio::io::stdout();
        while let Ok(Some(line)) = lines.next_line().await {
            if stdout.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            if stdout.write_all(b"\n").await.is_err() {
                break;
            }
            if stdout.flush().await.is_err() {
                break;
            }
        }
    });

    let _ = tokio::join!(stdin_to_sock, sock_to_stdout);
}
