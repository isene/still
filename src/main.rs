//! `still <program> [args...]`: what a terminal program costs while it
//! waits, and whether it ends when its terminal goes away.

use std::process::ExitCode;
use std::time::Duration;

const HELP: &str = "still: what a terminal program costs while it waits

    still [options] <program> [args...]

Runs the program on a terminal of its own and checks that it
- uses no CPU and never wakes up while it waits for a key,
- draws again when the terminal changes size,
- ends when the terminal is taken away.

Options:
    -t, --time <seconds>   how long to watch it wait (default 2)
    -w, --wakeups <n>      wake-ups a second to allow (default 0)
    -k, --key <keys>       keys that quit the program, to check that
                           keys arrive (\\e is Esc, \\r is Enter)
    -s, --start <seconds>  time the program gets to start (default 1)
        --no-resize        skip the resize check
        --no-hangup        skip the check of a terminal that goes away
    -h, --help             this text
    -V, --version          the version

Ends with 0 when every check passed, 1 when one failed.";

fn unescape(s: &str) -> Vec<u8> {
    s.replace("\\e", "\x1b").replace("\\r", "\r").replace("\\t", "\t").into_bytes()
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (mut time, mut start, mut allow) = (2.0f64, 1.0f64, 0u64);
    let (mut key, mut resize, mut hangup) = (None, true, true);
    let mut program = None;
    while let Some(a) = args.next() {
        let mut value = |name: &str| args.next().unwrap_or_else(|| { eprintln!("still: {name} needs a value"); std::process::exit(2) });
        match a.as_str() {
            "-h" | "--help" => { println!("{HELP}"); return ExitCode::SUCCESS; }
            "-V" | "--version" => { println!("still {}", env!("CARGO_PKG_VERSION")); return ExitCode::SUCCESS; }
            "-t" | "--time" => time = value("--time").parse().unwrap_or(2.0),
            "-s" | "--start" => start = value("--start").parse().unwrap_or(1.0),
            "-w" | "--wakeups" => allow = value("--wakeups").parse().unwrap_or(0),
            "-k" | "--key" => key = Some(unescape(&value("--key"))),
            "--no-resize" => resize = false,
            "--no-hangup" => hangup = false,
            _ => { program = Some(a); break; }
        }
    }
    let Some(program) = program else { eprintln!("{HELP}"); return ExitCode::from(2) };
    let rest: Vec<String> = args.collect();

    let mut app = match still::Run::new(&program).args(&rest).size(120, 40).start() {
        Ok(app) => app,
        Err(e) => { eprintln!("still: cannot start {program}: {e}"); return ExitCode::from(2); }
    };
    let mut failed = false;
    let mut check = |what: &str, ok: bool, note: String| {
        println!("{} {what}{}", if ok { "ok  " } else { "FAIL" }, if note.is_empty() { String::new() } else { format!(" ({note})") });
        failed |= !ok;
    };

    app.settle(Duration::from_secs_f64(start));
    if app.ended(Duration::ZERO) {
        eprintln!("still: {program} ended before it could be watched");
        return ExitCode::from(2);
    }

    match app.idle(Duration::from_secs_f64(time)) {
        Ok(idle) => {
            let secs = idle.watched.as_secs_f64().max(0.001);
            let per_sec = idle.wakeups as f64 / secs;
            check("waits without waking up", per_sec <= allow as f64, format!("{:.1} wake-ups a second", per_sec));
            check("waits without CPU time", idle.cpu.as_millis() as f64 <= 10.0 * secs.ceil() && (allow > 0 || idle.cpu.is_zero()),
                format!("{} ms in {:.1} s", idle.cpu.as_millis(), secs));
            check("draws nothing while it waits", idle.written == 0 || allow > 0, format!("{} bytes", idle.written));
        }
        Err(e) => check("could be watched", false, e.to_string()),
    }

    if resize {
        let ok = app.resize(100, 30).is_ok() && !app.output(Duration::from_millis(500)).is_empty();
        check("draws again after a resize", ok, String::new());
    }

    if let Some(key) = key {
        let _ = app.keys(&key);
        check("ends on its quit key", app.ended(Duration::from_secs(1)), String::new());
    } else if hangup {
        app.hang_up();
        let ended = app.ended(Duration::from_secs(1));
        check("ends when the terminal is gone", ended, String::new());
        if !ended {
            // Say what it does instead: sleeping for ever, or spinning.
            if let Ok(idle) = app.idle(Duration::from_secs(1)) {
                println!("     it lives on and uses {} ms of CPU a second", idle.cpu.as_millis());
            }
        }
    }
    app.kill();
    if failed { ExitCode::from(1) } else { ExitCode::SUCCESS }
}
