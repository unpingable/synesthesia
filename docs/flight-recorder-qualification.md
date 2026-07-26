# Flight-recorder qualification

This note records the local research qualification performed on 2026-07-26.
It is evidence for one host and workload, not benchmark-grade threshold
calibration or a universal kernel-support claim.

## Host and build

- Linux `6.8.0-136-generic`, x86_64
- readable `/sys/kernel/btf/vmlinux`
- explicit externally supplied `sudo`
- release renderer plus separate scheduler and TCP collector helpers
- flight recording in the unprivileged process after semantic normalization

The complete Rust gate passed before live qualification. The privileged
helpers had already been qualified against all four scheduler tracepoints and
the three TCP tracepoints documented in [design.md](design.md).

## Manual live boundary

Both helpers were armed with a one-second pre-trigger window, a 500 ms tail,
and the interactive `t` trigger.

The quiet TCP run truthfully contained no TCP pathology events: zero pre and
zero post events, a complete 500 ms tail, and zero loss at every boundary. The
scheduler run retained 717 pre-trigger and 335 post-trigger semantic pulses,
covering 0.993340 seconds before and 0.500000 seconds after the trigger. It
also reported zero kernel, collector, IPC, renderer, malformed, and writer
loss.

Both files parsed as complete JSON streams and replayed without privilege.
No `.part` file remained.

## Automatic TCP incident

The contained namespace lab was run against:

```sh
sudo ./target/release/synesthesia ebpf tcp \
  --flight-recorder /tmp/synesthesia-flight-tcp-rate.ndjson \
  --pre-trigger 10s --post-trigger 5s \
  --trigger tcp-retransmit-rate=100 --mode ascii

sudo ./examples/tcp-pathology-lab.sh
```

The trigger fired after two of three 250 ms windows crossed 100 retransmitted
semantic events/s. Eleven pre-trigger pulses covered the 333 ms onset; their
bounded aggregate counts rose through 11, 46, 110, 86, 80, 91, 71, and 81 as
the lab impairment developed. Sixty post-trigger pulses continued for the full
five-second tail and included a received-reset event.

In normalized semantic counts, the incident retained 858 retransmits before
the trigger and 3,313 retransmits plus two received resets afterward. It
reported zero loss at all six recorded boundaries and no prehistory capacity
eviction.

An earlier `tcp-reset` qualification fired truthfully on a sent reset at the
healthy-transfer boundary. That proved the immediate reset trigger but was
not accepted as evidence of pathology buildup; the retransmit-rate run above
is the controlled incident used for qualification.

## Automatic scheduler incident

The qualified command used the source-specific default:

```sh
sudo ./target/release/synesthesia ebpf scheduler \
  --flight-recorder /tmp/synesthesia-flight-scheduler-default.ndjson \
  --pre-trigger 5s --post-trigger 3s \
  --trigger auto --mode ascii

stress-ng --cpu "$(nproc)" --timeout 5s
```

The initial attempted threshold of 20,000 events/s did not fire. Measurement
of the preceding manual recording showed 250 ms scheduler windows ranging
from 12,464 to 18,440 events/s, so the documented 15,000/s default was retained
rather than guessed upward.

The automatic run fired after 0.767959 seconds. It retained 443 pre-trigger
pulses covering 0.676798 seconds and 1,550 post-trigger pulses over the full
three-second tail. Their aggregate semantic counts were 13,244 before and
48,804 after the trigger: 38,083 switches, 22,404 wakeups, and 1,561
migrations overall. All recorded loss and eviction counters were zero.

Human terminal timing and unrelated host activity make this a qualitative
live-instrument check. The recording proves that the semantic scheduler rate
crossed the stated rule and that the bounded tail captured continued scheduler
weather; it does not prove that `stress-ng` alone caused every retained event.

## Replay and cleanup

Both automatic incidents replayed unprivileged in deterministic ASCII mode.
Status identified `POST`, source, trigger kind, and five loss boundaries; the
field contained the stable trigger marker. Mechanical inspection found 30
lines, a maximum width of 100, zero non-ASCII bytes, and zero escape bytes.

After the lab and workload completed:

- no flight-recorder `.part` file remained;
- no test namespace or veth remained;
- no Synesthesia collector, `iperf3`, or `stress-ng` process remained;
- no Synesthesia BPF pin was found.

The real `/tmp` incidents contain live endpoint or scheduler identity data and
are deliberately not committed. The checked-in fixtures remain synthetic and
sanitized.
