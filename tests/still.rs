//! The checks, on programs every Linux machine has.

use std::time::Duration;

const SHORT: Duration = Duration::from_millis(300);

#[test]
fn a_program_that_waits_for_a_key_is_still() {
    // cat blocks in read on the terminal: the right way to wait.
    let mut app = still::Run::new("cat").start().unwrap();
    app.settle(SHORT);
    let idle = app.idle(Duration::from_millis(500)).unwrap();
    assert!(idle.is_still(), "{idle:?}");
}

#[test]
fn a_program_that_polls_is_caught() {
    // A loop that sleeps a hundredth of a second wakes up all the time.
    let mut app = still::Run::new("sh").args(["-c", "while :; do sleep 0.01; done"]).start().unwrap();
    app.settle(SHORT);
    let idle = app.idle(Duration::from_millis(500)).unwrap();
    assert!(idle.wakeups > 5, "{idle:?}");
    assert!(!idle.is_still());
}

#[test]
fn a_program_that_spins_is_caught() {
    let mut app = still::Run::new("sh").args(["-c", "while :; do :; done"]).start().unwrap();
    app.settle(SHORT);
    let idle = app.idle(Duration::from_millis(500)).unwrap();
    assert!(idle.cpu >= Duration::from_millis(300), "{idle:?}");
}

#[test]
fn keys_arrive_and_output_comes_back() {
    let mut app = still::Run::new("cat").start().unwrap();
    app.settle(SHORT);
    app.keys("hello\r").unwrap();
    let out = String::from_utf8_lossy(&app.output(SHORT)).into_owned();
    // The terminal echoes the line, and cat writes it once more.
    assert_eq!(out.matches("hello").count(), 2, "{out:?}");
}

#[test]
fn a_resize_reaches_the_program() {
    let mut app = still::Run::new("sh").args(["-c", "trap 'stty size' WINCH; while :; do read x; done"]).size(80, 24).start().unwrap();
    app.settle(SHORT);
    app.resize(100, 30).unwrap();
    let out = String::from_utf8_lossy(&app.output(SHORT)).into_owned();
    assert!(out.contains("30 100"), "{out:?}");
}

#[test]
fn a_program_that_reads_the_terminal_ends_when_it_is_gone() {
    let mut app = still::Run::new("cat").start().unwrap();
    app.settle(SHORT);
    app.hang_up();
    assert!(app.ended(Duration::from_secs(1)));
}

#[test]
fn a_program_that_never_reads_lives_on() {
    // It ignores SIGHUP and never looks at the terminal, so nothing
    // tells it. The check must say so, and the drop must clean up.
    let mut app = still::Run::new("sleep").arg("30").start().unwrap();
    app.settle(SHORT);
    app.hang_up();
    assert!(!app.ended(SHORT));
    let pid = app.pid();
    drop(app);
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
}

#[test]
fn without_ignore_hangup_the_signal_ends_it() {
    let mut app = still::Run::new("sleep").arg("30").ignore_hangup(false).start().unwrap();
    app.settle(SHORT);
    app.hang_up();
    assert!(app.ended(Duration::from_secs(1)));
}

#[test]
fn a_program_that_never_goes_quiet_does_not_hang_the_test() {
    // Prints ten times a second, so one quiet second never comes.
    let mut app = still::Run::new("sh").args(["-c", "while :; do echo x; sleep 0.1; done"]).start().unwrap();
    let start = std::time::Instant::now();
    app.settle(Duration::from_secs(1));
    assert!(start.elapsed() < Duration::from_secs(5), "{:?}", start.elapsed());
    let idle = app.idle(Duration::from_secs(1)).unwrap();
    assert!(idle.written > 0 && idle.wakeups > 0, "{idle:?}");
}
