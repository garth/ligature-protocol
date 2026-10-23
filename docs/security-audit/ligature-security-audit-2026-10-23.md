# Ligature Protocol security audit

**Date:** 2026-10-23 **Branch:** main **Commit:** 9285d0b (the v0.1.0-alpha release plus the
LP-117, LP-107, LP-118 and LP-85 commits after it) **Scope:** the specification, the schemas,
the Rust workspace and the Android app, against the threat model in chapter 09 and the
fifteen-item checklist of the `security-audit` skill. First audit; there is no baseline.

The project is alpha and had not been audited before this report. Every finding is new.
Findings at Critical or High are listed here in outline only, with the detail on a private
ticket, as `SECURITY.md` requires. All three were fixed on the day of the audit, before this
report was committed, under LP-120, LP-121 and LP-122; the commits named by those keys carry
the changes and their tests.

## Summary

The cryptographic core holds up: TLS 1.3 with raw public keys in both directions, Ed25519
only, no resumption or 0-RTT, the frame length bound checked before allocation, CBOR decoded
from a bounded slice, signed facts verified byte for byte before decoding with every signer
bound to the person's key chain, SQL through bound parameters, received file names reduced to
a safe base name, the control socket private to the user, no private material in the
repository and no key material in logs.

The failures are all one layer up, in which facts and which peers the daemon acts on:

1. A revocation is honoured on the strength of its subject alone, whoever signed it, so any
   self-signed revocation naming one of a person's keys revokes that key everywhere, for
   good. The verifier that would stop it exists and is called only by tests. (High)
2. The gossip and manifest streams have no trust check, and a peer the daemon has not
   recognised reaches them: for as long as any ceremony runs, for as long as a fresh daemon
   is uninitialised, and in a race window on every connection. Such a peer reads the whole
   device set and writes facts into the store, which is the route for finding 1. (High)
3. A revocation learnt from gossip does not end the live session with the revoked device,
   which keeps receiving the clipboard and notifications until the connection drops. (High)

Below those, a run of Medium findings: the grants file handed to a newcomer at pairing, a
clipboard fetch that trusts the size a peer gives at fetch time rather than the announced
one, notification bodies sent to the Linux notification daemon without markup escaping, a
release pipeline that ships unsigned artefacts and silently falls back to a throwaway APK
key, an Android share activity any app can drive, a flood of facts that breaks gossip for
good, a pairing race for whoever holds the offer text, and rejections that are logged at
`debug` or not at all.

| Risk | Count | Of which resolved or reported privately |
| --- | --- | --- |
| Critical | 0 | |
| High | 3 | 3, fixed the same day (LP-120, LP-121, LP-122) |
| Medium | 11 | 0 |
| Low | 21 | 0 |
| Info | 8 | |

## Trend since the previous audit

No previous audit. Everything is New.

## Findings by checklist item

Evidence cites `impl/crates/…` paths relative to `impl/`, and the Android app relative to
`impl/android/app/src/main/`. Confidence is 1 to 10; only 7 and above are reported.

### 1 Trust and authentication

**Status:** PARTIAL (FAIL at the audit; the three High findings fixed the same day)
**Risk:** Medium remaining **Change:** New

**Passes.** The introducer dials the offer's addresses and keeps only a connection whose key
is the offered binding (`ligatured/src/ceremony.rs:366-373`, checked again before the hello
at `:140-142`); the offer carries a 64-bit one-time code checked on hello
(`lp-identity/src/pairing.rs:316-318, 394-396`); the six-digit code is the top twenty bits of
32 bytes exported under `EXPORTER-ligatureprotocol-pairing-sas` with the two binding keys as
context (`pairing.rs:34-36, 201-221`), as ADR 0029 says; both sides confirm before anything is
granted (`pairing.rs:439-441, 656`); a grant must name the newcomer's own fresh keys
(`pairing.rs:446-457`); roles are accepted only when signed by the required subordinate key
of a verified person fact (`lp-identity/src/facts.rs:132-161, 215-238`); a revoked key is
refused at recognition (`lp-identity/src/trust.rs:86-88`), dropped on connect
(`ligatured/src/run.rs:144-150`), skipped by dial and redial (`run.rs:293-300, 329`) and
refused per message by every capability; gossip sends revocations first
(`lp-identity/src/gossip.rs:46-76`); the revoking device drops the revoked peer and pushes
gossip (`ligatured/src/control.rs:504-517`).

**1-A. A revocation is honoured whoever signed it.** High, confidence 9. Fixed under LP-120:
a revocation counts only when it verifies against the person it names and the subject is
that person's own key or device, or the revoker is the owner. In short: the store honours any
non-quarantined revocation by subject (`lp-identity/src/trust_store.rs:257-266`) after
checking only that the envelope's own signer signed it (`trust_store.rs:149-151`), and
`facts::verify_revocation` (`facts.rs:302-319`), which binds the signer to the person's
self-signing key, has no caller outside tests. Spec chapter 06 section 5 requires the chain.

**1-B. Unrecognised peers reach the gossip and manifest streams.** High, confidence 9.
Fixed under LP-121: the transport holds every stream but the pairing offer's until the
daemon admits the peer, strangers kept for a ceremony are dropped when it ends, and the
gossip handler checks recognition itself. In short: the gossip handler
(`ligatured/src/gossip.rs:78-88`) and the manifest handler (`capabilities.rs:171-189`) check
nothing about the peer; streams are dispatched by the transport before the daemon's
recognition runs (`lp-transport/src/manager.rs:198-206` spawns the stream loop, then sends
`Connected`; recognition is in `run.rs:137-150`); strangers are kept while uninitialised or
while any ceremony runs (`daemon.rs:513-521`) and nothing drops them when the ceremony ends
(the only `drop_peer` calls are `run.rs:149, 299`, `ceremony.rs:372`, `control.rs:504`).
`exchange` then sends every fact id and every fact but grants, and stores every self-signed
fact it is sent. This is also the route by which 1-A reaches a device from the LAN, and it
contradicts chapter 09 ("a stranger gets nothing but the pairing offer").

