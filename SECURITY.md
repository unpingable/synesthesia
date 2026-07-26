# Security

Synesthesia is experimental terminal visualization software. It consumes
potentially untrusted line, NDJSON, and tshark TSV input and renders activity
inside a terminal. It is not a security boundary or packet-inspection tool.

## Reporting

Please report suspected vulnerabilities privately through GitHub's security
reporting facilities when available. If private reporting is unavailable, open
an issue containing only enough detail to establish contact; do not publish
working exploit material or sensitive data.

No response-time or remediation SLA is promised.

## Relevant issues

Reports are especially useful when they involve:

- terminal escape handling or failure to restore terminal state;
- malformed-input crashes or parser confusion;
- memory, CPU, or channel-exhaustion behavior;
- unsafe handling of recording or replay files;
- mistakes around privilege boundaries.

Network and generic-input modes do not require capture privileges and do not
perform packet capture. External producers such as `tshark` may require
elevated privileges depending on the host.

The experimental scheduler source is different: its separate Linux collector
must load BPF programs and attach perf-event tracepoints. Synesthesia never
invokes `sudo`, changes sysctls, mounts tracefs, weakens lockdown or
unprivileged-BPF policy, installs capabilities, or creates setuid programs.
Users explicitly choose the credentials or capabilities supplied to the
launcher and helper. Running `sudo synesthesia ebpf scheduler` also runs the
launcher with those credentials; the process boundary is not a privilege-drop
mechanism.

The collector reads scheduler tracepoint fields limited to timestamps, CPU
IDs, PIDs, previous state, and task movement. It does not collect command
arguments, paths, environments, stack traces, or payloads. Reports involving
malformed helper records, verifier/error misclassification, ring-buffer or
pipe exhaustion, lingering attachments, or privilege-boundary mistakes are
particularly relevant.

## Supported versions

The project is currently at an experimental `0.1.x` stage. Security fixes are
made on the current `main` branch; older commits are not maintained as separate
supported release lines.
