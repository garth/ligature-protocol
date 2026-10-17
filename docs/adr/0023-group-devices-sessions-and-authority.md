# 0023. Group devices: registered to the group, sessions, and the authority of a request

**Status:** Proposed; items 4 and 5 superseded by [ADR 0024](0024-device-security-out-of-scope.md)
**Date:** 2026-10-17
**Extends:** [ADR 0021](0021-group-key-and-group-owned-devices.md), [ADR 0015](0015-shared-devices-and-groups.md)

## Context

ADR 0021 covered devices with no accounts, such as a lamp. A family tablet or television
is also the household's, but it has a screen, is used by whoever picks it up, and often
needs no login. Two questions follow. First, who owns it: ADR 0015's rule that the first
person to pair a device becomes its owner is wrong for a device the family bought
together. Second, and harder: such a device can be driven by anyone physically present
with no authentication, so what may it do towards members' devices? If the television
could pull Alice's clipboard or notifications, anyone holding the remote could read them.

## Decision

1. **A group device is registered to the group at pairing.** The pairing admin chooses
   *personal* or *group*; for a group device the owner set is the group key and the device
   is listed in the membership document. All members may use its device-scoped capabilities
   under the group policy. There is no individual owner to transfer from; an admin can
   re-register it.
2. **A request carries the authority of the person authenticated behind it.** A group
   device with nobody signed in acts with the **group's authority only**, which never
   includes any member's user-scoped capabilities. Such a device cannot read a member's
   clipboard, notifications or files, cannot receive shares addressed to a person, and
   cannot use a member's telephony. It may use other group devices' device-scoped
   capabilities under policy, and it may ask a member's device for a device-scoped
   capability only through the ordinary present-person approval, labelled as coming from
   an unauthenticated group device.
3. **Data reaches a group device by push.** A member who wants their photos on the
   television, their phone cast to the tablet or their playlist on the speaker sends from
   their own device to the group device's device-scoped capability. The person's device is
   the actor; the group device only provides.
4. **A member may open a person session on a group device**, which is a binding paired as
   usual. Such a binding is **never an introducer**, so a group device can never pair
   devices into anyone's set. While the session is present and unlocked the device acts
   with that person's authority, with reduced defaults: implicit access only to the
   **low-sensitivity tier** of their own user-scoped capabilities; the **elevated tier**
   needs per-use confirmation on another of the person's own devices. The session
   auto-locks after a short idle period, and on lock the device reverts to group authority.
5. **Capability families carry a sensitivity tier.** Low: clipboard, notification display,
   media control, displays, share receipt. Elevated: files, telephony and SMS, remote input
   into a personal device, sensors of a personal device, notification actions that reply or
   dismiss. A family's tier may be overridden per descriptor only upward.

## Consequences

Anyone holding the remote gets exactly what the family has decided the television may do,
and nothing of any member's. The push model fits how people already use shared screens.
Person sessions on shared screens are useful but second-class by design: short-lived,
confirmation-gated for anything sensitive, and unable to grow a device set. The sensitivity
tier is a new attribute on capability families and a new field in the schema. The
present-person approval prompt must show whether a request is authenticated, which the
transport already knows from the role chain.
