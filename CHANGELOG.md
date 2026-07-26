# Changelog

## Unreleased

- Add an experimental x86_64 Linux scheduler source backed by four stable
  scheduler tracepoints and an ephemeral Aya-loaded eBPF program.
- Add a separate bounded collector helper that normalizes and aggregates
  scheduler activity before fixed binary pulses cross into the renderer.
- Add stable CPU weather lanes, scheduler-specific rates and loss accounting,
  normalized recording, unprivileged replay, and a sanitized scheduler fixture.
- Add an experimental x86_64 Linux TCP-pathology source for retransmit, sent
  reset, and received reset tracepoints.
- Add a separate bounded TCP collector with 33 ms flow/kind aggregation,
  versioned fixed binary pulses, and four-boundary loss accounting.
- Add stable-flow retransmit lightning, directional reset impacts, compact TCP
  rates, sanitized unprivileged replay, and a contained namespace/netem lab.
- Add a bounded pre-trigger flight recorder for scheduler and TCP semantic
  events with typed automatic/manual triggers and a bounded post-trigger tail.
- Add atomic no-overwrite NDJSON incident publication, explicit loss metadata,
  sanitized source fixtures, and unprivileged replay trigger markers.

## 0.1.0 - 2026-07-26

- Add deterministic demo, line, NDJSON v1, and exact tshark TSV sources.
- Add bounded ingestion with explicit overload accounting.
- Add decaying temporal flow model, normalized recording, and timed replay.
- Add distinct weather and waterfall views in strict ASCII and color ANSI modes.
- Add safe interactive terminal lifecycle, controls, resize handling, and
  deterministic headless snapshots.
