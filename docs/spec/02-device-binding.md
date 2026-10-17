# 02. Device Binding

**Status:** Draft.

## 1. Intent

Every device holds its own long-term signing key and a separate key for MLS. Devices are
trusted because the user identity has cross-signed them, following the Matrix
cross-signing model. See [ADR 0003](../adr/0003-device-cross-signing.md).

## 2. Key hierarchy

The user identity holds a master signing key. It signs a self-signing key (which signs the
user's own devices) and a user-signing key (reserved for trusting other identities, for
example household members). Open question: whether LP needs the user-signing key at all in
its first version.

## 3. Adding a device

A new device is added by approval from an existing trusted device. The new device shows a
QR code or a short code; the existing device scans or enters it, verifies the new device's
key, and publishes a signature. Open question: the exact out-of-band channel for the
bootstrap when the devices are not co-located (likely: not supported in v1).

## 4. Device records

Each trusted device has a record in the user's Space describing its keys, display name,
platform and current capabilities (see [`org.ligatureprotocol.device`](../../lexicons/org/ligatureprotocol/device.json)).

## 5. Revocation

Any trusted device can revoke another. Revocation removes the device record, removes the
device from the MLS group (chapter 03) and publishes a revocation so peers stop trusting
it. Open question: whether revocation should require more than one device when the device
set is large, to limit damage from a single compromised device.

## 6. Open questions

- How long may a device operate without re-verifying its signature chain against the Space?
- How are devices with no camera and no keyboard (hubs) paired?
