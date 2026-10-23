# 07. Loss, Compromise and Rebuild

**Status:** Draft.

## 1. Intent

The protocol keeps no state that must survive the loss of every device, so there is
nothing to recover. A person who loses their devices, or whose key may have leaked,
rebuilds. See [ADR 0018](../adr/0018-rebuild-instead-of-recovery.md).

## 2. Rebuilding

1. A new device generates a new master key (chapter 01) and becomes the first introducer.
2. Each device the person still holds is re-paired by the ceremony of chapter 02.
3. Household members re-verify the new master key in person with the `person` role;
   shared-device owner sets are updated by any remaining owner, or re-established on the
   principal's pairing page.
4. Standing grants are re-approved as they are next needed.

The cost is the effort of re-introducing devices and people, which the design accepts in
exchange for having no shared state.

## 3. Abandoning a key

An abandonment is a revocation whose subject is the person's own master key, signed with
that person's self-signing key by an introducer that still holds it
([ADR 0039](../adr/0039-revocation-scope-and-live-session-close.md)); it is gossiped like
any revocation. Devices that receive it stop recognising the old key and its bindings.
Nobody can abandon a key on another person's behalf, so a person who has lost every
introducer has no abandonment to gossip: they rebuild (section 2), household members
re-verify them and may revoke the old `person` role on their own devices, and devices that
hear neither simply treat the old key's bindings as an unknown person's.

## 4. Open questions

- Whether implementations should offer an encrypted export of the local store (chapter 06
  section 6) so a rebuild can restore grants and known persons.
- How a shared device whose only owner has rebuilt gets a new owner without a factory
  reset.