**1-C. A revocation learnt from gossip does not end the live session.** High, confidence 8.
Fixed under LP-122: every gossip exchange and every revocation re-checks the connected
peers and closes the session of any no longer trusted. In short: only the revoking device drops the
peer; a device that learns the revocation by gossip keeps the connection and the senders
registered at attach (`clipboard_sync.rs:210-215`, `notification_sync.rs:143-148`), and
`send_to_all` and `displaying_peers` never re-check recognition, so a revoked device that
stays connected keeps receiving the clipboard and notifications until the session drops.
Chapter 06 section 5 says a revoked device gains nothing from sessions it held.

**1-D. Whoever holds the offer text can race to become the introducer.** Medium, confidence 7.
The newcomer takes the first `pairing` stream that arrives (`ceremony.rs:314-318`); a
holder of the offer (QR, printed text, the phone's Copy and Share actions) who dials first
passes the hello, the real introducer's stream queues in a channel of one and waits with no
timeout (`daemon.rs:497`, `ceremony.rs:144`), and the newcomer shows a six-digit code that
matches the attacker's session. The newcomer is never shown the name or key of the person it
is about to join (`ceremony.rs:244-253`), so the only signal is the real introducer not
showing a code. `lpctl pair --yes` skips the comparison (`lpctl/src/main.rs:672`).
Recommendation: show the introducer's person name and key fingerprint on the newcomer before
it confirms; time the introducer's wait out; refuse `--yes` for pairing; mark the copied
offer sensitive on Android (`EXTRA_IS_SENSITIVE`, `MainActivity.kt:97-100`); read the offer
from standard input rather than a command-line argument.

**1-E. A revoked introducer keeps the self-signing key.** Medium, confidence 8. Every new
binding is made an introducer by default (`ceremony.rs:432`, `lpctl/src/main.rs:63-65`) and
receives the self-signing seed (`pairing.rs:672-674`); `revoke` revokes one subject key and
rotates nothing (`control.rs:491-506`). A revoked device that kept its copy can sign a role
for a fresh key and rejoin, or sign a person fact with a huge version that wins for ever
(`trust.rs:65-69`). Chapter 09 and chapter 07 record the leaked self-signing key as open, so
this is a design gap rather than a bug; recorded under accepted risk below with what would
close it. Recommendation: make introducer opt-in; warn on revoking an introducer; plan
self-signing rotation; require the master key for person-fact updates or bound the version.

**1-F. Revocation is per key, not per device.** Low, confidence 7. Revoking a binding leaves
its principal recognised and vice versa (`trust.rs:106`); `introduce` accepts an offer whose
binding is already revoked and appears to succeed. Recommendation: revoke both keys of a
device together; refuse to introduce a revoked key.

**1-G. Small gaps.** Info. The short code has a modulo bias (2^20 mod 10^6,
`pairing.rs:219-220`); the word-list form and the staleness period of chapter 06 section 5
are not implemented; abandonment has no daemon path (`RevocationKind::Abandonment` is made
only in tests).

### 2 Signatures and facts

**Status:** PARTIAL **Risk:** High (through 1-A) **Change:** New

**Passes.** The signed string is `"ligature/fact/1" || type || payload` over the bytes as
received (`lp-core/src/envelope.rs:19, 38-52`); every `verify_*` verifies the signature before
decoding (`facts.rs:133-134, 216-217, 263-264, 306-307`) and `decode` checks the type name
(`:76-86`); payloads are stored and forwarded byte-exact (`trust_store.rs:153-167`,
`lp-identity/src/gossip.rs:77-80`); a person fact must be signed by its own self-signing key
with both subordinate keys endorsed by the master (`facts.rs:135-161`); the highest person
version wins (`trust.rs:65-69`); revocations are permanent regardless of timestamp; the store
quarantines a fact that fails at open and never acts on or gossips a quarantined row
(`trust_store.rs:201-266, 290-315`); unknown fact types are refused at insert (`:382`); the
vectors cover a tampered payload, a relabelled type, outsider-signed person and role facts,
the wrong subordinate key, a swapped subkey purpose, an oversize frame and a truncated frame.

**2-A.** The revocation chain, see 1-A.

**2-B. A revoked signer's earlier facts stay valid.** Low, confidence 8. Revocation is
checked against the subject only (`trust.rs:62, 86, 115`), never a role's signer, and roles
do not record which device introduced them; all introducers share the self-signing key, so
this follows from the design. Recommendation: document it; record the introducing binding in
the role fact if accountability per device is wanted.

**2-C. A quarantined fact never heals.** Low, confidence 8. `gossip::apply` skips any id
already present (`lp-identity/src/gossip.rs:90-93`) and `INSERT OR IGNORE` never clears the
flag, so a row corrupted on disk cannot be repaired by receiving the fact again.
Recommendation: replace a quarantined row when a valid copy arrives.

**2-D. Primitive strictness.** Info. Signatures are checked with `verify`, not
`verify_strict` (`lp-crypto/src/signing.rs:119-124`), so small-order keys pass the
signature check; the chain checks pin every signer, so the impact is nil except through 1-A.
The signed string has no length separator between type and payload; safe while no type
name is a prefix of another. Vector gaps: outsider-signed revocation through the daemon's
own path, a fact naming another person, version rollback, a weak key, forgiving-decoding
variants, the exporter context.

### 3 Transport and sessions

**Status:** PARTIAL **Risk:** Medium **Change:** New

**Passes.** TLS 1.3 only, raw public keys required and client authentication mandatory
(`lp-crypto/src/session.rs:126-139, 184-186, 233-239, 273-294`); a "certificate" must be
exactly a 44-byte Ed25519 SPKI with no intermediates (`:58-68, 113-119, 151-155`); suites
are rustls ring defaults; resumption, tickets and early data are off (`:281-282, 293-294`);
ALPN `ligature/1` on both sides (`:30, 280, 292`); the frame length is checked against 1 MiB
before allocation (`lp-transport/src/stream.rs:63-70`, `lp-core/src/frame.rs:58-64`), the
same on the control socket; ciborium decodes from the bounded slice with its nesting limit; a
bad header or unknown purpose resets that stream only (`manager.rs:215-232`); keep-alive 15 s
and idle 60 s (`quic.rs:34-51`); crossed dials settle the same on both ends and a redundant
discovery dial no longer replaces a live connection (`manager.rs:99-116, 174-199`,
`run.rs:264-283`, LP-117). Connection migration is on, as ADR 0028 intends.

**3-A.** Streams dispatched before recognition, see 1-B.

**3-B. A flood of facts breaks gossip for good.** Medium, confidence 7. No cap on facts
stored per exchange or per signer (`lp-identity/src/gossip.rs:89-96`); once the store holds
about thirty thousand facts the `have` list exceeds the 1 MiB frame and every exchange fails
at its first step (`ligatured/src/gossip.rs:35`), so revocations stop reaching or leaving
the device. Any trusted peer, and through 1-B any stranger, can do it. Recommendation:
accept facts only from trusted signers; cap facts per session and per signer; split or
summarise `have`; send revocations outside the `have`/`want` exchange.

**3-C. Known persons' devices get the full gossip.** Low, confidence 6, below the reporting
bar but recorded because the spec leaves it open (chapter 06 section 7): `is_trusted`
includes another person's binding (`trust.rs:46-51`) and `facts_for` does not filter by
recognition. No production code makes a `person` role yet.

**3-D. Header reads are serial, untimed and allocate the announced length.** Low, confidence
8. `stream_loop` awaits the header inline (`manager.rs:211-222`); a stream whose body never
arrives stalls later streams on that connection, and each header may announce 1 MiB which is
allocated at once (`stream.rs:69-70`). Recommendation: read headers in a task under a short
timeout with a small bound; grow body buffers as bytes arrive.

**3-E. quinn limits are defaults.** Low, confidence 7. Unidirectional streams are allowed
but never accepted, datagrams are enabled but unused, and the connection receive window is
unbounded (`quic.rs:45`). Recommendation: `max_concurrent_uni_streams(0)`, no datagram
buffer, a finite receive window, bidirectional streams sized for `files`.

**3-F. Dialling an offer can replace a live connection.** Low, confidence 7. `introduce`
dials each offer address through the manager, which replaces an existing connection to the
same key, and a wrong key at the address drops the peer by key (`ceremony.rs:356-373`,
`manager.rs:109-115`), the harm LP-117 removed from discovery. Recommendation: skip
addresses a live connection already answers; drop only the connection just created.

### 4 Discovery

**Status:** PARTIAL **Risk:** Medium (privacy) **Change:** New

**Passes.** The record carries `v` and a random per-process `id` only, instance and host
`lp-<id>` (`lp-transport/src/mdns.rs:296-327, 342-356`); nothing is advertised before
initialisation (`daemon.rs:210-212, 547`); `id` is used for filtering and back-off only
(`run.rs:220-245`), recognition always runs on the handshake key; a wrong key at the dialled
address is dropped and backed off (`run.rs:297-300`).

**4-A. The device's long-lived key is one handshake away from any bystander.** Medium,
confidence 9. The server presents its raw binding key to any client that completes a
handshake and accepts any client key (`session.rs:194-207, 277-279`); the endpoint binds
every interface (`run.rs:55-57`); the record gives the port. ADR 0037's "nothing stable
across restarts" holds for the record but not for what the port answers. Chapter 09 lists
the per-epoch identifier as open, so this is recorded under accepted risk with what would
close it. Recommendation: pursue the per-epoch identifier; consider proving the binding key
only after a recognition step inside the session.

**4-B. Records can make the daemon dial any address.** Low, confidence 8. The discovery loop
dials the first IPv4 address of any record with `v=1` and a fresh `id`, with no scope check
(`run.rs:216-241`), so an injected record sends a handshake, and with it the binding key, to
a public or loopback address. Recommendation: dial only addresses on a directly connected
subnet; ignore loopback, multicast and broadcast; try addresses in a sensible order.

**4-C. The fixed port.** Low, confidence 8. ADR 0037 says the port "is never fixed";
`LIGATURE_PORT` (LP-81, LP-85) fixes it and the units set 47471, a stable value for a
bystander to link across restarts. Recommendation: record the trade-off in ADR 0037 and in
the README's install note.

**4-D. Every interface address is advertised.** Info, confidence 5. `enable_addr_auto`
(`mdns.rs:354`) may publish MAC-derived IPv6 addresses. Not verified against mdns-sd.

**4-E. What the offer exposes.** Low, confidence 7. The offer holds both public keys, every
non-loopback IPv4 address of the host and the one-time code (`pairing.rs:56-81`,
`daemon.rs:556-571`); it is passed as a command-line argument on the desktops (visible in
process lists and shell history) and copied or shared from the phone without a sensitive
flag. See 1-D for what a holder can do.

### 5 Capabilities and authority

**Status:** PARTIAL **Risk:** Medium **Change:** New

**Passes.** Every capability message is refused before any effect unless the peer is a
binding of the same person: clipboard set and get (`clipboard_sync.rs:537-541, 715-723`),
notifications (`notification_sync.rs:262-266`), files push and fetch
(`files_sync.rs:1159-1163, 1213-1217`, data streams tied to an accepted transfer for that
peer at `:1339-1349`); the implicit grant refuses anything but `MyBinding`
(`grants.rs:315-322`); the registry is in memory and forgotten on disconnect
(`capabilities.rs:21-29, 128-137`); nothing dials or opens streams on a peer's say-so beyond
the bounded data streams of a fetch (`files_sync.rs:843-866`); the Android node goes
through the same code (`ligature-android/src/node.rs:217-347`).

**5-A. The grants file is handed to a newcomer at pairing.** Medium, confidence 9.
`Daemon::all_facts` (`daemon.rs:578-580`) does not apply `LOCAL_ONLY_TYPES`
(`lp-core/src/facts.rs:238`), and `ceremony.rs:353` puts the result in the grant's gossip;
ordinary gossip withholds grants (`lp-identity/src/gossip.rs:69`). Chapter 09 section 4 says
no off-device party learns grants. Recommendation: filter with `LOCAL_ONLY_TYPES` or reuse
`facts_for`.

**5-B. The manifest is not filtered by audience.** Low, confidence 9, latent. `announce_to`
sends the full manifest to every trusted peer (`daemon.rs:613`, `capabilities.rs:145`),
which includes another person's binding; no production code makes one yet. Chapter 05
section 2a requires filtering. Recommendation: build the manifest per peer from recognition.

**5-C. A peer can dismiss notifications it never received.** Low, confidence 8. A `Dismiss`
whose origin is this device calls `dismiss_own` with a peer-chosen id
(`notification_sync.rs:313-316`); on Linux any u32 closes that bus notification
(`notifications_linux.rs:191-198`), on Android any key is cancelled
(`NotificationListener.kt:122-125`). Recommendation: act only on ids this device emitted to
that peer.

### 6 Files on disk

**Status:** PARTIAL **Risk:** Low **Change:** New

**Passes.** `safe_name` keeps the last path component, drops control characters, trims,
strips leading dots, maps empty, `.` and `..` to `file`, and caps at 255 bytes
(`files.rs:414-437`); only that name is joined to the download directory (`:184-185`); the
part file is opened `create_new` so a planted symlink is not followed (`:189-193`) and
renamed only after the SHA-256 check (`files_sync.rs:1587-1592`); more or fewer bytes than
announced are refused (`:1408-1411, 1423-1426`); every failure removes the part file; the
cache lives under the runtime directory in a numbered directory per value, wiped at start
and on each new value (`clipboard_sync.rs:131, 341, 657-667, 766`); peers ask for 128-bit
random ids, never paths (`files_sync.rs:491-500, 1222-1230`); on Android the MediaStore row
stays pending until the digest matches and is deleted on abort (`Transfers.kt:66, 106-131`).

**6-A. A clipboard fetch trusts the fetch-time offer, not the announcement.** Medium,
confidence 8 (also item 8). The destination is chosen from the announced size
(`clipboard_sync.rs:759-771`), but `run_fetch` takes the size from the peer's reply offer
(`files_sync.rs:1079-1090`), the memory landing preallocates up to 64 MiB and then grows
without bound (`files.rs:375-389`), and the announced hash is compared only after landing.
A same-person peer can announce a kilobyte and deliver gigabytes into the daemon's memory,
on the desktops and on the phone. Recommendation: refuse an offer whose size or hash differs
from the announcement; cap the memory landing at the limit the caller chose.

**6-B. Putting a file in place can overwrite.** Low, confidence 7. `exists` follows
symlinks and `rename` replaces whatever is at the target (`files.rs:186, 315-336`), so a
dangling symlink or a same-user race breaks "never overwrite". Recommendation:
`RENAME_NOREPLACE` (or `RENAME_EXCL` on macOS), `symlink_metadata` for the check.

**6-C. Names of 242 to 255 bytes fail to land.** Low, confidence 8. The `.ligature-part`
suffix pushes the part name over the limit (`files.rs:27, 185, 426-436`). Recommendation:
cap below 255 minus the suffix, or name the part file randomly.

**6-D. The cache falls back to a shared temporary directory.** Low, confidence 6. Without
`XDG_RUNTIME_DIR` (or `HOME` on macOS) the cache is `/tmp/ligature/clipboard`
(`clipboard.rs:730`), created with default modes and no owner check. Recommendation: fall
back to the data directory; create 0700; refuse a directory owned by someone else.

**6-E. A peer's `text/uri-list` passes through when it has no files.** Low, confidence 6.
`clipboard_linux.rs:496-507` drops the peer's list only beside a file selection, so
`file:` URIs a peer chose can reach a file manager paste. Recommendation: never serve a
peer's `text/uri-list` bytes; build it from fetched cache paths only.

**6-F. Bidi and format characters survive sanitising; notifications show the raw name.**
Low, confidence 7. `is_control` covers category Cc only (`files.rs:420`) and the received
notification uses the offered name (`files_sync.rs:1511-1513`). Recommendation: strip Cf and
bidi controls; show the landed name.

**6-G. Android does not sanitise a received name.** Low, confidence 6. The descriptor's name
goes unchanged into MediaStore `DISPLAY_NAME` and the clipboard provider's `DISPLAY_NAME`
(`ligature-android/src/files.rs:51`, `Transfers.kt:63`, `ClipboardProvider.kt:46`);
MediaStore's own sanitising decides the outcome. Recommendation: apply `safe_name` there.

**6-H. Stale Android clipboard offers.** Low, confidence 5. A new clip with the same name and
size as the last is not re-sent, so old offers keep pointing at reused descriptors for the
window (`NodeHolder.kt:194-196`, `ligature-android/src/clipboard.rs:107-111`). Recommendation:
withdraw offers before closing handles.

**6-I. Pushes have no size limit or free-space check.** Info, by design for the same person
(`files_sync.rs:1164-1183`).

### 7 Local interfaces

**Status:** PARTIAL **Risk:** Medium **Change:** New

**Passes.** Runtime directory 0700, socket 0600, peer uid checked on accept and another uid
dropped with a warning (`control.rs:537-540, 553-593`); a stale socket is unlinked only
after a failed connect, inside the private directory (`:561-570`); the data directory is
0700 and the store 0600 (`trust_store.rs:70, 83, 399-409`); `keys.cbor` is written 0600
through a temporary file, fsync and rename (`key_file.rs:86-99`); the Wayland helper is this
executable spawned with fixed arguments and bounded frames (`clipboard_linux.rs:44, 255-264,
545-566`); on Android the service and provider are not exported, the provider serves only
keys the node holds read-only (`ClipboardProvider.kt:31-91`), the listener and tile are bound
by system permissions, backups are off and every pending intent is explicit and immutable.

**7-A. Any app can set every desktop's clipboard through the share activity.** Medium,
confidence 8. `ShareActivity` is exported for `SEND text/plain` (manifest `:53-72`) and sends
`EXTRA_TEXT` to every device with a toast and no confirmation (`ShareActivity.kt:25-38`,
`NodeHolder.kt:218-221`). A malicious app plants a paste on the desktops without a user
action, which the design otherwise requires. Recommendation: confirm with a preview before
sending from a share intent.

**7-B. The app opens whatever URI it is handed, including its own files.** Medium, confidence
6. `Transfers.open` opens any URI it is given, from the share sheet
(`ShareActivity.kt:41-79`) or from the clipboard, where only the app's own provider authority
is excluded (`NodeHolder.kt:348-370`). A `file:` URI naming the app's private key file would
be sent to a desktop's download folder as a file transfer. Whether `openFileDescriptor`
accepts a `file:` URI on the targeted versions was not run. Recommendation: accept `content:`
URIs only; reject this package's authorities and paths; show the names before sending.

**7-C. Device-to-device transfer can copy the keys.** Low, confidence 7. `allowBackup=false`
without `dataExtractionRules` does not stop device-to-device transfer on Android 12 and
later, so `keys.cbor` moves to a new phone and clones the device. Recommendation: add
`dataExtractionRules` excluding everything, or wrap the seeds with a non-exportable
Keystore key.

**7-D. The control socket's own uid comes from a file owner on macOS.** Low, confidence 8.
Without `/proc`, `same_user` compares the client's uid with the binary's owner
(`control.rs:595-604`), which fails closed for a root-owned binary. Recommendation:
`rustix::process::geteuid`.

**7-E. Smaller points.** Low. `bind` changes the runtime directory's mode without checking
owner or symlink (`control.rs:553-559`); the key file's temporary is not opened `create_new`
or `O_NOFOLLOW` and its mode is never checked on load (`key_file.rs:55-99`); the helper's
read of another client's clipboard has no size cap or deadline (`clipboard_linux.rs:636-668`);
`install.sh` substitutes `HOME` with `sed` (breaks on `|` or `&`); the systemd unit has no
hardening directives.

**7-F. Same-user control requests.** Info, accepted by ADR 0024: `pair`, `revoke`,
`clipboard.paste` and `files.get --to` to any path, `files.send` and `files.offer` from any
readable path are open to any process of the user.

### 8 Resource limits

**Status:** FAIL **Risk:** Medium **Change:** New

| Bound | Enforced on receive | Evidence |
| --- | --- | --- |
| Clipboard text size | Yes | `clipboard_sync.rs:542-548` |
| `maxBytes` push bound | Sender only | `build_for` only; receive bounded by the frame |
| 64 MiB kept per value | Sender only, parts only | `clipboard_sync.rs:446-455`; files announced whole |
| One-hour window | Partial | no timer for the value read at start (`:149-157`); a `get` after the window re-registers with no timer (`:459-472, 686-688`) |
| 16 MiB memory fetch | Bypassable | 6-A |
| `maxIconBytes` and icon hash | Yes | `notification_sync.rs:268-275` |
| Icon cache per session | None | `notification_sync.rs:276-283`; Linux icon files never removed (`notifications_linux.rs:507-524`) |
| Notifications per peer, id length | None | frame-bounded only; Linux `copies` map not cleared on detach |
| `maxChunkBytes` | Sender only | `files_sync.rs:1406-1419` |
| `maxStreams` | Sender only | `files_sync.rs:1350-1351`; quinn's default 100 is the real cap |
| Announced size vs bytes | Yes, against the offer | `files_sync.rs:1408-1411, 1423-1426`; see 6-A for which offer |
| Concurrent pushes | 16 per peer | `files_sync.rs:73, 1168-1176`; fetch requests uncapped |

**8-A.** The memory fetch bypass, see 6-A. Medium.

**8-B. macOS spawns a process and a thread per notification.** Medium, confidence 7.
`notifications_macos.rs:97-110` runs `osascript` in a new thread for every `show` with no
queue or limit, so a same-person peer streaming posts exhausts the process table.
Recommendation: one worker with a bounded queue and a per-peer rate.

**8-C. A peer that stops reading stalls publishing for everyone.** Low, confidence 6.
`Registry::publish` awaits each peer's channel in turn under the `publishing` lock
(`capabilities.rs:58, 88-95`), as do `send_to_all` and `local_posted`. Recommendation:
`try_send` with replacement, or a send timeout that drops the peer.

**8-D. The icon cache and the Linux icon files are unbounded.** Low, confidence 8.
Recommendation: cap icons per peer; remove written icons on detach and at start.

**8-E. The clipboard cache has no quota and the Linux cache is RAM-backed.** Low, confidence
8. A fetch to the cache is sized by the peer's declaration and the runtime directory is
tmpfs. Recommendation: cap cache bytes per value; check free space.

### 9 Secrets and material

**Status:** PARTIAL **Risk:** Medium **Change:** New

**Passes.** No private material in tracked files beyond `vectors/keys.json`, labelled test
material (`vectors/README.md:36`); no keystore, PEM or env file in the history; workflow
secrets used only through `${{ secrets.* }}` and piped, never echoed
(`.github/workflows/release.yml:139-154`); the Android signing config reads the environment
only (`build.gradle.kts:31, 50-57`); signing keys and seeds print only their public part or
length in `Debug` (`lp-crypto/src/signing.rs:107-111`, `lp-core/src/wire.rs:60-64`); no
`info` or `warn` line logs clipboard text, notification content or file contents.

**9-A. The release falls back to a throwaway APK signing key.** Medium, confidence 9.
Without `ANDROID_KEYSTORE_BASE64` the job generates a key, warns, and still publishes the APK
(`release.yml:143-147, 172-180`); the password secrets fall back to `android`. Users install
an APK whose signer changes every run and cannot be updated. Recommendation: fail the job on a
tag build when the secret is absent; drop the password fallbacks.

**9-B. File names and paths logged at `info`.** Low, confidence 9. Offered and landed file
names and the landed path (`files_sync.rs:504, 1362, 1594`), the download directory
(`run.rs:107`), the person's display name at init (`daemon.rs:313`) and the data directory
(`main.rs:104-108`) go to the journal, the Mac's log file and logcat at the services' default
level. Recommendation: ids and sizes at `info`, names and paths at `debug`.

### 10 Dependencies and build

**Status:** PARTIAL **Risk:** Medium **Change:** New

**Passes.** `cargo deny check` is clean at this commit (advisories, bans, licences, sources);
the one advisory ignore carries its reason (`impl/deny.toml:9-12`), yanked crates are
denied, wildcards banned, unknown registries and git sources denied; `Cargo.lock` is tracked;
`unsafe_code = "forbid"` workspace-wide with the two documented exceptions, and `unsafe`
appears only in `lp-macos` (six sites, four with a SAFETY comment); clippy runs with
`-D warnings`; every action in both workflows is on a major that runs on the current Node.

**10-A. Release assets carry no checksum or signature, and the installer strips quarantine.**
Medium, confidence 9. `package.sh` writes only the tarball, the workflow uploads assets as
they are, `install.sh` verifies nothing and on macOS removes `com.apple.quarantine` from
binaries that are ad hoc signed only (`packaging/install.sh:75-77`, `release.yml:101`), so
Gatekeeper never sees them; chapter 08 section 2b says distribution is notarised.
Recommendation: publish `SHA256SUMS` with a signature or build-provenance attestations;
have `install.sh` check them; remove quarantine only after verification.

**10-B. The release token is writable in the build jobs.** Medium, confidence 7.
`permissions: contents: write` applies to every job of `release.yml:18-19`, checkouts keep
credentials, and the build jobs run third-party build scripts and proc macros; `ci.yml` has
no `permissions` block; actions are pinned to major tags and `cargo-ndk` is installed
unpinned. Recommendation: `contents: read` at workflow level with `write` on the release job
only; `persist-credentials: false`; pin actions by commit; pin `cargo-ndk`.

**10-C. The `unsafe_code` overrides cover whole crates.** Low, confidence 8.
`ligature-android` allows `unsafe` crate-wide for generated code it does not itself contain.
Recommendation: `deny` with a scoped allow; add the two missing SAFETY comments in
`lp-macos/src/pasteboard.rs:83-86`.

### 11 Threat model coverage

**Status:** PARTIAL **Risk:** High (through 1-A, 1-B, 1-C) **Change:** New

| Chapter 09 goal | Status | Evidence or finding |
| --- | --- | --- |
| Confidentiality and integrity against the network | Met on the LAN | `session.rs` as under item 3; helpers unimplemented |
| No off-device party learns the device set, grants or household | Not met | 1-B (gossip to strangers), 5-A (grants to a newcomer) |
| Least exposure | Met | capabilities only to the same person's bindings (item 5) |
| Authenticity of device membership | Not met | positive membership verified; revocation unauthenticated (1-A) |
| Forward secrecy, post-compromise security | Met | TLS 1.3 ECDHE, no tickets or resumption |
| Leaked self-signing key | Open in the spec | 1-E |
| Nothing sent before mutual authentication | Partly | TLS mutual authentication holds; recognition is after dispatch (1-B) |
| Discovery announces nothing identifying | Met for the record | 4-A for what the port then answers |
| Capability content only on the session, in memory | Met | the cache and downloads as the spec allows; 6-D |
| Files to the chosen folder, base name only | Met | `files.rs:414-437`; 6-B, 6-G |
| Offers served by announced id only | Met | `files_sync.rs:1222-1235` |
| Shared devices, groups, helpers, discovery records, relay denial of service, no remote trust beyond the ceremony | Unimplemented layers | `discovery.rs`, `relay.rs`, `ble.rs` are stubs; no `person` role is ever made |
| Open question: should a compromised device revoke all others? | Worse than the question | any introducer can, with no quorum; and through 1-A anyone can |

Spec claims the code does not implement: the principal held by a system service (chapter
02 section 2, chapter 08 section 2), the headless pairing page, group registration, the
Keychain on macOS (chapter 08 section 2b: keys are in `keys.cbor`), notarised distribution
(10-A), the staleness period and word-list code (chapter 06 section 5), abandonment and
rebuild (chapter 07).

### 12 Injection and parsing

**Status:** PARTIAL **Risk:** Low **Change:** New

**Passes.** Every SQL statement binds its values (`trust_store.rs:154-157, 178, 238-241,
259-261, 308-310`); the one `format!` near SQL adds a literal clause inside a private helper
whose callers pass literals; CBOR is decoded from a bounded slice and errors never panic;
no `unwrap`, `expect` or unguarded index on peer data in the daemon, transport, core or
identity crates; casts from peer values are clamped; the control channel is bounded CBOR,
not JSON; no schema sets `additionalProperties: false`; AppleScript strings are escaped for
backslash and quote with control characters dropped (`notifications_macos.rs:42-72`), and no
shell is involved.

**12-A. Peer-supplied names reach the terminal and notifications unsanitised.** Low,
confidence 8. Device and person names are decoded with no length or character check
(`facts.rs:262-271`; the schemas' `maxLength: 64` is not enforced), and `lpctl` prints them
raw (`lpctl/src/main.rs:191-242, 466-471, 535`), so escape sequences and newlines from a
same-person device reach the user's terminal. Recommendation: enforce the length and strip
control and bidi characters at decode and at `init`; escape non-printables in `lpctl`.

**12-B. The offer parser has no bounds.** Low, confidence 8. Any number of `a=` addresses of
any kind, no length cap (`pairing.rs:85-114`); the introducer dials each for 3 s in turn
(`ceremony.rs:356-377`), so a crafted offer makes it send handshakes to arbitrary hosts and
holds the ceremony, and with it the stranger window of 1-B, open. Recommendation: cap length
and address count; reject unspecified, loopback, multicast and broadcast addresses; dial in
parallel under one timeout.

**12-C. The peer's `app_id` becomes the `desktop-entry` hint.** Low, confidence 4, below the
bar, noted: `notifications_linux.rs:157`. Recommendation: omit the hint for peers' posts.

### 13 Content passed through

**Status:** FAIL **Risk:** Medium **Change:** New

**Passes.** No HTML parsing, URL opening or shell anywhere in the daemon or the app; macOS
clipboard types go through an allow list and `public.file-url` is answered only with a
fetched path (`clipboard_macos.rs:38-47, 151-153, 190-198`); Android shows notification text
with plain `setContentText` (`NodeHolder.kt:463-472`); icons are checked against the bound
and the announced hash before use (`notification_sync.rs:268-275`); the provider serves only
announced keys (`ClipboardProvider.kt:87-91`); file `text/uri-list` on Linux is built from
cache paths (`clipboard_linux.rs:341-358`).

**13-A. Linux notification bodies are sent without markup escaping.** Medium, confidence 8.
`notifications_linux.rs:155-169` passes the title and body raw to `Notify`, with no
`GetCapabilities` check; the freedesktop spec says a server with `body-markup` interprets
`<b>`, `<a href>` and `<img src>` and that clients must escape `&`, `<` and `>`. Anyone who
can get text into a phone notification (a message from a stranger) reaches the desktop
daemon's renderer through the forwarded body; a received file's name in `notify_received`
(`files_sync.rs:1516-1528`) takes the same path. Recommendation: escape `& < > " '` in title
and body before `Notify`.

**13-B. The Wayland helper serves any MIME a peer sends.** Low, confidence 8. `apply` does
not filter parts against this device's `mimeTypes` and the helper offers every type
verbatim (`clipboard_linux.rs:816-841`); with 6-E a peer can place `file:` URIs or
`x-special/gnome-copied-files` on the desktop clipboard. Recommendation: drop parts whose
MIME this device did not advertise.

**13-C. Received files open on Android with the MIME the peer declared.** Info. A tap on the
received notification fires `ACTION_VIEW` with the descriptor's MIME
(`Transfers.kt:64, 257-266`); the system still asks before installing a package.

### 14 Security logging

**Status:** FAIL **Risk:** Medium **Change:** New

**Passes.** A stranger kept or dropped is logged at `info` with its key (`run.rs:146-148`); a
capability message from a device that is not the person's is logged at `warn` with the key
(`clipboard_sync.rs:710`, `notification_sync.rs:390`, `files_sync.rs:1160, 1214`); a
discarded gossip fact at `warn` with the reason (`gossip.rs:57`); a declined fetch and a
promised-part hash mismatch at `info` (`files_sync.rs:1232`, `clipboard_sync.rs:644`); a
control connection from another user at `warn` (`control.rs:538`). No line logs content.

**14-A. Security-relevant rejections are silent or `debug` only.** Medium, confidence 9.
Silent: a received file's hash or size mismatch (`files_sync.rs:1408-1410, 1423-1425,
1588-1591`), a refused or busy files offer (`:1164-1175`), every ceremony failure (wrong
code, rejected comparison, bad records, wrong key at an offer address,
`ceremony.rs:73-76, 127-130, 370-374`). Only `debug`: unknown stream purpose and invalid
header (`manager.rs:218, 229`), a stranger we dialled (`run.rs:298`), a clipboard `get` from
a stranger (`clipboard_sync.rs:721`), a data stream for an unaccepted transfer
(`files_sync.rs:1341, 1346`). The discarded-fact line lacks the peer key and can be flooded.
Recommendation: `warn` with the peer key and reason for each, rate-limited per peer; add the
key to `gossip.rs:57` and `pairing.rs:268` and the uid to `control.rs:538`.

**14-B. Peer strings logged unescaped.** Low, confidence 6. `purpose` and fact `kind` are
logged with `%` (`manager.rs:229`, `gossip.rs:57`). Recommendation: log with `?` and truncate.

### 15 OWASP Top 10 cross-check

| Category | Where it lands | Result |
| --- | --- | --- |
| A01 Broken access control | item 5, item 1 | capability authority met; stream admission (1-B) and revocation (1-A, 1-C) not |
| A02 Cryptographic failures | items 2, 3 | met; 2-D strictness noted |
| A03 Injection | item 12 | met for SQL and CBOR; 12-A terminal escapes |
| A04 Insecure design | item 11 | 1-E, 4-A and the quorum question are the spec's open items |
| A05 Security misconfiguration | items 7, 10 | 7-C, 10-B, the fixed port (4-C) |
| A06 Vulnerable components | item 10 | clean at this commit |
| A07 Authentication failures | item 1 | the ceremony holds; 1-D race |
| A08 Integrity failures | items 2, 10 | facts met; release artefacts unsigned (10-A), throwaway APK key (9-A) |
| A09 Logging failures | item 14 | 14-A |
| A10 Server-side request forgery | none | the daemon dials what mDNS or an offer names (4-B, 12-B), never a URL a peer supplies; recorded as not applicable |

## Other observations

- `ShareActivity` and the clipboard send path forward whatever the user, or an app on the
  phone, hands them (7-A, 7-B); the notification listener forwards every app's
  notifications including one-time codes, with no sensitivity filter
  (`NotificationListener.kt:35-70`), which the design accepts between a person's own devices.
- Discovery dials one record at a time with a 5 s timeout and the endpoint accepts handshakes
  inline (`quic.rs:144-155`); both are denial-of-service shapes by an unauthenticated
  stranger and are out of scope by the exclusions, noted for the relay work.
- A stranger's wrong-code pairing stream aborts a waiting `pair join`
  (`ceremony.rs:318`, `pairing.rs:394-395`); recommendation: reset that stream and keep
  waiting.

## Critical findings

| Item | Status | Risk | Finding |
| --- | --- | --- | --- |
| 1, 2 | Fixed, LP-120 | High | A revocation is honoured whoever signed it (1-A) |
| 1, 3, 5 | Fixed, LP-121 | High | Unrecognised peers reach the gossip and manifest streams (1-B) |
| 1 | Fixed, LP-122 | High | A revocation learnt from gossip does not end the live session (1-C) |
| 5 | Open | Medium | The grants file is handed to a newcomer at pairing (5-A) |
| 6, 8 | Open | Medium | A clipboard fetch trusts the fetch-time offer (6-A) |
| 13 | Open | Medium | Linux notification bodies sent without markup escaping (13-A) |
| 9, 10 | Open | Medium | Throwaway APK key, unsigned assets, quarantine stripped (9-A, 10-A, 10-B) |
| 7 | Open | Medium | Any app drives the share activity; any URI is opened (7-A, 7-B) |
| 3 | Open | Medium | A flood of facts breaks gossip for good (3-B) |
| 1 | Open | Medium | The offer holder's pairing race (1-D) |
| 14 | Open | Medium | Rejections silent or `debug` only (14-A) |
| 8 | Open | Medium | A process per notification on macOS (8-B) |

## Deferred or accepted risk

| Item | Risk | Deferred since | Reason | What would unblock it |
| --- | --- | --- | --- | --- |
| 1-E | Medium | 2026-10-23 | Every introducer holds the self-signing key by design (ADR 0031); chapter 07 and chapter 09 record the leaked key as open | Self-signing rotation, or introducer as opt-in with a rebuild on revoking one |
| 4-A | Medium | 2026-10-23 | The raw binding key is the TLS identity (ADR 0028); chapter 09 lists the per-epoch identifier as open | The per-epoch identifier, or proving the key after recognition |
| 7-F | Info | 2026-10-23 | Device security is out of scope (ADR 0024) | nothing; recorded |
| 6-I | Info | 2026-10-23 | Pushes between a person's own devices are accepted by design (milestone 4) | A free-space check if wanted |

## Recommendations in priority order

1. Release the three High fixes (LP-120, LP-121, LP-122) in the next alpha; each carries a
   test (an outsider's revocation does not revoke; a stranger gets no gossip and is dropped
   when the ceremony ends; a device learning a revocation closes the session).
2. Medium, protocol and daemon: filter grants from the pairing gossip (5-A); refuse a
   fetch-time offer that differs from the announcement and cap the memory landing (6-A);
   cap facts per exchange and per signer and keep `have` within a frame (3-B); show the
   introducer's name and key on the newcomer, time the introducer out, refuse `--yes` for
   pairing (1-D); log every rejection at `warn` with the peer (14-A).
3. Medium, platforms: escape notification markup on Linux (13-A); a bounded notification
   worker on macOS (8-B); confirm share-intent sends and accept `content:` URIs only on
   Android (7-A, 7-B).
4. Medium, release: fail the APK job without the keystore secret, sign or attest the assets
   and verify in `install.sh`, read-only tokens in the build jobs (9-A, 10-A, 10-B).
5. Low: the rest of items 2, 3, 4, 6, 7, 8 and 12 as listed, in whatever order the
   milestones touch that code.
