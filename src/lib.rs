//! Test what a terminal program costs while it waits.
//!
//! A program that draws in a terminal should do nothing between two key
//! presses: no timer, no polling, no wake-up. One that does drains the
//! battery of every laptop it runs on, and nobody sees it, because the
//! program still works.
//!
//! `still` runs a program on a terminal of its own (a pty), so a test
//! can press keys, change the size, take the terminal away, and measure
//! what the program does meanwhile.
//!
//! ```no_run
//! use std::time::Duration;
//!
//! let mut app = still::Run::new("target/release/myapp").start().unwrap();
//! app.settle(Duration::from_millis(500));
//!
//! // One second with no key: no CPU time, and it never woke up.
//! let idle = app.idle(Duration::from_secs(1)).unwrap();
//! assert_eq!(idle.wakeups, 0, "{idle:?}");
//!
//! // The terminal goes away: the program must end.
//! app.hang_up();
//! assert!(app.ended(Duration::from_secs(1)));
//! ```
//!
//! Linux only: the numbers come from `/proc`.

use std::fs;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A program to start, and the terminal it gets.
pub struct Run {
    cmd: Command,
    cols: u16,
    rows: u16,
    ignore_hangup: bool,
}

impl Run {
    /// The program to test. It gets `TERM=xterm-256color`, a terminal of
    /// 80 by 24, and it ignores SIGHUP (see [`ignore_hangup`](Self::ignore_hangup)).
    pub fn new(program: impl AsRef<std::ffi::OsStr>) -> Run {
        let mut cmd = Command::new(program);
        cmd.env("TERM", "xterm-256color");
        Run { cmd, cols: 80, rows: 24, ignore_hangup: true }
    }

    pub fn arg(mut self, arg: impl AsRef<std::ffi::OsStr>) -> Run {
        self.cmd.arg(arg);
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Run
    where I: IntoIterator<Item = S>, S: AsRef<std::ffi::OsStr> {
        self.cmd.args(args);
        self
    }

    /// An environment variable for the program. Give a test its own
    /// `HOME`, so it never touches the files of the person running it.
    pub fn env(mut self, key: impl AsRef<std::ffi::OsStr>, value: impl AsRef<std::ffi::OsStr>) -> Run {
        self.cmd.env(key, value);
        self
    }

    /// The size of the terminal, in cells.
    pub fn size(mut self, cols: u16, rows: u16) -> Run {
        (self.cols, self.rows) = (cols, rows);
        self
    }

    /// Whether the program ignores SIGHUP. On by default.
    ///
    /// When a terminal goes away, the kernel sends SIGHUP, and that
    /// alone ends most programs. But a program run under `nohup`,
    /// `strace` or a service manager never gets it, and then only the
    /// dead terminal itself can tell it to stop. That is the case that
    /// goes wrong, so it is the case tested.
    pub fn ignore_hangup(mut self, on: bool) -> Run {
        self.ignore_hangup = on;
        self
    }

    /// Start the program on a new terminal.
    pub fn start(mut self) -> io::Result<App> {
        let (mut master, mut slave) = (0, 0);
        let mut size = libc::winsize { ws_row: self.rows, ws_col: self.cols, ws_xpixel: 0, ws_ypixel: 0 };
        // SAFETY: openpty fills in the two descriptors or fails. The
        // casts are for macOS, which takes these two as `*mut`.
        if unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null_mut::<libc::termios>() as _, &mut size as *mut libc::winsize as _) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: both are fresh descriptors that nothing else owns.
        let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
        // The test's own end must not be inherited by the program, or
        // the terminal would never look gone to it.
        // SAFETY: plain fcntl calls on a descriptor we own.
        unsafe {
            libc::fcntl(master.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC);
            let flags = libc::fcntl(master.as_raw_fd(), libc::F_GETFL);
            libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
        self.cmd.stdin(Stdio::from(slave.try_clone()?));
        self.cmd.stdout(Stdio::from(slave.try_clone()?));
        self.cmd.stderr(Stdio::from(slave));
        let ignore = self.ignore_hangup;
        // SAFETY: between fork and exec only async-signal-safe calls.
        unsafe {
            self.cmd.pre_exec(move || {
                // A session of its own, with the pty as its terminal.
                if libc::setsid() < 0 { return Err(io::Error::last_os_error()); }
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 { return Err(io::Error::last_os_error()); }
                if ignore { libc::signal(libc::SIGHUP, libc::SIG_IGN); }
                Ok(())
            });
        }
        let child = self.cmd.spawn()?;
        Ok(App { child, master: Some(master) })
    }
}

/// What a program did while nobody touched it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Idle {
    /// How long it was watched.
    pub watched: Duration,
    /// CPU time it used, all its threads together. The kernel counts
    /// this in steps of a hundredth of a second.
    pub cpu: Duration,
    /// How many times a thread of it went to sleep and so had been
    /// awake: a timer that fired, a poll that returned. A program that
    /// waits for a key the right way has zero.
    pub wakeups: u64,
    /// What it wrote to the terminal meanwhile, in bytes.
    pub written: usize,
}

