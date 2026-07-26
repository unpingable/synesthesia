# Synesthesia design

Synesthesia deliberately knows less than its inputs. It preserves event shape:
time, magnitude, category, endpoints, and direction. Everything after that is
temporal sculpture.

```text
source reader -> NormalizedEvent -> bounded channel -> TemporalModel
                                                    -> ModelSnapshot
                                                    -> weather | waterfall
                                                    -> RenderFrame
                                                    -> plain text | Ratatui
```

## Boundaries

- `source` owns parsing and normalization. Readers never draw.
- `ingestion` is a 2,048-event non-blocking channel. Full channels drop new
  decorative events and increment an atomic counter.
- `model` advances by elapsed time, not one simulation tick per event. It keeps
  at most 4,096 activity and rate samples and 512 active flows.
- `view` turns an immutable snapshot into cells. Stable endpoint/flow hashes
  create structure without an identity database.
- `render` owns the backend-neutral cell grid and Ratatui widget.
- `terminal` is a guard around raw mode, alternate screen, cursor state, and a
  panic hook.
- `app` connects producers, time, controls, and the backend.

Input records are capped at 64 KiB and oversized records are drained through
the next newline. The model retains hashes rather than source-provided category
strings. Snapshot dimensions are capped at 500x200. Every memory-bearing stream
structure therefore has an explicit ceiling.

## Weather

Origin/flow hashes establish lanes and anchors. Direction chooses velocity.
Magnitude changes brightness, trail length, and whether an event produces a
vertical echo. Age continuously reduces persistence. Repeated endpoint pairs
therefore trace recognizable paths while quiet periods retain sparse,
deterministic atmospheric motion.

## Waterfall

Event age maps to the horizontal time axis. A blend of category and flow hashes
selects rows. Magnitude controls density and heavy-event thickness. It is a
projection of retained history, with no random decoration.

## Time

Interactive sources are timestamped by model arrival time so animation is
stable across source timestamp domains. Recordings preserve optional producer
timestamps. Replay converts consecutive valid timestamp differences to delays;
speed scales those delays and a 50 ms fallback handles missing or regressing
timestamps.

## Extension seam

New sources implement one internal `EventSource` trait and emit
`NormalizedEvent`. There is intentionally no plugin ABI, daemon, database, or
network service.

## Experimental scheduler source implementation choice

The local scheduler-source campaign qualified an x86_64 Ubuntu 6.8 host with
readable kernel BTF, Clang 18 with an eBPF backend, bpftool 7.4, and no kernel
lockdown. Unprivileged BPF is disabled and the BPF/tracefs mounts are
root-readable, so live attachment requires privileges granted externally.

The narrow route is a separate Linux-only
`synesthesia-scheduler-collector` helper using Aya plus one checked-in C eBPF
program compiled by Clang. Aya does not require libbpf or a daemon, while the
host lacks the libbpf development pkg-config package and `bpf-linker`. The C
program attaches only stable scheduler tracepoints and emits a fixed, 48-byte
record through a bounded ring buffer. It does not collect command arguments,
paths, environment data, stack traces, or packet contents.

The helper strictly decodes raw records, normalizes their scheduler meaning,
and aggregates them into 33 ms CPU/category windows before crossing the
process boundary. It sends fixed 64-byte binary pulses to the renderer; the
live hot path performs no JSON serialization. At most 4,096 raw records are
drained per poll and at most 2,048 aggregate buckets exist per window.
Kernel-ring loss, collector aggregation loss, and renderer-channel loss remain
separate counters.

The renderer materializes those already-normalized pulses as the existing
`NormalizedEvent` type. NDJSON v1 remains the durable interoperability,
fixture, recording, and replay representation. Task identities are compact
PID labels and are not retained in an unbounded task table. Migration becomes
paired departure/arrival pulses so both CPU regions react.

Live qualification used Linux `6.8.0-136-generic` on x86_64 with readable
`/sys/kernel/btf/vmlinux`, Clang 18.1.3, bpftool/libbpf 7.4/1.4, Aya 0.13.1,
no kernel lockdown, and explicit root privilege. The qualified kernel names
the shared wakeup BTF context `trace_event_raw_sched_wakeup_template`; CO-RE
relocation resolves that type for both wakeup tracepoints. This is not a claim
of support for every vendor kernel or architecture.

