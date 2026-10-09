# tmux-status

Native CPU, memory, load, battery and network status for **Linux and macOS**.
The TPM integration has **one collector per tmux server**, shared by all sessions
and clients. Rust system collectors handle platform details; our adapter owns
scheduling, cached snapshots, rendering and lifecycle.

## Installation with TPM

```tmux
set -g @plugin 'lidongpeng36/tmux-status'
set -g status-right '#{tmux_status}'
```

For a remote layout (no date/battery, with normalized load):

```tmux
set -g status-right '#{tmux_status_remote} #[fg=colour134]#{E:USER}@#H#[default]'
```

Once TPM checks out the plugin, its first load **automatically downloads a pinned
release binary and verifies SHA-256 before an atomic install**. No Rust toolchain,
compilation, PATH edits, hooks written by the user, or separate service setup are
required. macOS and Linux on x86_64/ARM64 are supported; Linux releases are static
musl binaries. The first download needs curl or wget and HTTPS access to GitHub.
macOS releases require macOS 11 or newer.

Downloads happen in a tmux background job, so the first cold start shows a
placeholder while installation finishes. Later loads use the cached binary in
`bin/` without checking the network. A TPM update changes the pinned version;
old binaries may remain cached for rollback. Download failure shows
`STATUS:unavailable`; the next reload/attach retries. Disconnecting the server
during installation cancels the downloader and removes partial files.

Standard TPM users install the plugin with their normal TPM workflow. Dotfiles
that automatically install declared plugins need no additional action.

## Options

Set these before TPM loads plugins:

| Option | Default | Meaning |
| --- | --- | --- |
| `@tmux-status-interval` | `5` | CPU/memory publication interval, seconds |
| `@tmux-status-battery-interval` | `60` | Battery query/cache interval, seconds |
| `@tmux-status-network` | `on` | Enable background HTTP connectivity checks |
| `@tmux-status-network-interval` | `30` | Successful probe interval, seconds |
| `@tmux-status-network-timeout-ms` | `2000` | Per-target whole-request timeout |
| `@tmux-status-network-direct` | `off` | Bypass environment/macOS system proxies |
| `@tmux-status-check-urls` | Cloudflare, Google | HTTP(S) URLs separated by `\|`, expecting 204 |
| `@tmux-status-bin` | automatic | Absolute binary path for offline/custom installations |

Reload updates the existing collector through a private local control socket;
it preserves the process and CPU baseline. Keep prefix, zoom and key-table
indicators as native tmux formats around `#{tmux_status}`. Colors and Nerd Font
battery glyphs follow the project's compact default palette.

## Network semantics

The default targets, in order, are:

- `https://cp.cloudflare.com/generate_204`
- `https://connectivitycheck.gstatic.com/generate_204`

These are tiny connectivity requests, not pings. Any **204 response** means the
configured HTTP route works; failed targets fall back to the next target.
Redirects are not followed, TLS certificate validation remains enabled, and
unexpected responses are not silently accepted. Requests run off the collection
thread and never hold up CPU/memory output. reqwest respects proxy environment
variables (including SOCKS) and macOS system proxy settings. Linux system desktop
proxy settings are not automatically imported.

| Indicator | JSON state | Meaning |
| --- | --- | --- |
| Green `●` | `reachable` | At least one configured target returned 204 |
| Yellow `●` | `limited` | No target returned 204, but a target returned another HTTP response |
| Red `●` | `unreachable` | All configured targets failed to provide an HTTP response |
| Grey `○` | `unknown` | No result yet, expired result, or client initialization failure |

A limited result can indicate an interception, authentication page, blocked
endpoint or misconfigured URL; it is **not proof of a captive portal**. These
checks do not establish reachability of every website or of a separate direct
route when the configured route uses a proxy.

Failures back off from 2x to at most 10x the base interval. A native interface/IP
address change resets backoff, normally within five seconds. Not every default
route or proxy change changes an interface/address fingerprint. Configure targets
appropriate for your environment instead of treating one public site as universal.

## Lifecycle

TPM starts a tmux-owned `run-shell -b` job. A private directory (0700), protected
lock file and OS file lock elect one owner **per server PID**. A competing start
sends configuration to the owner and exits; sessions, clients and reloads do not
create additional metric collectors. Different tmux servers remain isolated.

The owner publishes `@tmux-status-local`, `@tmux-status-remote`,
`@tmux-status-json` and `@tmux-status-collector-pid`. Status rendering reads native
variables, so clients do not fork collectors. One short tmux CLI invocation per
publication updates shared options; the collector never attaches a control-mode
client that would keep the server alive.

The first snapshot is immediate; a second CPU sample after 250 ms establishes
an interval delta. Pipe-close notification and handled termination signals stop
the owner and clean its control socket. An uncatchable SIGKILL can leave a stale
socket; the next start safely replaces it while holding the lock. Zero-byte lock
files are retained deliberately to avoid unlink/open races, and do not indicate
running processes. Reload or client attach repairs a crashed collector.

## Collectors and metric definitions

| Component | Responsibility | License |
| --- | --- | --- |
| [sysinfo](https://github.com/GuillaumeGomez/sysinfo) 0.39.6 | CPU deltas, memory, load, interface/address snapshots | MIT |
| [starship-battery](https://github.com/starship/rust-battery) 0.12.0 | Native battery discovery, state, energy and units | ISC |
| [reqwest](https://github.com/seanmonstar/reqwest) 0.12 | HTTP, DNS, TLS and proxies | MIT / Apache-2.0 |
| fs2 / signal-hook / nix | Locking, graceful signals, macOS kqueue | MIT / Apache-2.0 |
| clap / serde / serde_json | CLI and optional JSON Lines | MIT / Apache-2.0 |

- CPU is global utilization since the previous sample. The initial snapshot is
  `CPU:--` / JSON `null`, not a misleading zero.
- Linux used memory is total minus `MemAvailable`. macOS used memory includes
  non-purgeable internal pages, wired pages and physical compressor pages;
  reclaimable file cache is excluded. Displayed `G` values are GiB.
- Load is 1/5/15 minute load divided by logical CPUs, distinct from CPU utilization.
- Battery absence hides the segment; a query error shows `BAT:--` and sets
  `battery_error`. Multiple batteries are weighted by full energy; percentages
  are averaged only when comparable energy is unavailable. Mixed states are
  unknown. Discovery repeats each battery interval to handle hotplug.
- `sysinfo` does not report every OS refresh failure and may retain stale values
  after a failed refresh; this adapter does not claim otherwise.

## Standalone use and development

Rust 1.95 or newer:

```sh
cargo build --release --locked
./target/release/tmux-status --once --json
./target/release/tmux-status --network --plain
```

Standalone mode streams output and stays alive until its pipe closes. Unlike TPM
mode, each explicitly launched stream is independent and network checks are off
unless `--network` or `--probe IP:port` is supplied. `--probe` keeps the optional
numeric TCP-only diagnostic mode. `--json`, `--plain`, `--load`, `--no-date` and
`--no-battery` support diagnostic/alternate layouts; see `--help`.

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
python3 tests/smoke.py
python3 tests/installer.py
python3 tests/cold_install_lifecycle.py
python3 tests/publisher_lifecycle.py
python3 tests/tmux_lifecycle.py
```

CI checks Linux and macOS. Release builds additionally test the shipped binary
on all four OS/architecture combinations. Lifecycle tests use separate sockets,
servers and pseudo terminals, never the user's existing tmux server.

MIT licensed. Dependency licenses remain their respective authors' licenses.
