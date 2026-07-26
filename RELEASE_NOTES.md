# Synesthesia v0.2.0

Data in. Terminal weather out.

This is the first downloadable Linux binary release of Synesthesia. It adds:

- a bounded, event-driven ember layer for heavy semantic activity;
- unprivileged Linux process weather from `/proc`;
- experimental Linux eBPF scheduler weather;
- experimental TCP retransmit lightning and reset impacts;
- a bounded pre-trigger flight recorder with unprivileged replay;
- deterministic ASCII snapshots and color ANSI rendering.

The archive targets x86_64 Linux with glibc. `demo`, `stdin`, `replay`, and
`proc` run without privilege. Scheduler and TCP modes require explicit external
BPF/perf-event privilege and compatible kernel BTF and tracepoints; Synesthesia
never invokes `sudo`, installs capabilities, or creates setuid files.

The eBPF sensors capture no packet payload, process arguments, environment,
stack traces, or application-layer data. TCP recordings may contain endpoint
metadata. Process recordings may contain bounded process names and PIDs unless
`proc --anonymize` is used.

Synesthesia remains experimental. It is a visual instrument, not packet
inspection, a scheduler profiler, a diagnosis engine, or a production
observability service.
