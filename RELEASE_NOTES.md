# Synesthesia v0.2.1

Data in. Terminal weather out.

This patch release makes installation and experimental-source troubleshooting
less folkloric:

- `synesthesia doctor` with read-only text and schema-v1 JSON reports;
- Bash, Zsh, and Fish completions generated from the actual command tree;
- a generated `synesthesia(1)` manual page;
- a real rendering of a sanitized replay excerpt from the controlled TCP eBPF
  capture;
- a versioned archive containing `bin/` and `share/` trees.

The archive targets x86_64 Linux with glibc. `demo`, `stdin`, `replay`, and
`proc` and passive doctor run without privilege. Scheduler, TCP, and the
explicit `doctor --check-live` test require external BPF/perf-event privilege
and compatible kernel BTF and tracepoints; Synesthesia never invokes `sudo`,
installs capabilities, or creates setuid files.

The eBPF sensors capture no packet payload, process arguments, environment,
stack traces, or application-layer data. TCP recordings may contain endpoint
metadata. Process recordings may contain bounded process names and PIDs unless
`proc --anonymize` is used.

The new GIF is replay, not a live terminal recording. Its semantic pulses came
from the real controlled namespace/netem eBPF capture and contain only
documentation-range endpoints.

Synesthesia remains experimental. Doctor diagnoses the privilege terrarium; it
does not tend it.
