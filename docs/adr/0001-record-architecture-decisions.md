# 0001. Record architecture decisions

**Status:** Accepted
**Date:** 2026-10-17

## Context

Ligature Protocol makes a number of foundational choices (identity root, encryption,
transport, state storage) whose rationale would otherwise live only in chat logs and
contributors' heads. Future contributors need to know not just what was decided but why,
and what the alternatives were.

## Decision

We record architecture decisions as ADRs in `docs/adr/`, using the format described by
Michael Nygard: a numbered file with Context, Decision and Consequences sections and a
Status line. New decisions start as "Proposed". A decision is changed by adding a new ADR
that supersedes the old one, not by rewriting history.

## Consequences

Decisions are discoverable and reviewable in pull requests. The ADR set becomes the
authoritative summary of the architecture; the spec chapters cite ADRs rather than
re-arguing them. Contributors must write an ADR for any change that touches a recorded
decision, which adds a small amount of friction.