impl Idle {
    /// True when the program did nothing at all: no wake-up, no CPU
    /// time, nothing drawn.
    pub fn is_still(&self) -> bool {
        self.wakeups == 0 && self.cpu.is_zero() && self.written == 0
    }
}

/// A running program and the test's end of its terminal.
pub struct App {
    child: Child,
    master: Option<OwnedFd>,
}

impl App {
    /// The program's process id.
    pub fn pid(&self) -> u32 { self.child.id() }

    fn fd(&self) -> Option<RawFd> { self.master.as_ref().map(|m| m.as_raw_fd()) }

    /// Wait up to `wait` for the terminal to have something to read.
    fn readable(&self, wait: Duration) -> bool {
        let Some(fd) = self.fd() else { return false };
        let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        // SAFETY: one valid pollfd.
        unsafe { libc::poll(&mut p, 1, wait.as_millis().min(i32::MAX as u128) as i32) > 0 }
    }

    /// Read what the program has written and is still writing, until
    /// it has been quiet for `quiet`. Returns the bytes, escape
    /// sequences and all.
    ///
    /// A program that never stops writing (a clock, a monitor) would
    /// keep this waiting for ever, so it gives up after three times
    /// `quiet` plus one second.
    pub fn output(&mut self, quiet: Duration) -> Vec<u8> {
        self.read_for(quiet, quiet * 3 + Duration::from_secs(1))
    }

    /// Read until the program has been quiet for `quiet`, and for no
    /// longer than `limit` in all.
    fn read_for(&mut self, quiet: Duration, limit: Duration) -> Vec<u8> {
        let mut out = Vec::new();
        let mut buf = [0u8; 65536];
        let start = Instant::now();
        loop {
            let left = limit.saturating_sub(start.elapsed());
            if !self.readable(quiet.min(left)) { break; }
            let Some(fd) = self.fd() else { break };
            // SAFETY: the buffer is ours and as long as we say.
            let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
            // Zero or an error: the program closed its end.
            if n <= 0 { break; }
            out.extend_from_slice(&buf[..n as usize]);
            if start.elapsed() >= limit { break; }
        }
        out
    }

    /// Let the program start up and draw: read and drop its output
    /// until it has been quiet for `quiet`. Like [`output`](Self::output),
    /// it gives up on a program that never goes quiet.
    pub fn settle(&mut self, quiet: Duration) {
        self.output(quiet);
    }

