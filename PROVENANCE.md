# Provenance

This project is human-directed and AI-assisted. Final design authority,
acceptance criteria, and editorial control rest with the human author.
AI contributions were material and are categorized below by function.

## Human authorship

The author defined the project direction, requirements, product identity,
constraints, and acceptance standard. AI systems contributed implementation,
tests, documentation, critique, and validation under human direction and
review; they did not independently determine project goals or publication
decisions.

## AI-assisted collaboration

### Initial implementation campaign

Lead collaboration: OpenAI Codex.

Synesthesia was developed as a greenfield project in
`/home/jbeck/git/synesthesia`. The initial campaign produced the normalized
event pipeline, bounded temporal model, terminal renderers, tshark adapter,
tests, documentation, and demo artifact in these five commits:

- `6cead8b` — bootstrap normalized event stream
- `fd5eafa` — add bounded temporal model and replay
- `8acb2db` — add terminal weather renderers
- `a7b5db6` — add tshark network adapter
- `e696e56` — harden synesthesia v0.1

The implementation was produced with AI-assisted development under human
direction and review.

### Originality and third-party software

Repository inspection found no copied implementation from Cava, Wireshark,
Ratatui examples, or other projects. The code uses ordinary Rust dependencies
and idiomatic patterns; third-party Rust dependencies remain governed by their
own licenses. Dependency use does not transfer those projects' authorship or
license terms to Synesthesia.

### Artifacts and initial qualification

`docs/demo.gif` is a real terminal recording generated from this project with
VHS, not a fabricated mockup.

The tshark adapter was fixture-tested during initial qualification because
`tshark` was unavailable on the build host. The exact TSV fixture and parser
remain in the repository.

### Experimental scheduler campaign

Lead collaboration: OpenAI Codex.

A later local research campaign added one narrow Linux scheduler source under
human direction and live qualification:

- `d795617` — add scheduler event source boundary
- `16f291f` — add live ebpf scheduler capture
- `a9ac4c9` — tune scheduler terminal weather
- `dd2bd99` — harden experimental scheduler source

The implementation uses a checked-in, original C tracepoint sensor and an
AI-assisted Rust collector/renderer integration. It does not copy Cava,
Wireshark, bpftrace, libbpf examples, or Aya examples. Aya is used as an
ordinary third-party dependency under its own license.

Live qualification on the development host exposed and corrected a kernel-BTF
type-name mismatch for the wakeup tracepoint. All four scheduler tracepoints
then attached successfully and produced a real ASCII snapshot. The checked-in
`examples/scheduler.ndjson` fixture is synthetic and sanitized; it does not
contain host process names, usernames, or unrelated machine activity.

### Experimental TCP-pathology campaign

Lead collaboration: OpenAI Codex.

A later local research campaign added one narrow Linux TCP-pathology source
under human direction and live qualification:

- `417a7cd` — add tcp pathology event source boundary
- `1a022c4` — add live ebpf tcp pathology capture
- `1a70ac7` — tune tcp pathology terminal weather
- `cd2f928` — harden experimental tcp pathology source

The original checked-in C tracepoint sensor and Rust helper integration use
Aya as an ordinary third-party dependency. They do not copy Wireshark,
libbpf-tools, bpftrace, Aya examples, or other TCP tracing implementations.
The selected tracepoint layouts were derived from the qualification host's
Linux headers and BTF rather than assumed from memory.

All three selected tracepoints attached successfully on the development host:
`tcp_retransmit_skb`, `tcp_send_reset`, and `tcp_receive_reset`. The
checked-in raw TSV and normalized NDJSON fixtures are synthetic and sanitized,
using only documentation-range addresses. They contain no captured host
addresses, payloads, process data, usernames, or application metadata.

### Experimental flight-recorder campaign

Lead collaboration: OpenAI Codex.

A later local research campaign added one bounded pre-trigger incident
recorder for the existing scheduler and TCP semantic sources:

- `658c936` — add bounded incident flight recorder
- `b5e36ec` — add scheduler and tcp flight triggers
- `f81518d` — integrate live incident recording
- `ec4abd7` — qualify synesthesia flight recorder
- `e82237f` — harden experimental flight recorder

The recorder was implemented as original Rust code under human direction. It
retains only normalized semantic events in the unprivileged renderer process;
it does not copy tracing backends, incident-management products, or rules
engines. No new third-party dependency was added.

