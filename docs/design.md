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

The narrow integrated route is Aya userspace plus one checked-in C eBPF
program compiled by Clang. Aya does not require libbpf or a daemon, while the
host lacks the libbpf development pkg-config package and `bpf-linker`. The C
program will attach only stable scheduler tracepoints and emit a fixed,
48-byte record through a bounded ring buffer. It will not collect command
arguments, paths, environment data, stack traces, or packet contents.

The userspace boundary decodes that record strictly, maps it into NDJSON v1,
and supplies an internal stable CPU-lane hint to the existing temporal model.
Task identities are compact PID labels and are not retained in an unbounded
task table. Migration deliberately becomes paired departure/arrival sensory
events so both CPU regions react.
