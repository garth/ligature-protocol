# Ligature Protocol Specification: Overview

**Status:** Draft. This document is incomplete and will change without notice.

## 1. Introduction

Ligature Protocol (LP) binds the devices belonging to one person into a single trusted
group so that any device can discover and use the capabilities of the others. It is
designed for Linux desktops first, with Android, tablets, home hubs and (within platform
limits) iPhones as peers. This chapter sets out the goals, the layering, and how the
remaining chapters fit together.

## 2. Goals and non-goals

LP aims to be local-first, self-hostable, end-to-end encrypted, and built on existing open
standards. It does not aim to be a general messaging protocol, a file-sync engine, or a
replacement for the protocols it bridges to. Open question: whether multi-user households
(several identities sharing hub devices) are a first-class goal or a later extension.

## 3. Terminology

Terms such as *identity*, *device*, *device set*, *capability*, *grant*, *guardian* and
*Space* are defined in the [glossary](../glossary.md). Normative keywords (MUST, SHOULD,
MAY) follow RFC 2119 and RFC 8174 once this document leaves draft status.

## 4. Layering

LP is organised as five layers plus a shared-state store:

1. **Identity** (chapter 01): an AT Protocol DID is the root of trust for a person.
2. **Device trust** (chapter 02): each device has its own keys, cross-signed by the identity.
3. **Encryption** (chapter 03): one MLS group per device set protects all peer traffic.
4. **Discovery and transport** (chapter 04): mDNS/DNS-SD and BLE locally, a WireGuard
   overlay remotely.
5. **Capabilities** (chapter 05): typed capability descriptors, requests and grants.

Shared state (chapter 06) lives in an atproto Space on a PDS. Recovery (chapter 07)
describes how a person regains control after losing devices. Chapters 08 and 09 cover
platform constraints and the threat model.

## 5. Document conventions

Each chapter states its intent per section and lists open questions inline. Wire formats,
message encodings and state machines are deliberately absent from this draft and will be
added once the architecture decisions in `docs/adr/` are accepted.

## 6. Relationship to AT Protocol

LP reuses AT Protocol for identity (DIDs and handles), schemas (Lexicons) and permissioned
shared state (Spaces). LP does not use the atproto firehose or relays for device traffic.
Open question: how much of the atproto OAuth and session model LP devices should adopt
when talking to the PDS.
