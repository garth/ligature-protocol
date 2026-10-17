# 05. Capabilities

**Status:** Draft.

## 1. Intent

A device advertises typed capabilities. Peers discover them, request access, and receive
explicit grants. See [ADR 0008](../adr/0008-capability-model.md).

## 2. Capability descriptors

A capability descriptor names a capability type (an NSID), the principal that offers it,
its scope (`user` or `device`, chapter 10), and type-specific parameters. Descriptors are
published in the device record and announced over the transport. See
[`org.ligatureprotocol.capability`](../../lexicons/org/ligatureprotocol/capability.json).

## 3. Initial capability families

The first families are: notifications, clipboard, files, remote input, media control,
telephony/SMS, displays, sensors. Each will become its own spec section with its own
Lexicon. Open question: which two or three families the proof of concept should cover
(clipboard is the current target).

## 4. Requests and grants

A device requests a capability from a peer; the peer (or the user, through the peer)
grants or denies. Grants are typed records with scope and expiry. See
[`org.ligatureprotocol.grant`](../../lexicons/org/ligatureprotocol/grant.json).

Defaults follow identity boundaries (ADR 0015): a person's own devices may use that
person's user-scoped capabilities without a prompt; anything across identities needs an
explicit grant, approved by the person whose capability it is or, for a device-scoped
capability, by the person present on the device or the device's policy (chapter 10).
Open question: whether a person may opt their own devices out of the implicit default.

## 5. Invocation

Once granted, a capability is invoked over MLS-protected application messages. Each
capability family defines its own operations. No wire format is defined in this draft.

## 6. Bridging

Capabilities may be backed by an external protocol (KDE Connect, LocalSend, Matter, MPRIS).
A bridge is just a device that advertises a capability on behalf of something else. See
[ADR 0010](../adr/0010-interop-bridges.md).

## 7. Open questions

- Versioning and negotiation of capability types.
- Capability discovery across group identities: what a member can see of another
  member's devices before asking.