The checked-in scheduler and TCP flight incidents are synthetic and sanitized.
The real live qualification recordings were kept under `/tmp` and were not
committed because they contain live scheduler identity or endpoint data.
Manual and automatic capture paths were qualified through both separate
collector helpers. The controlled TCP retransmit-rate incident and scheduler
event-rate incident both completed with zero reported kernel, collector, IPC,
renderer, malformed-record, writer, or prehistory-eviction loss. Exact counts
and workload caveats are preserved in
`docs/flight-recorder-qualification.md`.

### Particles, procfs, and binary distribution campaign

Lead collaboration: OpenAI Codex.

A later human-directed campaign added bounded source-neutral particles, one
unprivileged Linux procfs source, and the repository's first binary
distribution path:

- `3eed151` — add bounded particle weather overlay
- `6134b50` — add proc activity source boundary
- `84ae73c` — add nonroot proc activity weather

The particle model and procfs parser/tracker are original Rust implementations.
No htop, procps, btop, tracing-tool, or visualizer implementation was copied.
The procfs field selection follows the Linux procfs text interfaces and reads
no process arguments, environments, paths, file descriptors, usernames, or
memory.

The checked-in process fixture is synthetic and sanitized, using generic
process names. Real workload recordings remained under `/tmp`. The release
archive is assembled from locally built project binaries and the repository's
own license, notice, README, and release notes. Packaging uses standard Cargo,
GNU tar, gzip, and SHA-256 tools and adds no third-party runtime service or
installer.

### Diagnostics and distribution-polish campaign

Lead collaboration: OpenAI Codex.

A later human-directed campaign added the read-only doctor, Clap-derived shell
completions and man page, and one experimental kernel-weather recording.
`docs/tcp-kernel-weather.gif` is a real VHS recording of Synesthesia replaying
a sanitized excerpt from the prior live controlled TCP namespace/netem
capture. It is not presented as live capture.

The excerpt retains actual relative timing, pathology category, magnitude, and
direction for selected semantic pulses. It contains only the lab's
documentation-range `192.0.2.0/24` endpoints; unrelated host traffic was
excluded, timestamps were rebased, and no synthetic pathology was inserted.
The source excerpt and reproducible tape are checked in beside the other
examples. The GIF contains no terminal prompt, hostname, username, home path,
shell history, or real endpoint.

### Meter and theme campaign

Lead collaboration: Anthropic Claude (Claude Code, Fable 5).

A later human-directed campaign added host CPU/memory/network lanes to the
procfs source, the calibrated meter equalizer view, the rainbow and pastel
themes with an xterm-256 color tier, and one pre-existing overflow fix:

- `1d6b1cf` — fix cold theme inbound blue channel overflow at full intensity
- `5da3e62` — add rainbow and pastel themes, theme aliases, and an ANSI-256 tier
- `a39552d` — add host cpu, memory, and network lanes and a meter equalizer view
- `d5a2259` — repair meter semantics per external review findings
- `cdb75bc` — project meter rate lanes perceptually with a linear opt-in
- `9ba2882` — document meter, host lanes, themes, and their review qualification

This campaign used three AI systems in distinct functions under human
direction. Anthropic Claude implemented, tested, and documented the changes
and orchestrated the review workflow. OpenAI ChatGPT contributed design
critique and direction through the operator: the lane-contract framing, the
sample-and-hold gauge challenge, the honest-labeling and covered-duration
review challenges, and the perceptual-scaling decision. Moonshot Kimi served
as the independent external reviewer: a frozen-prototype desk review, a
repair re-review against explicit obligations, and two merge-gate verdicts,
all without repository access. The human operator set requirements, validated
the display against live workloads, and made the merge decision. The review
evidence is distilled in `docs/meter-theme-qualification.md`.

The meter, projection, and color implementations are original Rust. The
equalizer and peak-cap presentation follows the familiar audio-meter idiom as
a concept; no code was copied from Cava, Winamp-family visualizers, btop,
or other meters. The HSV conversion and xterm-256 quantization implement
standard published formulas. No new third-party dependency and no new
fixtures were added; live validation traffic (NFS, builds, browsing) was
observed interactively and never recorded into the repository.

## Provenance basis and limits

This document is a functional attribution record based on commit history,
repository artifacts, and documented working sessions. It is not a complete
forensic account of every proposal, rejected alternative, or tool interaction.

Model names and tools are recorded at the platform level; exact model versions
may vary across sessions and are not exhaustively reconstructed here.

## What this document does not claim

- No exact proportional attribution. Contributions are categorized by
  function, not quantified by token count or lines of code.
- Design and implementation were not cleanly sequential. Architecture,
  implementation, tests, and visual tuning informed one another.
- Dependency licenses are not reproduced here; Cargo metadata and the lockfile
  identify the dependency set used by the project.

---

This document reflects the project state as of 2026-07-31 and may be revised.
