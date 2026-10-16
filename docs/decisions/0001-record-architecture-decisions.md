# 0001. Record architecture decisions

- **Status:** Accepted
- **Date:** 2026-10-17

## Context

This project rebuilds a working Python tool as a native Rust app, and its first
phase is about architecture rather than features. Contributors need to see what
was chosen, what else was considered and why, without reading old issues and
pull requests. Several choices also wait on measurements, so a reader has to be
able to tell a settled decision from a working direction.

The [Rust architecture research](../research/rust-architecture.md#licence-audit-and-contributor-documentation)
recommends the `adr-tools` record fields (number and title, date, status,
context, decision, consequences), extended with alternatives, dependency
licences and pins, evidence and acceptance measurements.

## Options considered

| Option | Trade-offs |
| --- | --- |
| Decisions live only in issues and pull requests | Nothing extra to write, but the reasoning is scattered, hard to find and easy to contradict later. |
| One architecture document that is edited in place | Easy to read in one go, but edits erase what was decided before and why it changed. |
| One numbered record per decision, in the `adr-tools` style with the extra fields above | Each decision can be read, linked and replaced on its own. Costs one short file per decision and some discipline to keep statuses current. |

## Decision

Record each architecture decision as its own file in `docs/decisions/`, named
`NNNN-short-title.md`. Numbers have four digits, increase by one and are never
reused.

Every record has these parts:

- **Status** and **Date**.
- **Context**: the problem and the constraints that apply.
- **Options considered**: each with its trade-offs, its evidence and, for a
  dependency, its licence and the version or commit that was examined.
- **Decision**: what was chosen, stated so that someone can follow it.
- **Consequences**: what becomes easier, harder or required as a result.
- **Accept when** (for a proposed record): the measurement or check that
  settles it. **Revisit when** (for an accepted record): what would make us
  reconsider it.
- **Evidence**: links to the project's research, parity checklist or benchmark
  results, or to upstream sources pinned to a version or commit.

A record has one of three statuses:

- **Proposed**: the direction is chosen, but it waits on a named measurement or
  check. Work may build on it, but should keep the alternative reachable, for
  example behind the narrow crate boundary the record describes.
- **Accepted**: the decision holds until a later record replaces it.
- **Superseded by NNNN**: a later record replaced it. The later record says
  which record it supersedes.

Records change in these ways:

- A proposed record becomes accepted in the pull request that adds the
  evidence named under **Accept when**.
- If the evidence points to a different option, write a new record that
  supersedes the proposed one instead of rewriting it.
- To change an accepted decision, write a new record. In the old record, change
  only the status line.
- Fixing a typo or a broken link is not a new decision.

Evidence follows the research vocabulary: **observed** (seen in a primary
source or measured here), **reported** (stated upstream but not checked here)
and **inferred** (this project's reasoning). Every dependency named in a
decision has its licence recorded in that decision.

[`docs/ARCHITECTURE.md`](../ARCHITECTURE.md) lists every record with its
status.

## Consequences

- A change that alters an architectural rule must update the relevant record
  or add a superseding one in the same pull request.
- Readers can trust that an accepted record describes the current rule and a
  proposed record names what is still unproven.
- The research documents remain the place for detailed evidence. Records
  summarise and link them rather than repeating them.

## Revisit when

The number of records makes the index hard to use, or contributors find a
different format clearer in practice.

## Evidence

- [Licence audit and contributor documentation](../research/rust-architecture.md#licence-audit-and-contributor-documentation)
  in the Rust architecture research, including the `adr-tools`
  [template at `b3279ba`](https://github.com/npryce/adr-tools/blob/b3279baf9be2207d1a4f4bbd608fd0b591c72aee/src/template.md).
