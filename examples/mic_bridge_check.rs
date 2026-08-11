// Exercise the local half of the remote microphone bridge without needing an
// ssh connection: bind the listener, connect to it the way the remote shim
// would, and confirm audio flows and the recorder is reaped afterwards.
//
//   cargo run --example mic_bridge_check
use std::io::Read;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

const SAMPLE_RATE: usize = 16_000;
const BYTES_PER_SAMPLE: usize = 2;
const SECONDS: usize = 2;

fn recorders_running() -> usize {
    let out = std::process::Command::new("pgrep")
        .args(["-fc", "-t raw -q -"])
        .output();
    out.map(|o| {
        String::from_utf8_lossy(&o.stdout)
            .trim()
            .parse()
            .unwrap_or(0)
    })
    .unwrap_or(0)
}

fn main() {
    env_logger::init();
    tuxflow::remote::mic::ensure_listener().expect("listener should bind");

    let sock =
        std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into()) + "/tuxflow/mic.sock";
    println!("connecting to {sock}");
    let mut stream = UnixStream::connect(&sock).expect("shim-side connect should succeed");

    let want = SAMPLE_RATE * BYTES_PER_SAMPLE * SECONDS;
    let mut buf = vec![0u8; 8192];
    let mut got = 0usize;
    let start = Instant::now();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set timeout");
    while got < want {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) => {
                eprintln!("read stopped: {e}");
                break;
            }
        }
        if start.elapsed() > Duration::from_secs(10) {
            break;
        }
    }
    let secs = got as f64 / (SAMPLE_RATE * BYTES_PER_SAMPLE) as f64;
    println!(
        "received {got} bytes ({secs:.2}s of audio) in {:.2?}",
        start.elapsed()
    );

    // Hanging up must stop the recorder, or it keeps the capture device open.
    drop(stream);
    std::thread::sleep(Duration::from_millis(500));
    let leftover = recorders_running();
    println!("recorders still running after hangup: {leftover}");

    assert!(
        got > want / 2,
        "expected roughly {SECONDS}s of audio, got {secs:.2}s"
    );
    assert_eq!(leftover, 0, "recorder leaked after the peer hung up");
    println!("OK");
}
