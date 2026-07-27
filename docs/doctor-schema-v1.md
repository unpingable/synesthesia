# Doctor report schema v1

`synesthesia doctor --format json` emits one JSON object. Schema version `1`
has these stable top-level fields:

- `schema_version`: integer `1`
- `synesthesia_version`: package version string
- `git_commit`: embedded 40-character commit or `"unknown"`
- `platform`: `os`, `architecture`, `kernel`, and conservative `libc` strings
- `checks`: ordered check objects
- `mode_summary`: stable command names mapped to readiness states
- `privacy_notice`: the report's public-sharing boundary

Each check contains:

- `id`: stable dotted identifier
- `group`: `build`, `terminal`, `proc`, `ebpf`, `scheduler`, or `tcp`
- `label`: short human label
- `status`: `pass`, `warn`, `fail`, `unknown`, `not_applicable`, or
  `not_tested`
- optional `observed`: a bounded JSON scalar, array, or object
- `summary`: what was observed—and, where relevant, what was not proven
- optional `remediation`: one safe next step
- `safe_to_share`: whether the check is designed for a public bug report

Mode readiness is one of `available`, `available_with_limitations`,
`unsupported_platform`, `missing_prerequisite`, `permission_required`,
`not_included_in_build`, or `unknown`.

Exit status `0` means the report completed and generally usable modes have no
hard failure. Passive warnings about experimental privileged modes do not make
the command fail. Exit status `1` means an explicitly requested check, such as
`--check-live`, failed. Exit status `2` means doctor itself could not construct
or write a valid report.

Schema v1 contains no captured activity, process list, hostname, username,
home directory, endpoint, argument, environment content, or raw capability
blob. Adding fields is permitted within schema v1; changing existing field or
enum meanings requires another schema version.
