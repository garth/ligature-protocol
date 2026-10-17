# 0030. In-person verification is mandatory; there is no remote joining

**Status:** Accepted
**Date:** 2026-10-17
**Amends:** item 6 of [ADR 0017](0017-master-key-identity-and-proximity-ceremony.md)

## Context

ADR 0017 left open whether two people who had both published their master keys could
recognise each other remotely, skipping the proximity ceremony, and listed "a way to
invite someone who is not in the room" among the reasons to publish a key. The protocol is
for devices owned by an individual or by a co-habitation group. Everyone who should ever
hold a `person` role or a group membership is someone the user meets. A remote path would
add attack surface for a case the protocol does not need to serve.

## Decision

1. The proximity ceremony (chapter 02) is the **only** way to establish a `person` role or
   a group membership. There is no remote joining, whether or not both parties have
   published their keys. The in-person step is a security feature, not a convenience.
2. Optional key publication (chapter 01 section 4) serves a human-readable name and relay
   listing only. It never establishes trust.
3. After a rebuild (chapter 07), a published key announces that the person has a new key;
   contacts still verify the new key in person before trusting it.

## Consequences

Impersonation at a distance is impossible by construction: nothing a stranger can do over
the network makes any device recognise them. The trade is that a household member who is
away for months cannot be added until they are home, which fits the intended use. Chapter
01's open question is closed.
