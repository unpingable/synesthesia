# AGENTS.md — Working in this repo

This file is a **travel guide**, not a law.
If anything here conflicts with the user's explicit instructions, the user
wins.

> Instruction files shape behavior; the user determines direction.

---

## Quick start

```bash
cargo run --release -- demo
cargo test --workspace --all-features
```

## Tests

Run the complete local gate before proposing commits:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Never claim tests pass without running them. Keep `Cargo.lock` committed: this
repository ships an application binary.

## Safety and irreversibility

### Do not do these without explicit user confirmation

- Push to a remote or create/close pull requests and issues.
- Delete, amend, squash, rebase, or otherwise rewrite published history.
- Add dependencies or modify the lockfile without explaining why.
- Publish a crate, release, or Git tag.

### Preferred workflow

- Make changes in small, reviewable steps.
- Preserve useful fixtures and the real terminal recording.
- Inspect existing behavior before changing renderer or model semantics.
- Treat terminal lifecycle and bounded memory as correctness properties.

## Repository layout

```text
src/                 Rust source: CLI, sources, model, views, renderers
bpf/                 Minimal checked-in Linux scheduler tracepoint sensor
tests/fixtures/      Exact external-format parser fixtures
examples/            NDJSON samples, replay fixture, VHS recording recipe
docs/design.md       Architecture, bounds, and temporal/rendering model
docs/demo.gif        Real generated terminal recording
```

## Coding conventions

- Stable Rust, edition 2024, with the intentional MSRV in `Cargo.toml`.
- `#![forbid(unsafe_code)]` at crate and binary roots.
- `cargo fmt` and warning-free Clippy are required.
- Prefer deterministic, headless tests over tests that own a real terminal.
- Add dependencies only when they materially reduce risk or complexity.

## Invariants

1. Every memory-bearing stream structure has an explicit bound.
2. Sources produce normalized events; the temporal model, views, and terminal
   renderer remain separate.
3. Terminal raw mode, alternate screen, cursor state, errors, Ctrl-C, and panic
   paths must restore the user's shell.
4. Time advances independently of event count, and deterministic inputs remain
   reproducible.
5. Visual quality is a product requirement, not optional polish.
6. ASCII snapshots contain printable ASCII and newlines only, with no escape
   sequences.
7. Live scheduler capture stays in the separate Linux helper. Normalize and
   aggregate before fixed binary pulses cross into the renderer; NDJSON is for
   interoperability, recording, fixtures, and replay, not the tracepoint hot
   path.
8. eBPF links and maps are ephemeral. Never pin objects, escalate privilege,
   change kernel policy, or invoke `sudo` from Synesthesia.

## What this is not

- Not packet inspection, long-term storage, alerting, or a browser dashboard.
- Not an adapter collection; add sources only with a concrete product need.
- Not a reason to introduce a daemon, database, plugin ABI, or capture
  privileges.
- Not Matrix-style random decoration disconnected from real temporal activity.

Do not turn status output into a dashboard. Do not add generated fake
screenshots or fabricated recordings.

## Status claims

Long-lived docs rot when they carry too many roles at once—design record,
shipped-state log, pickup context, and acceptance record.

If a document claims `shipped`, `built`, `implemented`, or `done`, treat that
as a claim, not evidence. The claim should name its basis—paths, tests,
commits, or a changelog entry—or be treated as historical until rechecked.

Use this repository's vocabulary. Do not import status taxonomies or field
names wholesale from unrelated projects.

## When you're unsure

Ask before changing:

- the normalized event v1 behavior;
- the bounded-memory limits or overload policy;
- terminal lifecycle guarantees;
- the visual identity or scope boundaries.
