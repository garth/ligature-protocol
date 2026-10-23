# 0008. Typed capabilities with explicit discovery, request and grant

**Status:** Proposed; the Lexicon per family is superseded by [ADR
0020](0020-json-schema-and-reverse-dns-names.md) (a JSON Schema per type); the capability model
stands
**Date:** 2026-10-17

## Context

Devices differ widely in what they can do. A phone has telephony and sensors; a desktop
has displays and a clipboard; a hub has neither. Peers need to learn what is on offer and
users need control over what is shared, especially once household members join.

## Decision

Devices advertise typed capabilities (initially: notifications, clipboard, files, remote
input, media control, telephony/SMS, displays, sensors). Peers discover capabilities,
request them, and receive explicit grants. Grants are typed records with scope and expiry.

## Consequences

The model is uniform across capability families and extensible by NSID. Every new
capability family needs a Lexicon and a spec section. Defaulting grants between a user's
own devices to "allow" is tempting for usability and is left as an open question in spec
chapter 05.
