# tmux-status

A small Rust adapter that streams CPU, memory, optional load averages, battery,
and optional TCP reachability to tmux on **Linux and macOS**. It reuses
maintained collectors rather than implementing platform metrics itself:

| Component | Responsibility | License |
| --- | --- | --- |
| [sysinfo](https://github.com/GuillaumeGomez/sysinfo) 0.39.6 | CPU deltas, memory, load | MIT |
| [starship-battery](https://github.com/starship/rust-battery) 0.12.0 | Native battery discovery, state, energy and units | ISC |
| clap / serde / serde_json | CLI and optional JSON Lines | MIT / Apache-2.0 |
| nix (macOS only) | Owned kqueue and pipe-close notification | MIT |

Our code owns only scheduling, aggregation, rendering, and pipe lifecycle.
No shell utilities, tmux queries, daemon service, disk caches or network traffic
are used by the default collector. `libc` supplies a local clock and pipe polling.

## Build and use

Rust 1.95 or newer:

```sh
cargo build --release --locked
install -m 755 target/release/tmux-status ~/.local/bin/tmux-status
```

The destination directory must exist. No install-time downloads or automatic
compilation happen when tmux loads its configuration.

```tmux
set -g status-interval 5
set -g status-right '#($HOME/.local/bin/tmux-status --interval 5)'
```

The command intentionally stays alive and flushes one line every five seconds.
Tmux displays the latest complete line from `#()`, allowing CPU deltas and the
battery snapshot to stay in memory. It is a tmux-owned format job, not a global
daemon: tmux may create separate jobs for different clients/format contexts.
Closing the output pipe stops the collector promptly even during a long interval.
Changing the command causes tmux to use a different format job; unchanged commands
can reuse an existing job. No hooks are installed and no options are overwritten.

Keep prefix/zoom/key-table indicators outside this command as native tmux formats.
Existing continuum/resurrect plugins are independent and continue to work.

For a remote machine, hide the date and battery and add normalized load:

```tmux
set -g status-right '#($HOME/.local/bin/tmux-status --no-date --no-battery --load)'
```

Battery icons and the separator require a Nerd Font. Use `--plain` for ASCII
output, `--json` for JSON Lines, or `--once --json` for a diagnostic snapshot.
Run `tmux-status --help` for intervals and other options.

## Metric meanings

- **CPU:** global utilization across logical CPUs since the previous sample.
  First output is `CPU:--` / JSON `null`, because no interval exists yet.
- **Memory:** `sysinfo`'s used/total bytes. On Linux, used is total minus
  `MemAvailable`. On macOS, used includes non-purgeable internal pages, wired
  pages and physical compressor pages; reclaimable file cache is excluded.
  This differs from older plugins that classify compressor pages as free.
  Displayed `G` values are GiB (1024³ bytes).
- **Load:** 1/5/15 minute load divided by the number of logical CPUs; load is
  not the same quantity as CPU percentage.
- **Battery:** queried every 60 seconds, including caching absence/errors.
  No battery hides the segment; errors show `BAT:--` and `battery_error: true`.
  Multiple batteries are weighted by reported full energy. If any pack lacks
  valid energy, their percentages are averaged; mixed states are `unknown`.
  Discovery is repeated each battery interval to handle hotplug.
- **Unknown values:** missing/invalid values are not presented as zero.
  `sysinfo` does not expose every OS refresh failure; its retained values may
  remain stale after a failed refresh. This adapter does not claim otherwise.

## Optional TCP probe

Network probing is **off by default**. Opt in with a numeric address:

```sh
tmux-status --probe 127.0.0.1:8080 --probe-interval 30 --probe-timeout-ms 250
```

This tests TCP connection reachability to that one endpoint, not general
Internet connectivity, HTTP health, TLS validity or proxy reachability. Hostnames
are rejected to avoid blocking DNS outside the connection timeout. IPv6 uses
`[address]:port`. There is no built-in public target.

Probes run on a separate thread so slow connections do not delay status output.
Success is green, failure red, unknown/expired yellow. Plain text shows `TCP:up`,
`TCP:down` or `TCP:--`. Failures back off from 2x to at most 10x the base interval.
`--once` performs a synchronous probe within the specified timeout.

## Validation

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
python3 tests/smoke.py
python3 tests/tmux_lifecycle.py  # requires tmux, uses an isolated socket
```

CI runs these checks on Linux and macOS with the minimum supported Rust version.
Tests cover aggregation, unknown values, loopback probe outcomes, backoff, live
CPU/memory snapshots, stream output, input validation and pipe-close termination.

MIT licensed. Dependency licenses remain their respective authors' licenses.
