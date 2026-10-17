# 0022. Ad-hoc sharing between people's devices

**Status:** Proposed
**Date:** 2026-10-17
**Extends:** [ADR 0008](0008-capability-model.md), [ADR 0015](0015-shared-devices-and-groups.md)

## Context

People in a household want to hand each other things: a photo, a link, a file, a note,
from one phone to another or to the family laptop, without setting up a sync folder or a
chat. Apple's AirDrop is the reference experience: pick a nearby device, send, the receiver
accepts. LP has the pieces (known persons, manifests, grants, pairwise streams) but no
capability that puts them together, and its `files` family is about browsing and
transferring files within one person's devices.

## Decision

1. **A `share` capability family.** A device that is willing to receive shares advertises
   `org.ligatureprotocol.capability.share` in its manifest. It is user-scoped: it belongs
   to a binding, so a share to the family laptop is a share to a particular person's
   account on it. Payloads are text, links and files, one or several per share.
2. **Each share is accepted by the receiver.** A share request names the sender (a known
   person or group member) and describes the payload; the receiving person accepts or
   declines on the receiving device. Acceptance is a one-time grant (basis `approval`,
   not standing). A person may make it standing for a specific sender ("always accept from
   my partner").
3. **Discovery is by manifest.** After authentication, a device sends another person's
   device only the capabilities that person could use. `share` is therefore visible to
   known persons and group members and to nobody else, so a sender sees exactly the
   devices they may send to.
4. **Contacts only by default.** Shares are accepted from known persons and group members.
   An opt-in **open mode**, "anyone nearby for a few minutes", in which an unknown but
   authenticated key may offer a share and nothing else, is left as an open question; if
   adopted it is the single exception to "nothing before recognition" in chapter 03.
5. **Transfer is the pairwise session.** The payload travels on the `share` stream of the
   QUIC session between the two devices, so it is readable by exactly those two.

## Consequences

Sharing between family members works with no new trust machinery: knowing someone is
enough to see their devices, and each item still needs their consent. The `files` family
keeps its meaning (browsing and sync within a person's own devices). Open mode, if
adopted, needs its own threat-model entry, since it lets strangers reach a prompt. Shares
to a group-owned device (a photo to the hub's frame) are a device-scoped variant and are
left to the hub's policy under ADR 0021.
