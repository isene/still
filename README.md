# still

Test what a terminal program costs while it waits.

A program that draws in a terminal should do nothing between two key
presses. No timer, no polling, no wake-up. One that wakes up twice a
second drains the battery of every laptop it runs on, and nobody sees
it, because the program still works.

`still` runs a program on a terminal of its own and measures it.

## The command

```
$ still myapp
ok   waits without waking up (0.0 wake-ups a second)
ok   waits without CPU time (0 ms in 2.0 s)
ok   draws nothing while it waits (0 bytes)
ok   draws again after a resize
ok   ends when the terminal is gone
```

It works on any terminal program, in any language. It ends with 0 when
every check passed and 1 when one failed, so it fits in a CI job.

| Option | What it does |
|---|---|
| `-t`, `--time <seconds>` | How long to watch the program wait (2) |
| `-w`, `--wakeups <n>` | Wake-ups a second to allow (0) |
| `-k`, `--key <keys>` | Keys that quit the program, to check that keys arrive (`\e` is Esc) |
| `-s`, `--start <seconds>` | Time the program gets to start (1) |
| `--no-resize` | Skip the resize check |
| `--no-hangup` | Skip the check of a terminal that goes away |

Give the program a home of its own, so the test never touches your
files: `HOME=$(mktemp -d) still myapp`.

## The checks

**It waits without waking up.** A program that waits for a key the
right way sleeps in the kernel until the key comes. `still` counts how
many times a thread of the program went to sleep, and so had been
awake. A timer that fires every two seconds shows as 0.5 a second.

**It ends when the terminal is gone.** When a terminal window closes,
most programs die from the SIGHUP signal. A program under `nohup`,
`strace` or a service manager never gets that signal.

Then only the dead terminal can tell it to stop. A read loop that does
not check for it spins on a full core until someone kills it. `still`
runs the program with SIGHUP ignored, so this case is the one tested.

## The library

```toml
[dev-dependencies]
still = { version = "0.1", package = "fe2o3-still" }
```

```rust
use std::time::Duration;

#[test]
fn it_costs_nothing_while_it_waits() {
    let mut app = still::Run::new(env!("CARGO_BIN_EXE_myapp"))
        .env("HOME", "/tmp/myapp-test")
        .size(120, 40)
        .start()
        .unwrap();
    app.settle(Duration::from_millis(500));

    let idle = app.idle(Duration::from_secs(1)).unwrap();
    assert!(idle.is_still(), "{idle:?}");

    app.keys("j").unwrap();
    assert!(!app.output(Duration::from_millis(200)).is_empty());

    app.hang_up();
    assert!(app.ended(Duration::from_secs(1)));
}
```

| Call | What it does |
|---|---|
| `Run::new(program)` | The program, with `.arg`, `.args`, `.env`, `.size` and `.ignore_hangup` |
| `.start()` | Start it on a new terminal |
| `app.settle(quiet)` | Let it start and draw |
| `app.idle(watch)` | Watch it: CPU time, wake-ups, bytes written |
| `app.keys(keys)` | Type into the terminal |
| `app.output(quiet)` | What it wrote, escape sequences and all |
| `app.resize(cols, rows)` | A new terminal size |
| `app.hang_up()` | Take the terminal away |
| `app.ended(wait)` | True when the program has ended |

A test that fails half way leaves no program behind.

## Install

```
cargo install fe2o3-still
```

Linux only: the numbers come from `/proc`. One dependency, `libc`.

## Why

A bug in a terminal library kept a full core busy in every program
built on it, once its terminal was gone. The programs worked, so nobody
saw it for months. Twenty lines of test found it. This is that test,
for everyone.

Part of [Fe₂O₃](https://isene.org/fe2o3/). Public domain.
