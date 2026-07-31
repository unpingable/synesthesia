# Synesthesia design

Synesthesia deliberately knows less than its inputs. It preserves event shape:
time, magnitude, category, endpoints, and direction. Everything after that is
temporal sculpture.

```text
source reader -> NormalizedEvent -> bounded channel -> TemporalModel
                                                    -> ModelSnapshot
                                                    -> weather | waterfall | meter
                                                    -> RenderFrame
                                                    -> plain text | Ratatui
```

## Boundaries

- `source` owns parsing and normalization. Readers never draw.
- `ingestion` is a 2,048-event non-blocking channel. Full channels drop new
  decorative events and increment an atomic counter.
- `model` advances by elapsed time, not one simulation tick per event. It keeps
  at most 4,096 activity and rate samples, 512 active flows, and 2,048
  short-lived particles.
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

## Meter

The meter is an equalizer projection over six host lanes. The model
deliberately retains category hashes rather than strings, so lane
interpretation lives in a view-side contract table keyed by stable hash —
the same move as the TCP pathology glyph table. Each entry declares label,
gauge-versus-rate aggregation, a full-scale ceiling, and direction. Sources
emit documented categories; the view owns the meaning it is authorized to
read; unknown-category sources fall back to relative hash-bucket bars.

Gauges (CPU, memory) are sample-and-hold: the newest observation stands
verbatim until stale, judged against the observed sample cadence, then
decays. Rates (network, accounted process I/O) divide summed deltas by the
actually covered duration derived from event timestamps, and peak caps
divide each delta by its own inter-arrival gap. The configured sampler
interval is never a time base, so stalls, jitter, and replay speed do not
distort apparent throughput. Every level is a pure function of the immutable
snapshot; peak caps persist exactly as long as the samples that justify
them.

Rate-lane bar heights project through `sqrt(rate/ceiling)` by default so
workstation-scale traffic reads against capacity-scale ceilings while
saturation still means saturation; `--meter-scale linear` is the literal
alternative. Both are fixed pure functions — the same rate always draws the
same height. A pinned bar carries a clip marker meaning the true rate
exceeded the real ceiling in either mode, and the status line names the
projection and any non-neutral gain. Gauges are always linear and take no
gain. Labels claim only what is measured: accounted per-process I/O is
`proc r`/`proc w`, not disk throughput.

## Event-driven particles

Particles are a second immutable-snapshot projection, not a change to field
physics. The model classifies a small visual hint from a normalized category,
then derives origin, velocity, energy, and lifetime from stable event hashes,
direction, magnitude, and model time. Generic heavy events make embers;
retransmits fracture, resets impact, and migrations travel between stable
regions. The classifier remains narrow and sources do not draw cells.

At most 16 particles spawn from one semantic event. At most 2,048 are active,
with deterministic oldest-first eviction, and every lifetime is between 250
ms and two seconds. Updating and rendering therefore remain bounded under a
flood. `--particles off` skips only this projection and leaves activities,
rates, flows, gain, decay, and the base render unchanged. Low-magnitude events
spawn none; particles are never fabricated during a quiet source period.

## Linux proc activity source

The unprivileged process source is a monotonic sampler downstream of a narrow
procfs reader:

```text
/proc/stat + /proc/meminfo + /proc/net/dev + /proc/<pid>/{stat,io}
  -> bounded ProcSample -> bounded ProcTracker
  -> NormalizedEvent -> ordinary renderer channel
```

The first sample seeds a quiet baseline. Later samples compare PID plus kernel
start time, so PID reuse produces one exit and one new birth without a counter
spike. CPU tick deltas become neutral `proc.cpu` events. Read/write byte deltas
become directional `proc.io.read` and `proc.io.write`; counter regression is
treated as reset, not wraparound magnitude. Process birth/exit, changes in
`procs_running`, and coarse low-`MemAvailable` bands are observations rather
than diagnoses.

Host meter lanes ride the same sampler. `host.cpu` and `host.memory` carry a
0..1 ratio each sample; a stalled tick counter emits nothing rather than a
fabricated gauge. `/proc/net/dev` byte counters become directional
`host.net.rx` and `host.net.tx` deltas, clamped once so magnitude and label
agree, with per-line fault isolation so one malformed row cannot discard the
readable interfaces around it. Aggregation covers physical interfaces —
those with a `device` entry under `/sys/class/net`, an existence check only —
excluding loopback, falling back to every non-loopback interface when
nothing is detectably physical, or exactly the repeatable `--net-interface`
selection; explicitly selected names absent from a sample are counted, never
silently narrowed. Host, network, and process counter resets keep separate
diagnostic counters.

A scan holds at most 8,192 process records, tracks at most 4,096 identities,
and emits at most 8,192 semantic events. When tracking is full, already-known
processes remain preferred and new identities are refused into one bounded
background observation. Disappearing and permission-denied procfs entries are
counted separately and do not abort a sample.

The reader never opens `cmdline`, `environ`, `cwd`, `exe`, `fd`, cgroup,
credential, or memory interfaces. Normalized recordings include bounded
`comm`, PID, and start-time identity by default. Session-local anonymization
hashes PID/start-time and omits name/PID labels. It is deliberately not a
process table or persistent identity database.

## Time

Interactive sources are timestamped by model arrival time so animation is
stable across source timestamp domains. Recordings preserve optional producer
timestamps. Replay converts consecutive valid timestamp differences to delays;
speed scales those delays and a 50 ms fallback handles missing or regressing
timestamps.

## Extension seam

Streaming readers implement the internal `EventSource` trait; sampled sources
use an equally narrow bounded batch boundary. Both emit `NormalizedEvent`.
There is intentionally no plugin ABI, daemon, database, or network service.

## Read-only diagnostics

