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
