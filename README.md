# Synesthesia

**Data in. Terminal weather out.**

Synesthesia turns live machine activity into a terminal-native visual
instrument.

![A real Synesthesia demo recording](docs/demo.gif)

```sh
cargo run --release -- demo
```

```text
          .:--=+*       signal drifts through stable lanes
    ..:-==       #      heavy events bruise the field
  .:                  repeated flows learn their own weather
```

No setup is the point. Any line can become an event:

```sh
nc -lk 9000 | cargo run --release -- stdin --format lines
```

## Download

The v0.2.0 release is one x86_64 Linux glibc archive:

```sh
sha256sum -c synesthesia-v0.2.0-x86_64-unknown-linux-gnu.tar.gz.sha256
tar -xzf synesthesia-v0.2.0-x86_64-unknown-linux-gnu.tar.gz
./synesthesia demo
```

Download the archive and its SHA-256 checksum from the
[v0.2.0 release](https://github.com/unpingable/synesthesia/releases/tag/v0.2.0).
The archive also contains the two experimental eBPF collector helpers beside
the launcher. It does not install them, grant capabilities, or set setuid
bits.

To build from source instead:

```sh
git clone https://github.com/unpingable/synesthesia.git
cd synesthesia
cargo run --release -- demo
```

Synesthesia is not published to crates.io.

## Diagnose an installation

```sh
./synesthesia doctor
./synesthesia doctor --format json | jq .
```

Doctor passively reports platform, terminal, `/proc`, packaged-collector,
kernel-BTF, tracepoint-visibility, and privilege readiness. It distinguishes
missing support, restricted visibility, external privilege, and conditions
that were simply not tested. A normal run never attaches probes, invokes
`sudo`, generates traffic, changes mounts or sysctls, or writes a file.

`--check-ebpf` makes passive eBPF prerequisites an explicitly requested check.
`--check-live` is different: it actively attempts to attach and immediately
detach the existing scheduler and TCP programs using only the privilege the
caller already has. It generates no workload and creates no pins. Synesthesia
never escalates itself.

Text is the default; JSON follows the documented
[doctor schema v1](docs/doctor-schema-v1.md). Exit status is `0` for a
completed generally usable report, `1` when a requested check fails, and `2`
when doctor itself cannot produce valid output. Default output is designed for
public bug reports and contains no hostname, username, home path, endpoint,
process identity, arguments, environment contents, or captured activity.

## Shell completions and manual page

Generate Bash, Zsh, or Fish completions directly from the installed command
tree:

```sh
synesthesia completions bash > ~/.local/share/bash-completion/completions/synesthesia
synesthesia completions zsh > ~/.local/share/zsh/site-functions/_synesthesia
synesthesia completions fish > ~/.config/fish/completions/synesthesia.fish
```

Those are conventional user-local examples, not directories Synesthesia
creates or modifies. Release archives include the same generated files under
`share/`.

Generate the current roff manual from the same command model:

```sh
synesthesia manpage > synesthesia.1
man ./synesthesia.1
```

## Linux process weather without root

```sh
./synesthesia proc
```

Or from source:

```sh
cargo run --release -- proc
```

Process mode samples Linux `/proc` every 250 ms. It turns per-process CPU
deltas, readable storage-I/O deltas, process birth/exit, runqueue changes, and
low-available-memory observations into ordinary normalized events. Stable
process identities develop stable regions; CPU load sustains motion, reads and
writes move in opposite directions, and process churn arrives as a brief
front. No root, BTF, eBPF, tshark, daemon, or service is required.

The source reads `/proc/stat`, `/proc/meminfo`, `/proc/<pid>/stat`, and readable
`/proc/<pid>/io`. It does not read command-line arguments, environments,
working directories, executable paths, open files, cgroup paths, usernames,
or process memory. Recordings contain bounded process names and PIDs by
default; use `--anonymize` for stable session-local opaque identities:

```sh
./synesthesia proc --anonymize --record proc-session.ndjson
./synesthesia replay proc-session.ndjson
```

Sampling accepts `--interval 50ms` through `--interval 5s` and an optional
`--pid PID`. A scan considers at most 8,192 candidates, tracks at most 4,096
processes, and emits at most 8,192 semantic events per sample. The first sample
is a quiet baseline rather than a fabricated process-start storm.

The network hook uses one exact field order:

```sh
sudo tshark -l -n -T fields \
  -e frame.time_epoch \
  -e ip.src -e ipv6.src -e ip.dst -e ipv6.dst \
  -e _ws.col.Protocol -e frame.len \
  -e tcp.srcport -e udp.srcport -e tcp.dstport -e udp.dstport \
  -E header=n -E separator=/t -E occurrence=f -E quote=n \
  | cargo run --release -- stdin --format tshark-tsv
```

That command emits exactly 11 tab-separated columns. The
[checked-in fixture](tests/fixtures/tshark-fields.tsv) exercises IPv4, IPv6,
TCP, UDP, missing fields, direction inference, and heavy frames. `tshark` was
not installed on the qualification machine, so the command is fixture-verified,
not live-capture verified. Synesthesia never needs capture privileges; the
producer owns those.

Wireshark is analysis. This is the hallucination layer.

## Experimental Linux scheduler source

Build both the renderer and its Linux-only collector helper:

```sh
cargo build --release --features ebpf --bins
sudo ./target/release/synesthesia ebpf scheduler
```

Then disturb the weather:

```sh
stress-ng --cpu "$(nproc)" --timeout 20s
```

This source is experimental, Linux-only, and currently qualified on x86_64
Linux 6.8 with kernel BTF, Clang 18, bpftool 7.4, Aya 0.13.1, and explicit
`sudo`. Synesthesia never invokes `sudo`, changes sysctls, mounts filesystems,
or pins BPF objects. The one-command form above knowingly runs the launcher
and helper with the credentials supplied by the user.

The helper attaches to `sched_switch`, `sched_wakeup`,
`sched_wakeup_new`, and `sched_migrate_task`. It captures compact scheduler
identities and CPU movement—not command lines, arguments, paths, environment,
stacks, or payloads. Raw records are normalized and aggregated into 33 ms
CPU/category windows before a fixed binary stream crosses into the renderer;
JSON is not on the live tracepoint path.

Record the normalized result and replay it later without eBPF or privilege:

```sh
sudo ./target/release/synesthesia ebpf scheduler --record scheduler.ndjson
./target/release/synesthesia replay scheduler.ndjson
./target/release/synesthesia replay examples/scheduler.ndjson
```

The collector requires readable kernel BTF, the four tracepoints, a kernel
with ring-buffer support, and sufficient BPF/perf-event privilege. It currently
supports x86_64 only. A successful tracepoint event is evidence that the event
occurred; it is not proof of complete scheduler causality, task runtime, CPU
utilization, or why the scheduler made a decision.

## Experimental Linux TCP-pathology source

Retransmits are lightning. Resets are impact events.

![Real rendering of a sanitized TCP kernel recording](docs/tcp-kernel-weather.gif)

This is a real Synesthesia rendering of a sanitized replay—not a live GIF.
The source events were captured from the existing eBPF TCP collector during
the controlled namespace/netem lab, then restricted to documentation-only
`192.0.2.0/24` flows. Relative timing, category, magnitude, and direction are
preserved; unrelated host traffic was excluded. The replay shows retransmit
buildup, reset impacts, a quiet gap, a second affected flow, and settle.

The exact input and VHS recipe are
[tcp-kernel-weather.ndjson](examples/tcp-kernel-weather.ndjson) and
[tcp-kernel-weather.tape](examples/tcp-kernel-weather.tape).

```sh
cargo build --release --features ebpf --bins
sudo ./target/release/synesthesia ebpf tcp
```

The Linux-only helper attaches to `tcp_retransmit_skb`, `tcp_send_reset`, and
`tcp_receive_reset`. It reads endpoint identity, address family, ports, socket
state, CPU, and event time from those tracepoints. It never reads packet
payloads, socket contents, command lines, TLS metadata, or application
protocols. Synesthesia does not invoke `sudo`; privilege must be supplied
explicitly by the user.

A retransmit means the local TCP stack retransmitted data. It does not prove
packet loss, congestion, a remote fault, or why retransmission was necessary.
A reset tracepoint proves that the local stack sent or received a reset; it
does not assign blame to an application or peer.

Raw events are aggregated into bounded 33 ms flow/kind windows inside the
collector, then cross into the renderer as a fixed binary pulse stream. NDJSON
is reserved for recording, replay, fixtures, and interoperability:

```sh
sudo ./target/release/synesthesia ebpf tcp --record tcp-session.ndjson
./target/release/synesthesia replay tcp-session.ndjson
./target/release/synesthesia replay tests/fixtures/tcp-pathology.ndjson --speed 0.25
```

To generate controlled local retransmits and resets without touching the
host's primary interfaces or routes, run the contained namespace lab in a
second terminal:

```sh
sudo ./examples/tcp-pathology-lab.sh
```

The lab uses two temporary namespaces, a veth pair, documentation-only
addresses, bounded `iperf3` transfers, and `tc netem`; its exit trap removes
the namespaces, veths, qdisc, and child process. Current limitations are
x86_64, readable kernel BTF, ring-buffer support, all three named tracepoints,
and externally granted BPF/perf-event privilege. Live qualification used Linux
6.8 on x86_64, Clang 18, bpftool 7.4, Aya 0.13.1, and explicit `sudo`.

## Experimental flight recorder

Watch the machine fail from ten seconds before it knew it was failing.

```sh
sudo ./target/release/synesthesia ebpf tcp \
  --flight-recorder incident.ndjson \
  --pre-trigger 10s \
  --post-trigger 5s

./target/release/synesthesia replay incident.ndjson --speed 0.2
```

The same options compose with `ebpf scheduler`. While armed, Synesthesia keeps
only a rolling window of normalized semantic events—not raw tracepoints—and
does not create the output file. An automatic source-specific trigger freezes
that bounded history, records one trigger marker, streams a bounded tail, and
publishes one self-contained NDJSON incident. Press `t` to trigger manually or
`x` to cancel before a trigger.

The TCP default fires when retransmit semantic-event rate reaches 100/s in at
least two of three 250 ms windows. The scheduler default uses 15,000 scheduler
semantic events/s with the same debounce. Explicit triggers are:

```text
manual
tcp-retransmit-rate=100
tcp-reset
scheduler-event-rate=15000
scheduler-migration-rate=1000
```

These thresholds describe observations, not diagnoses. A retransmit-rate
trigger does not claim congestion; a scheduler-event-rate trigger does not
claim CPU saturation. One incident is captured per run. Existing output and
`.part` paths are refused, history is capped at 100,000 events and 32 MiB, and
kernel, collector, IPC, renderer, history-eviction, malformed-input, and
recording-writer losses remain separate in metadata. No packet payload or
process argument is added.

`q`, Escape, or Ctrl-C cancels cleanly while armed. After a trigger, the same
actions publish the valid tail captured so far with an `interrupted`
termination. A normal tail is flushed, synchronized, and atomically linked
into place without overwriting an existing file; failures preserve the
`.part` file for inspection. Replay is unprivileged and identifies `PRE`,
`TRIGGER`, and `POST` with a stable trigger marker.

The [local qualification record](docs/flight-recorder-qualification.md)
includes the exact controlled commands, retained event counts, loss counters,
cleanup checks, and the limits of the workload interpretation.

## Inputs, recordings, and replay

```sh
cargo run --release -- stdin --format ndjson
cargo run --release -- stdin --format ndjson --record session.ndjson
cargo run --release -- replay session.ndjson --speed 2
cargo run --release -- schema
```

NDJSON v1 requires `v`, `category`, and `magnitude`. Timestamp, endpoints,
direction, and string labels are optional:

```json
{"v":1,"timestamp":1720000000.125,"category":"tcp","origin":"10.0.0.4:54321","target":"10.0.0.8:443","magnitude":1514,"direction":"outbound","labels":{"protocol":"TLS"}}
```

Unsupported versions and malformed records are counted and skipped. Individual
records are capped at 64 KiB. Replay preserves timestamp deltas, scaled by
`--speed`; untimed or discontinuous records use 50 ms.

## Views and controls

`weather` is a moving two-dimensional flow field. `waterfall` rolls real event
history through category and stable-flow bands. Both accept `--mode ascii` or
`--mode ansi` and one of `phosphor`, `amber`, `cold`, or `monochrome` via
`--theme`.

`q`/Escape quits, Space pauses, `1`/`2` selects a view, `a` toggles rendering,
`c` changes theme, `p` toggles event-driven particles, `+`/`-` changes gain,
`[`/`]` changes persistence, and `h`/`?` shows help. `--particles off`
preserves the field without the ember layer. While a flight recorder is armed,
`t` triggers manually and `x` cancels without writing.

For plain, deterministic output:

```sh
cargo run --release -- demo --seed 42 --snapshot --width 100 --height 30 --mode ascii
printf 'alpha\nbeta\ngamma\n' |
  cargo run --release -- stdin --format lines --snapshot --width 80 --height 24 --mode ascii
```

Snapshots never enter raw mode or emit terminal control sequences. ASCII
snapshots contain printable ASCII and newlines only.

## What it is not

- not packet inspection
- not long-term storage
- not alerting
- not a browser dashboard
- not a replacement for Wireshark
- not a scheduler profiler or complete causality model
- not a TCP connection tracker, packet capture, or diagnosis of retransmit cause
- not a process table, profiler, argument collector, or persistent process history
- not an incident manager, retention service, rules engine, or alerting system
- not yet a stable wire protocol beyond the documented v1 behavior

See [the design notes](docs/design.md) for boundaries and bounded-memory
behavior. See [the recording recipe](examples/demo.tape) to make a real GIF
with [VHS](https://github.com/charmbracelet/vhs).

## License and provenance

Licensed under [Apache License 2.0](LICENSE). See [NOTICE](NOTICE) for project
attribution and [PROVENANCE.md](PROVENANCE.md) for the human-directed,
AI-assisted development record. Security reporting guidance is in
[SECURITY.md](SECURITY.md).