    /// Type into the terminal: text, or bytes such as `"\x1b"` for Esc
    /// and `"\r"` for Enter.
    pub fn keys(&mut self, keys: impl AsRef<[u8]>) -> io::Result<()> {
        let Some(fd) = self.fd() else { return Err(io::Error::other("the terminal is gone")) };
        let mut rest = keys.as_ref();
        while !rest.is_empty() {
            // SAFETY: the slice is valid for its length.
            let n = unsafe { libc::write(fd, rest.as_ptr().cast(), rest.len()) };
            if n < 0 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::WouldBlock { std::thread::sleep(Duration::from_millis(1)); continue; }
                return Err(e);
            }
            rest = &rest[n as usize..];
        }
        Ok(())
    }

    /// Give the terminal a new size. The program gets SIGWINCH, as it
    /// does when a window is resized.
    pub fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()> {
        let Some(fd) = self.fd() else { return Err(io::Error::other("the terminal is gone")) };
        let size = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
        // SAFETY: a valid winsize for a pty descriptor.
        if unsafe { libc::ioctl(fd, libc::TIOCSWINSZ as _, &size) } < 0 { return Err(io::Error::last_os_error()); }
        Ok(())
    }

    /// Watch the program for `watch` without touching it, and report
    /// what it did. Call [`settle`](Self::settle) first, or the last of
    /// its drawing is counted.
    pub fn idle(&mut self, watch: Duration) -> io::Result<Idle> {
        let pid = self.child.id();
        let before = usage(pid)?;
        let start = Instant::now();
        let mut written = 0;
        // Sleep the whole time unless the program writes.
        loop {
            let left = watch.saturating_sub(start.elapsed());
            if left.is_zero() { break; }
            if self.readable(left) {
                let n = self.read_for(Duration::ZERO, Duration::ZERO).len();
                // Readable with nothing to read: the program is gone.
                if n == 0 { std::thread::sleep(left.min(Duration::from_millis(20))); }
                written += n;
            }
        }
        let watched = start.elapsed();
        let after = usage(pid)?;
        Ok(Idle {
            watched,
            cpu: Duration::from_millis(after.ticks.saturating_sub(before.ticks) * 1000 / ticks_per_second()),
            wakeups: after.switches.saturating_sub(before.switches),
            written,
        })
    }

    /// Take the terminal away, as when its window is closed. After
    /// this, [`keys`](Self::keys) and [`resize`](Self::resize) fail.
    pub fn hang_up(&mut self) {
        self.master = None;
    }

    /// Wait up to `wait` for the program to end. True when it has.
    pub fn ended(&mut self, wait: Duration) -> bool {
        let end = Instant::now() + wait;
        loop {
            if matches!(self.child.try_wait(), Ok(Some(_)) | Err(_)) { return true; }
            if Instant::now() >= end { return false; }
            // Keep its output drained, so a full pipe never holds it up.
            let left = end.saturating_duration_since(Instant::now());
            self.read_for(Duration::from_millis(10).min(left), left);
            if self.master.is_none() { std::thread::sleep(Duration::from_millis(10)); }
        }
    }

    /// End the program now, whatever it is doing.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for App {
    /// A test that fails half way leaves no program behind.
    fn drop(&mut self) {
        self.kill();
    }
}

struct Usage { ticks: u64, switches: u64 }

fn ticks_per_second() -> u64 {
    // SAFETY: sysconf with a valid name.
    (unsafe { libc::sysconf(libc::_SC_CLK_TCK) }).max(1) as u64
}

/// CPU ticks and voluntary context switches of a process, all its
/// threads together.
fn usage(pid: u32) -> io::Result<Usage> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    // The name in brackets may hold spaces; the numbers come after it.
    let rest = stat.rsplit_once(')').map(|(_, r)| r).unwrap_or("");
    let f: Vec<&str> = rest.split_whitespace().collect();
    let num = |i: usize| f.get(i).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    let ticks = num(11) + num(12);
    let mut switches = 0;
    for task in fs::read_dir(format!("/proc/{pid}/task"))?.flatten() {
        let Ok(status) = fs::read_to_string(task.path().join("status")) else { continue };
        for line in status.lines() {
            if let Some(n) = line.strip_prefix("voluntary_ctxt_switches:") {
                switches += n.trim().parse::<u64>().unwrap_or(0);
            }
        }
    }
    Ok(Usage { ticks, switches })
}
