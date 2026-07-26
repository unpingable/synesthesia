# Security

Synesthesia is experimental terminal visualization software. It consumes
potentially untrusted line, NDJSON, and tshark TSV input, and reads mutable
Linux procfs records, before rendering activity inside a terminal. It is not a
security boundary, packet-inspection tool, or process profiler.

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

The experimental scheduler and TCP-pathology sources are different: their
separate Linux collectors must load BPF programs and attach perf-event
tracepoints. Synesthesia never invokes `sudo`, changes sysctls, mounts
tracefs, weakens lockdown or unprivileged-BPF policy, installs capabilities,
creates setuid programs, or configures network namespaces and qdiscs. Users
explicitly choose the credentials or capabilities supplied to the launcher
and helper. Running either eBPF command through `sudo` also runs the launcher
with those credentials; the process boundary is not a privilege-drop
mechanism.

The collector reads scheduler tracepoint fields limited to timestamps, CPU
IDs, PIDs, previous state, and task movement. It does not collect command
arguments, paths, environments, stack traces, or payloads. Reports involving
malformed helper records, verifier/error misclassification, ring-buffer or
pipe exhaustion, lingering attachments, or privilege-boundary mistakes are
particularly relevant.

The TCP collector reads only tracepoint-provided endpoint addresses, ports,
address family, socket state, CPU, kind, and time. It does not read payloads,
socket contents, process arguments, TLS metadata, or application protocols.
Endpoint addresses in a live recording may still be sensitive operational
data; inspect recordings before sharing them.

Flight-recorder incidents have the same sensitivity as ordinary normalized
recordings: they may contain scheduler PIDs or TCP endpoint addresses even
though they never contain payloads, process arguments, or stack traces. The
recorder refuses existing final and partial paths. A failed capture may retain
an explicitly suffixed `.part` file; inspect or remove it deliberately rather
than assuming failure wrote nothing.

Linux process mode is unprivileged and reads only `/proc/stat`,
`/proc/meminfo`, `/proc/<pid>/stat`, and readable `/proc/<pid>/io`. It does not
read process arguments, environments, working directories, executable paths,
open files, usernames, cgroup paths, or memory. Ordinary process recordings
may contain bounded `comm` names and PIDs. `proc --anonymize` replaces both
with stable opaque identities for that session, but it is not a general
redaction guarantee for labels supplied by other sources.

Release archives contain executable eBPF collector helpers next to the
launcher so sibling lookup is explicit. They have ordinary `0755` mode only:
no setuid bit and no file capability. Synesthesia does not install or
privilege them. `demo`, `stdin`, `replay`, and `proc` are unprivileged;
scheduler and TCP collection remain explicit privileged operations.

`examples/tcp-pathology-lab.sh` is an explicit root-operated qualification
tool, not code invoked by Synesthesia. It creates two temporary network
namespaces and a private veth/qdisc, refuses non-root execution, never changes
a host interface or route, and removes its resources through an exit trap.
Review shell scripts before executing them with privilege.

## Supported versions

The project is currently at an experimental `0.2.x` stage. Security fixes are
made on the current `main` branch; older commits are not maintained as separate
supported release lines.
