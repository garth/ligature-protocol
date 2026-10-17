# Ligature Protocol (LP)

> **Status: pre-alpha.** Nothing here is stable. The specification is a draft, the
> reference implementation is a scaffold, and the Lexicon namespace is a placeholder.

Ligature Protocol binds a person's devices (Linux desktops, Android phones, tablets,
home hubs, and iPhones where the platform allows) into one trusted group, so any device
can discover and use the capabilities of the others. Think of it as an open,
Apple-Continuity-like ecosystem for Linux users, built on existing open standards.

## Principles

- **Local-first.** Devices talk to each other directly. Everything works on a LAN with no
  internet connection.
- **Self-hostable, no required central server.** The only hosted component is an
  AT Protocol Personal Data Server (PDS), and you can run your own.
- **End-to-end encrypted.** All device-to-device traffic is protected by an MLS group
  (RFC 9420). Secrets at rest are stored only as MLS-encrypted payloads.
- **Build on open standards.** AT Protocol for identity and shared state, MLS for group
  encryption, mDNS/DNS-SD and BLE for discovery, WireGuard for the remote overlay.
- **Interoperate, don't replace.** Bridge to KDE Connect, LocalSend, Matter/Thread,
  CalDAV/CardDAV and Syncthing where practical.

## Architecture

```mermaid
flowchart TB
    subgraph Identity["Identity layer"]
        DID["AT Protocol DID<br/>(did:plc / did:web) + DNS handle"]
    end

    subgraph Trust["Device trust layer"]
        XS["Per-device keys,<br/>cross-signed by the user identity"]
    end

    subgraph Crypto["Encryption layer"]
        MLS["MLS group (RFC 9420)<br/>one group per user's device set"]
    end

    subgraph Transport["Transport layer"]
        LAN["Local: mDNS/DNS-SD + BLE discovery,<br/>direct connections"]
        WAN["Remote: WireGuard overlay<br/>(Headscale-compatible)"]
    end

    subgraph Caps["Capability layer"]
        ADV["Advertise typed capabilities"]
        GRANT["Discover, request, grant"]
        USE["Notifications, clipboard, files,<br/>remote input, media, telephony,<br/>displays, sensors"]
    end

    subgraph State["Shared state (atproto Space on a PDS)"]
        REG["Device registry, settings,<br/>household permissions as Lexicon records"]
        SEC["Secrets: MLS-encrypted payloads only"]
    end

    DID --> XS --> MLS --> LAN & WAN --> ADV --> GRANT --> USE
    XS -. "publishes" .-> REG
    MLS -. "encrypts" .-> SEC
    REG -. "bootstraps" .-> MLS
```

Real-time traffic never routes through the PDS. The PDS holds the device registry and
other shared state so that devices can find and verify each other; the data itself
flows directly between devices.

## Repository layout

| Path | Contents |
| --- | --- |
| `docs/spec/` | The protocol specification (draft) |
| `docs/adr/` | Architecture decision records |
| `docs/glossary.md`, `docs/prior-art.md` | Terminology and related work |
| `lexicons/` | Draft AT Protocol Lexicons (`org.ligatureprotocol.*`, placeholder namespace) |
| `impl/` | Rust reference implementation (cargo workspace) |

## Reference implementation

The `impl/` workspace contains:

- `lp-core`: shared types and the capability model
- `lp-identity`: DID resolution and device key cross-signing
- `lp-crypto`: MLS wrapper (planned on OpenMLS)
- `lp-transport`: mDNS/BLE discovery and connections
- `ligatured`: Linux user daemon (D-Bus, systemd user unit)
- `lpctl`: CLI for pairing and inspection

```sh
cd impl
cargo build
cargo test
```

## Documentation

- [Specification overview](docs/spec/00-overview.md)
- [Architecture decisions](docs/adr/)
- [Glossary](docs/glossary.md)
- [Prior art](docs/prior-art.md)
- [Contributing](CONTRIBUTING.md)
- [Security policy](SECURITY.md)

## License

Code is licensed under [Apache-2.0](LICENSE). The specification text under `docs/` is
licensed under [CC-BY-4.0](LICENSE-SPEC).
