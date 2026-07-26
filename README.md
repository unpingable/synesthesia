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

Synesthesia currently runs from source; it is not published to crates.io and
has no packaged release:

```sh
git clone https://github.com/unpingable/synesthesia.git
cd synesthesia
cargo run --release -- demo
```

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
`c` changes theme, `+`/`-` changes gain, `[`/`]` changes persistence, and
`h`/`?` shows help.

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
- not yet a stable wire protocol beyond the documented v1 behavior

See [the design notes](docs/design.md) for boundaries and bounded-memory
behavior. See [the recording recipe](examples/demo.tape) to make a real GIF
with [VHS](https://github.com/charmbracelet/vhs).

## License and provenance

Licensed under [Apache License 2.0](LICENSE). See [NOTICE](NOTICE) for project
attribution and [PROVENANCE.md](PROVENANCE.md) for the human-directed,
AI-assisted development record. Security reporting guidance is in
[SECURITY.md](SECURITY.md).
