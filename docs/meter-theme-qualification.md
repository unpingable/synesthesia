# Meter and theme qualification

This note records the review-gated development of the meter view, the host
telemetry lanes, and the color-theme extension merged on 2026-07-31 as five
commits ending at `cdb75bc`. It is evidence about those diffs as reviewed and
about one x86_64 Linux 6.8 workstation, not a benchmark or a universal claim.

## Process

Each feature line was developed as an ordered patch series against
`f783d25` (v0.2.1). The meter prototype was frozen with exact base
provenance before review and never modified afterward; defects were repaired
in a separate bounded delta. Review was performed by an independent external
model (Kimi CLI, non-interactive, the full patch embedded in the prompt, no
repository access). External reviews are desk checks of diffs; execution
evidence comes from the local gate: formatter, strict Clippy, and the full
test suite, run at every stage with pass decided by exit code.

Unrelated changes kept separate review identities. A latent base bug found
during theme testing was extracted into its own single-purpose patch rather
than riding inside the theme series.

## First meter review

The frozen prototype review returned eight findings. The two most severe
were display-integrity defects:

| # | Finding |
| --- | --- |
| 1 | Rate lanes divided by a configured window, not actually covered duration; peaks divided by the configured interval. Stalls, jitter, and replay distorted apparent throughput. |
| 2 | Gauge bars showed the maximum of recent history with a fabricated release glide instead of the newest observation. |
| 3 | Per-process accounted I/O was labeled as disk throughput. |
| 4 | One malformed `/proc/net/dev` row discarded every interface in the sample. |
| 5 | Network event magnitude was clamped but its label carried the unclamped value. |
| 6 | A partially matched `--net-interface` selection was silently narrowed. |
| 7 | Host tick and network counter resets shared the process reset diagnostic. |
| 8 | The generic fallback used `0` as an empty-bucket sentinel, misreading a genuine hash of zero. |

## Repair and re-review

The repair delta addressed all eight findings. Rates divide by covered
duration derived from event timestamps; gauges are sample-and-hold with a
staleness deadline derived from observed cadence; lanes are labeled
`proc r`/`proc w`; parsing, clamping, selection surfacing, per-domain reset
counters, and the option-typed bucket tag follow the review's minimal fixes.
Findings 1 and 2 are locked in as regression unit tests annotated with their
provenance.

The re-review received the original findings as explicit obligations and
returned FIXED verdicts on all eight, independently re-deriving the envelope
and covered-duration arithmetic. It reported four residual observations,
recorded here as known-open refinements rather than defects:

- a lone baseline-less delta still divides by the one-second recency target;
- rate lanes hold their last value until retention expires rather than
  decaying when stale, unlike gauges;
- skipped malformed `/proc/net/dev` rows are counted in the shared
  `unreadable` diagnostic;
- gauge staleness derives from the single newest inter-sample gap, which one
  jittered pair can shorten.

## Perceptual scaling review

A fixed 1 Gbit/s / 512 MiB/s linear display made ordinary workstation
activity nearly invisible. The replacement projects rate lanes through
`sqrt(rate/ceiling)` by default with `--meter-scale linear` as the literal
alternative. An interim ×8 linear display gain was superseded before testing
and replaced; the successor patch records that supersession.

The merge-gate review verified the projection is presentation-only, a fixed
pure function (the same rate always draws the same height, unlike adaptive
normalization), that gauges stay linear and unscaled, that a clip marker
means the true rate exceeded the real ceiling in both modes, and that the
status line names the projection and any non-neutral gain. Verdict: merge,
with non-blocking observations (clip-flag wording under non-neutral gain,
clip marker overpainting a coinciding peak tick, startup-fixed scale mode).

## Theme review

The color review hand-audited every u8 arithmetic path across six themes,
three color depths, four directions, and the full intensity range: the cube
quantizer's maximum index is 231, the gray ramp spans 232–255, and float→u8
casts saturate. It confirmed hue derives from the stable category hash alone
so lane color never changes with activity, that all depth tiers derive from
one RGB definition, that the four existing themes render byte-identically in
truecolor apart from the overflow fix, and that ASCII mode remains unstyled.
Verdict: merge. Its `NO_COLOR` observation (256-color `TERM` selecting the
indexed tier) is moot in practice because `NO_COLOR` forces ASCII mode,
which never styles.

The overflow itself — the cold theme's inbound blue channel exceeding u8 at
full intensity, a debug-build panic and a silently wrapped release color —
predates this work and was fixed in its own reviewed commit.

## Merged verification

The merged tree was compared byte-for-byte against the fully stacked
patch-series worktree that had been validated interactively. The final gate
passed with 166 tests (two privileged live-attachment tests ignored by
design). Qualitative live checks on the qualification host covered synthetic
CPU load, build-generated storage writes, and NFS traffic; the operator
validated that perceptual scaling distinguishes an idle system from an
active one without rendering ordinary activity as saturation. Reads from
`/dev/zero` to `/dev/null` correctly produced no accounted storage I/O.