The launcher waits for the helper's first binary pulse before entering raw
terminal mode, so permission, BTF, verifier, tracepoint, and ring-buffer setup
failures leave the shell untouched. Dropping the helper kills and waits for
the child; dropping Aya then closes ephemeral links, programs, maps, and ring
buffers. Nothing is pinned under `/sys/fs/bpf`.

The live visual check covered idle, one pinned CPU worker, six all-core workers,
short burst workers, and settling. The states were appreciably different, but
concurrent Lean and Rust builds were also active, so this is a qualitative
instrument check rather than a controlled scheduler benchmark. The recorded
session reported zero kernel, collector, and renderer-channel losses.

## Experimental TCP-pathology source

The TCP source reuses the scheduler campaign's separate-helper shape without
creating a public probe framework:

```text
three TCP tracepoints -> 1 MiB eBPF ring
                      -> synesthesia-tcp-collector
                      -> 33 ms bounded flow/kind aggregation
                      -> 96-byte SYNT pulse
                      -> bounded renderer channel
                      -> TemporalModel -> weather | waterfall
```

The checked-in C sensor uses the host BTF layouts
`trace_event_raw_tcp_event_sk_skb` for `tcp_retransmit_skb` and
`tcp_send_reset`, and `trace_event_raw_tcp_event_sk` for
`tcp_receive_reset`. The fields used are the tracepoint header's address
family, source/destination addresses, host-order source/destination ports,
socket state where present, plus a monotonic timestamp and current CPU. The
tracepoint classes do not expose a safe byte-length field, so the sensor does
not dereference the skb merely to synthesize retransmitted-byte metrics.

Each raw kernel record is exactly 56 bytes. It contains a version, kind,
family, CPU, ports, socket state, and two fixed 16-byte address slots. It
contains no payload, process data, stack, socket content, or application
metadata. The collector strictly rejects wrong sizes, unsupported versions,
kinds, and families.

The collector coalesces equal `(kind, family, endpoint pair)` keys for 33 ms.
There are at most 1,024 buckets in a window and at most 4,096 raw records are
drained per poll. A new key at the bucket ceiling is deterministically refused
and counted as collector loss; existing keys continue accumulating. The map is
discarded every window, so it cannot become a permanent flow table. The pipe
uses a distinct `SYNT` magic and fixed 96-byte versioned record, preventing
scheduler/TCP protocol confusion.

Kernel ring reservation/read loss, collector bucket refusal, IPC loss, and
renderer-channel drop counters remain separate. The current pipe is blocking
and bounded: an output failure terminates the helper rather than silently
drops a pulse, so its IPC-loss counter remains zero in normal operation.
Renderer overload still uses the existing non-blocking 2,048-event channel.

Normalization maps local retransmits to `tcp.retransmit` and outbound flow,
sent resets to `tcp.reset.send` and outbound impact, and received resets to
`tcp.reset.receive` with peer-to-local inbound direction. Endpoint pairs are
canonicalized only for stable lane identity; origin/target direction remains
truthful. Magnitude is a bounded function of aggregate count because the
selected tracepoint ABI does not provide retransmitted byte length.

The view recognizes those three semantic categories without changing global
gain or decay. Retransmits draw deterministic jagged fractures through the
stable flow region. Sent resets make an outward horizontal `X` cut; received
resets make an inward vertical `!` impact. ANSI substitutes coherent Unicode
strokes, while ASCII remains printable and escape-free. Repetition on one
flow therefore accumulates localized turbulence under the same fixed temporal
physics.

Qualification uses `examples/tcp-pathology-lab.sh`: two temporary network
namespaces, one veth pair, documentation-range IPv4 addresses, bounded
`iperf3` transfers, and loss applied only to the private client veth with
`tc netem`. A closed port generates a contained reset. An exit trap removes
the namespaces and therefore their veths and qdisc; no host route or primary
interface is changed.

Live qualification on Linux `6.8.0-136-generic` x86_64 used explicit root
privilege and all three tracepoints attached. The contained quiet and healthy
phases emitted no lab-flow pathology. The 1% loss phase produced 5,010 raw
retransmits on one stable flow; the 8% phase produced 822 on a second flow
(fewer events under stronger TCP backoff, not evidence of less impairment).
The closed-port phase produced a received-reset impact, and the following
10-second settle emitted no lab-flow pathology. In total, 5,835 raw lab events
became 116 semantic pulses; the largest 33 ms bucket contained 141 events.
The namespace and veth cleanup check was empty and no lab process remained.
