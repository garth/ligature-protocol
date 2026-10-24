# 0042. Releases are signed with a release key, and the update check belongs to the CLI

**Status:** Accepted
**Date:** 2026-10-24

## Context

A release is a set of files on the project's GitHub Releases page: a tarball per desktop
platform and an APK (LP-85). Nothing published with them ties them to the project, so a
person installing one, or a tool installing one for them, has only the transport's TLS to
go on, and no way to check a file after it has been downloaded, mirrored or handed on.
The owner asked for a one-command upgrade of the desktops (`lpctl upgrade`) and for the
CLI to say when a new version exists; both need something the CLI can verify against a key
it already holds. (Numbers 0040 and 0041 are reserved by the milestone 7 and 8 plans.)

The protocol's own trust is between persons and devices (ADR 0003, 0017, 0032): a key is
recognised because someone verified it in a ceremony. The maintainers' authority over what
code a device runs is a different root and must not be confused with it. Two of the
project's principles bear on the check for new versions: local-first, with no central
server the protocol depends on, and least exposure. A daemon that contacts a fixed server
on a timer is a dependency and a disclosure, even an innocuous one.

## Decision

1. **Every release carries a signed checksum file.** The release workflow writes
   `SHA256SUMS` over the release's files and signs it in the
   [minisign](https://jedisct1.github.io/minisign/) format (Ed25519) as
   `SHA256SUMS.minisig`, with the trusted comment `ligature-protocol v<version>`. The
   signing key is the project's **release key**, held by the maintainer and, as an
   unencrypted secret key, in the repository's CI secrets. A release built without it is
   signed with a key made for the run and is not the project's. The public key is
   published in the install guide and compiled into `lpctl`; it is not a protocol key,
   is never gossiped and never signs a fact.
2. **A key change is a new release and a new ADR.** A new release key is published the
   same way, and a `lpctl` from before the change cannot verify releases signed with it,
   so the first install after a key change is by hand; the ADR says why the key changed.
3. **`lpctl upgrade` verifies before it unpacks.** It fetches the release's checksum file
   and signature, verifies the signature against the compiled-in key, checks that the
   trusted comment names the version it asked for, checks the tarball's hash, and only
   then unpacks it and runs its `install.sh`. It never installs a version that is not
   newer than the one running, so a replayed older release is refused.
4. **The check for a new version belongs to the CLI, not the daemon.** `lpctl` asks the
   releases service at most once a day, when a person runs it on a terminal, and sends
   nothing but the request: no identity, no device key, no state. It is switched off by
   an environment variable. The daemon never contacts the service, and the protocol does
   not depend on it: a device whose owner never checks works as before.
5. **Android is covered by the checksum file alone.** The APK's own signature is
   Android's mechanism for updates; its hash in `SHA256SUMS` lets anyone verify a
   download by hand, and the app does not check for versions.

## Consequences

A person or a tool can verify any release offline against one short public key, and a
mirrored or forwarded tarball is as good as one from the releases page. The upgrade
command is one more place to get right: it is the single most valuable target in the
implementation, which is why it does nothing before the signature verifies and nothing on
the daemon's behalf. The maintainer holds one more secret, the release key, whose loss
means a key change under decision 2 and whose compromise means an advisory and a key
change. The CLI gains an HTTP client and a signature-verification crate, in `lpctl` only;
the daemon and the layer crates are unchanged. The releases service sees, once a day per
machine that runs `lpctl`, an address and a user agent, and nothing else.
