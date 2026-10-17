# 0007. Define schemas as AT Protocol Lexicons under org.ligatureprotocol.*

**Status:** Superseded by [ADR 0020](0020-json-schema-and-reverse-dns-names.md)
**Date:** 2026-10-17

## Context

Device records, capability descriptors and grants need a schema language with versioning,
tooling and a namespace convention. Since state lives in a Space (ADR 0006), using
atproto's own schema system avoids a second type system.

## Decision

All LP records and capability descriptors are defined as atproto Lexicons (Lexicon v1)
under the namespace `org.ligatureprotocol.*`. The domain is not yet registered; the
namespace is a placeholder until it is.

## Consequences

*Superseded on 2026-10-17.* With AT Protocol no longer used anywhere in LP, nothing specific
to it is kept. Schemas are JSON Schema; type names keep the reverse-DNS convention, which is
generic.

Original consequences follow.

LP gets code generation and validation from the atproto ecosystem. The placeholder
namespace must be replaced or the domain registered before any public release, since NSIDs
are meant to be owned by the controller of the reversed domain. Capability types also
become NSIDs, giving third parties a path to define their own.
