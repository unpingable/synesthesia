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

Synesthesia itself does not require capture privileges and does not perform
packet capture. External producers such as `tshark` may require elevated
privileges depending on the host. Keep those privileges confined to the
producer; piping its output into Synesthesia does not grant Synesthesia a need
for root access.

## Supported versions

The project is currently at an experimental `0.1.x` stage. Security fixes are
made on the current `main` branch; older commits are not maintained as separate
supported release lines.
