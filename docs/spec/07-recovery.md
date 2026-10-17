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

Any introducer of the new key may gossip a signed statement that a named old master key is
abandoned. Devices that receive it stop recognising the old key's bindings. Devices that
never receive it simply treat those bindings as an unknown person's. Open question: whether
the abandonment statement should be signed by the old key as well when it is still
available (loss without compromise), to defeat a forged abandonment by a third party.

## 4. Published keys

A person who has published their key (chapter 01) publishes the new one in the same place,
so contacts who are not in the room learn of the rebuild.

## 5. Open questions

- Whether implementations should offer an encrypted export of the local store (chapter 06
  section 6) so a rebuild can restore grants and known persons.
- How a shared device whose only owner has rebuilt gets a new owner without a factory
  reset.