Doctor first captures a bounded `DiagnosticSnapshot` using only read-only
kernel/procfs metadata, conventional terminal environment hints, TTY state,
and sibling collector metadata. A pure report builder then produces typed
`DoctorCheck` values and mode readiness; text and JSON render only that model.
Tests inject synthetic snapshots rather than inspecting the qualification
host.

The live collectors and doctor share one internal prerequisite table for
architecture, helper names, BTF type expectations, and all scheduler/TCP
tracepoints. Passive doctor never calls the attachment code. Only the explicit
`--check-live` path loads the already-built programs, attaches without
generating activity, and immediately drops the links and maps.

Tracepoint visibility, tracepoint presence, live attachment, and event
occurrence remain separate claims. An inaccessible tracefs is reported as
restricted—not as proof that a tracepoint is absent. Likewise root or relevant
capabilities make attachment plausible but never guarantee verifier success.

## Generated command documentation

The Clap `Cli` definition is the only command tree. `completions` passes that
tree to `clap_complete` for Bash, Zsh, or Fish; `manpage` passes it to
`clap_mangen`, then appends small deterministic policy sections covering
privilege, environment, exit status, privacy, project URL, and license.
Neither artifact contains runtime status prose or terminal escapes. Local and
GitHub release packaging invoke the exact built launcher so generated files
match the shipped version.

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

## Bounded incident flight recorder

Flight recording begins after the helper has produced normalized semantic
pulses and before the ordinary nonblocking renderer submission:

```text
fixed helper pulse -> NormalizedEvent -> bounded recorder command channel
                                     \-> bounded renderer channel -> model

recorder worker -> rolling pre-trigger deque -> trigger marker
                -> streaming post-trigger writer -> atomic NDJSON publish
```

This placement keeps JSON and retention out of the kernel and privileged
collector. It also means trigger policy observes exactly the semantic pulses
available to the instrument, including each pulse's bounded aggregate count.
The renderer can drop decorative delivery independently without erasing the
recorder's own distinct accounting.

The explicit states are `Disarmed`, `Armed`, `CapturingTail`, `Complete`,
`Cancelled`, and `Failed`. `arm` is the only transition from `Disarmed`.
`Armed` retains and evicts; one manual or automatic trigger moves to
`CapturingTail`; a monotonic deadline or early interruption publishes
`Complete`. Cancelling while armed writes nothing. Illegal transitions are
errors rather than implicit mode changes.

Pre-trigger retention is bounded three ways: requested monotonic duration,
100,000 events, and 32 MiB of estimated encoded records. The duration is
capped at 30 seconds. Time expiry and capacity eviction both remove the oldest
event deterministically; capacity eviction has its own counter and the actual
oldest-to-trigger duration is stored. Post-trigger duration is capped at 30
seconds and defaults to five.

The recorder owns a separate 4,096-message bounded channel and worker. Live
producer threads use nonblocking sends. Saturation increments writer loss and
does not stall a collector or tracepoint. The worker writes prehistory once at
trigger time, then streams post-trigger events through a fixed 64 KiB
`BufWriter`; it never accumulates the completed incident in memory.

The self-contained format remains valid NDJSON v1. Reserved
`synesthesia.flight.metadata` start/end records describe flight format version
1, source, clocks, trigger, configured and actual durations, event counts,
termination, host kernel/architecture, and each loss boundary.
`synesthesia.flight.trigger` is the one phase marker. Ordinary events receive
only the bounded label `synesthesia.flight.phase=pre|post`. Replay validates
metadata versions, keeps metadata out of weather activity, exposes recorded
losses in status, and renders the trigger as a stable vertical marker.

Automatic triggers are deliberately typed, not an expression language.
TCP retransmit, scheduler event, and scheduler migration rates use fixed
250 ms windows and fire after at least two of the three most recently
completed windows meet the threshold. A sent or received TCP reset can trigger
immediately. Manual triggering is always available while armed, and a session
fires at most once.

No final path exists while armed. Triggering creates
`incident.ndjson.part` with exclusive creation. Completion flushes and syncs
the file, then uses a no-clobber hard link as the atomic publication step and
removes the partial link. Existing final or partial paths are refused. A write
or publication failure never claims success and preserves a partial file when
one exists.

The controlled live results, including exact counts and interpretation limits,
are recorded in
[flight-recorder-qualification.md](flight-recorder-qualification.md).

## Binary distribution

`scripts/build-release.sh` is the one packaging path used locally and by the
tag workflow. It builds all three release binaries with the `ebpf` feature and
`Cargo.lock`, then stages only the launcher, scheduler/TCP helpers, generated
man page and completions, proof GIF, README, release notes, license, and
notice. The archive is x86_64 Linux glibc only and uses one versioned root with
`bin/` and `share/` subtrees.

GNU tar receives sorted names, numeric root ownership, normalized modes, and a
single `SOURCE_DATE_EPOCH`; gzip receives `-n`. SHA-256 is generated beside the
archive. The script refuses a dirty worktree by default and never publishes.
The GitHub workflow runs the same formatter, Clippy, and test gates before
calling this script and creating a release for an already-pushed version tag.

The archive installs nothing. Its helpers remain ordinary sibling executables
with no setuid bit or file capability. Dynamic glibc linking and experimental
kernel requirements are documented limitations rather than hidden installer
behavior.

The local feature, regression, and reproducibility evidence for the first
binary release is recorded in
[v0.2.0-qualification.md](v0.2.0-qualification.md).

The experimental TCP GIF is renderer evidence, not a fabricated screenshot or
a live-capture claim. Its checked-in NDJSON is a selected, timestamp-rebased
excerpt from the controlled live namespace/netem capture, restricted to
documentation-only endpoints. VHS then records the ordinary unprivileged
replay path.
