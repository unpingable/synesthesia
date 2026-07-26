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

This document reflects the project state as of 2026-07-26 and may be revised.
