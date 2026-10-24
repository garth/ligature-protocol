---
hide:
  - navigation
  - toc
---

# Ligature Protocol

**Your devices, working as one.** What you copy, the notifications you get and the files
you share move between the devices you own, directly, over your own network: no account, no
server, no company in between. An open protocol, with apps for Linux, macOS and Android.

[Install the apps](guide/install.md){ .md-button .md-button--primary }
[Read the user guide](guide/index.md){ .md-button }
[Releases](https://github.com/garth/ligature-protocol/releases){ .md-button }

<div class="grid cards" markdown>

- :material-content-copy: **One clipboard**

    ---

    Copy on the laptop, paste on the phone. Text, formatted text, images and files; large
    content is fetched only when you paste it.

    [The clipboard](guide/clipboard.md)

- :material-bell-ring: **Notifications everywhere**

    ---

    A message that pops up on the phone shows on the desktop with the app's icon, and
    dismissing it anywhere clears it everywhere the platform allows.

    [Notifications](guide/notifications.md)

- :material-file-send: **Files between devices**

    ---

    Share from the phone or send from the command line; files land in the other device's
    download folder, with progress and a cancel at either end.

    [Files](guide/files.md)

- :material-key-chain: **Yours alone**

    ---

    You are a key your first device creates. Devices join by an in-person ceremony, every
    session is end-to-end encrypted, and nothing lives outside your devices.

    [First steps](guide/first-steps.md)

</div>

## How it works

Ligature binds a person's devices into one trusted group. A person is an Ed25519 key,
created on their first device; each device gets its own keys, signed in by the person's
introducer devices in a proximity ceremony with a QR code and a six-digit code. Devices find
each other on the local network and hold one QUIC connection per pair, with TLS 1.3 and
their own keys as the credentials, so the session itself is the end-to-end layer. Over it,
devices advertise capabilities (clipboard, notifications, files, and more to come) and use
each other's. There is no shared database: each device keeps its own signed facts and
gossips them to its peers, revocations first. The [specification](spec/00-overview.md) has
the whole design, and the
[repository](https://github.com/garth/ligature-protocol) holds the decisions behind it, the
plans for what comes next and the Rust implementation, which is both the reference and the
product.

## Status

Alpha. The releases carry pairing, the clipboard with text, HTML, images and files,
notifications, and file transfer, between a Linux machine, a Mac and an Android phone on
one network. Next come the phone's messages and calls and media control, then one keyboard
and mouse across the desktops, then a screen viewed and controlled from another device.
Remote connections, sharing between people and shared household devices are specified and
not yet built. Each release is reviewed against the project's threat model before it goes
out; the reports are in the
[repository](https://github.com/garth/ligature-protocol/tree/main/docs/security-audit).

## Sponsors

Organisations that support the project's development. Tiers are Gold, Silver and Bronze;
a tier appears here as soon as it has a sponsor. To sponsor Ligature, get in touch through
the [discussions](https://github.com/garth/ligature-protocol/discussions).

### Gold

[![NordStack](assets/sponsors/nordstack.svg#only-light){ width="300" }](https://nordstack.co.uk)
[![NordStack](assets/sponsors/nordstack-dark.svg#only-dark){ width="300" }](https://nordstack.co.uk)

[NordStack Ltd](https://nordstack.co.uk), an independent UK software firm.

<small>Specification and documentation
[CC-BY-4.0](https://github.com/garth/ligature-protocol/blob/main/LICENSE-SPEC), code
[Apache-2.0](https://github.com/garth/ligature-protocol/blob/main/LICENSE).</small>
