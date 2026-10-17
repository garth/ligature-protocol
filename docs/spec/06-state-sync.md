# 06. Shared State and Synchronisation

**Status:** Draft.

## 1. Intent

The device registry, user settings and household permissions are stored as typed records
in an atproto Space on a self-hostable PDS. See
[ADR 0006](../adr/0006-atproto-space-shared-state.md). Spaces are currently alpha in
atproto, which this chapter treats as an accepted risk.

## 2. What lives in the Space

Device records, capability descriptors, grants, guardian records and non-secret settings.
Anything secret is stored as an MLS-encrypted payload (chapter 03). Group records and the
records of shared devices live in a group-owned Space rather than any one person's
(chapter 10). Open question: who hosts it; the household hub's PDS is the natural choice.

## 3. Permissions

A Space is permissioned: the user's devices can read and write; guardians and group
members receive narrower permissions. In a group Space, members read membership and shared
device records; admins change them. Open question: how LP permissions map onto whatever
permission model Spaces ship with.

## 4. Consistency and conflicts

Records are last-writer-wins at the PDS. Devices cache records and reconcile on reconnect.
Open question: whether LP needs CRDT semantics for any record type (settings seem likely).

## 5. Offline operation

Devices must keep working against their cache when the PDS is unreachable. Open question:
which operations (adding a device, revoking a device) are allowed to proceed offline and
how they are reconciled.

## 6. Open questions

- What happens when the user migrates PDS?
- Should devices verify Space records against the identity's signing chain independently
  of the PDS's own auth?
