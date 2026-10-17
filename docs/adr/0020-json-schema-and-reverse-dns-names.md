# 0020. JSON Schema for records and messages, reverse-DNS type names

**Status:** Proposed
**Date:** 2026-10-17
**Supersedes:** [ADR 0007](superseded/0007-atproto-lexicons.md)

## Context

ADR 0007 chose AT Protocol Lexicons because shared state lived in an atproto Space and the
identity was an atproto DID. Both are gone (ADRs 0016 and 0017), and the decision was taken
that nothing specific to AT Protocol remains. Lexicon also has concrete costs for LP: no
floating-point numbers (sensor capabilities need them), a `record` framing that implies a
repository, byte-counted string lengths, and a small toolchain tied to one ecosystem.

Alternatives considered: JSON Schema (2020-12), with the largest validator and code
generation ecosystem and no coupling to any stack; CDDL, the natural choice if the wire
format is CBOR, but with thinner tooling and less familiarity; Protocol Buffers, compact
and strongly typed but binary-only and less readable for a specification; keeping Lexicon.

## Decision

1. Records and messages are defined in **JSON Schema 2020-12**, one file per schema under
   `schemas/`, each marked draft.
2. **Type names use reverse-DNS**: `org.ligatureprotocol.device`,
   `org.ligatureprotocol.capability.clipboard`. This is the D-Bus and Java convention, not
   an atproto one, and it gives third parties a namespace for their own capability types.
   The domain is a placeholder until registered.
3. **Keys** are multibase strings with a multicodec prefix. **Timestamps** are RFC 3339.
   **Bytes** are base64url strings in JSON.
4. The wire encoding (JSON, CBOR or other) is a separate, pending decision. If CBOR is
   chosen, CDDL may be generated from or kept alongside these schemas.

## Consequences

Any language's JSON Schema tooling validates LP data, and CI compiles every schema with a
standard validator. Floats, closed enums and additional-property control are available.
Capability types remain namespaced strings. The specification loses the atproto codegen
story, which it no longer needed. The `lexicons/` directory is replaced by `schemas/`.
