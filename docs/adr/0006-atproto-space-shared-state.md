# 0006. Store shared state as typed records in an atproto Space

**Status:** Proposed
**Date:** 2026-10-17

## Context

Devices need a shared, durable view of the device registry, settings and household
permissions that survives any single device being offline or lost. Options considered: a
CRDT replicated only between devices, a Matrix room, a Syncthing folder, and an atproto
Space (permissioned data on a PDS, currently alpha).

## Decision

Shared state is stored as typed Lexicon records in an atproto Space on a self-hostable PDS.
Secrets are stored only as MLS-encrypted payloads; the PDS never holds plaintext secrets.

## Consequences

The same identity and hosting serve both identity and state. The PDS becomes an
availability dependency for bootstrap and reconciliation, but not for real-time traffic.
Spaces are alpha, so the API may change and LP tracks that risk explicitly. Devices need a
cache and reconciliation strategy for offline operation.
