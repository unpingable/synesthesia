# Synesthesia v0.3.0

Data in. Terminal weather out.

This release turns process mode into a calibrated instrument:

- continuous host CPU and memory ratio gauges plus directional
  `/proc/net/dev` byte deltas, with physical-interface auto-selection and a
  repeatable `--net-interface` override;
- a calibrated `meter` equalizer view (`--view meter`, key `3`) with a stable
  six-lane contract, sample-and-hold gauges, covered-duration rates,
  retention-bounded peak caps, and clip markers, externally reviewed with its
  findings locked in as regression tests;
- perceptual square-root rate-lane projection, with a `--meter-scale linear`
  opt-in and status-line disclosure of projection and non-neutral gain;
- `rainbow` and `pastel` themes with hash-stable lane hues, theme aliases
  `matrix`/`ice`/`mono`, and an xterm-256 color tier between truecolor and
  named 16-color;
- a fix for the cold-theme inbound blue-channel overflow that panicked debug
  builds at full intensity.

The archive targets x86_64 Linux with glibc. `demo`, `stdin`, `replay`, and
`proc` and passive doctor run without privilege. Scheduler, TCP, and the
explicit `doctor --check-live` test require external BPF/perf-event privilege
and compatible kernel BTF and tracepoints; Synesthesia never invokes `sudo`,
installs capabilities, or creates setuid files.

The eBPF sensors capture no packet payload, process arguments, environment,
stack traces, or application-layer data. TCP recordings may contain endpoint
metadata. Process recordings may contain bounded process names and PIDs unless
`proc --anonymize` is used.

Synesthesia remains experimental. The meter measures your machine; it does not
flatter it.
