# 0032. No key publication; display names are self-asserted signed facts

**Status:** Accepted
**Date:** 2026-10-17
**Amends:** item 6 of [ADR 0017](0017-master-key-identity-and-proximity-ceremony.md)

## Context

ADR 0017 allowed a person to publish their master key at a DNS name. After ADR 0030 that
publication could never establish trust, leaving it two uses: a human-readable name for a
key one already trusts, and a place to list relays. Relays already travel by gossip between
a person's devices, and a name is better carried with the key itself. The specification
also had no answer to where a known person's display name comes from.

## Decision

1. **There is no key publication.** The protocol has no DNS or HTTPS component. A master
   key is learned only in the proximity ceremony.
2. **A person's display name is self-asserted.** Every person has a signed **person fact**
   carrying their master key, display name, self-signing and user-signing public keys with
   the master key's signatures over them, and the fact's own signature by the self-signing
   key. It is handed over in the ceremony that establishes the `person` role and gossiped
   like any other fact (chapter 06). This is the same shape as a device record's display
   name: whoever owns the key names it.
3. Whether an implementation lets a user relabel a known person locally is a user-interface
   matter that no other device ever sees; the specification does not cover it.

## Consequences

One fewer component, one fewer schema concept, and the key chain finally has a record
that carries it: the person fact is what a verifier uses to check a binding's signature
back to a master key. A misleading self-chosen name is possible within a household and is
treated as a prank, not an attack, given the in-person step. Chapter 01's last open
question is closed; the `publication` module and `PublishedName` type leave the scaffold.
