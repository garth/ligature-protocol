# Milestone 4 implementation decisions

Choices that are not part of the protocol but that moving files needs settled. Each is
recorded under the ticket that made it; the protocol-level choices are in
[04-files.md](04-files.md) section 3.

## LP-102: the daemon core, `lpctl` and the desktops

**One synchroniser per daemon.** `ligatured::files_sync::FilesSync` sits beside
`ClipboardSync` and `NotificationSync`: one stream handler for the capability type that
tells a control stream from a data stream by its first message (`offer` or `request`
against `begin`), the in-memory offer registry, the transfers in flight, and the per-peer
stream budget. It advertises `send` always and `receive` while the store says files can
land, re-checking every two seconds as the clipboard does for its availability.

**Transfers are watched, not awaited.** `push` and `fetch` return at once with a
`Transfer`: a `watch` channel of `TransferProgress` (the files with their bytes done and
state, and the outcome once it ends) plus a cancel flag. The control channel streams every
change as a `files.progress` event and the final state as the response, so `lpctl` shows a
live line and the Android app a progress notification from the same source.

**Sources and landings.** `FileSource` is a path or bytes in memory; milestone 5 adds its
clipboard selection as a variant. `FileStore` is where pushed files land by default
(`DirectoryStore` over the download folder on the desktops; MediaStore on Android, LP-103)
and hands out a `Landing` per file that writes, hashes as it goes, and either renames into
place or removes every trace. `Destination` on a fetch is the store, a path (a directory
lands under the name rule, a file path is refused if it exists) or memory. The landing
trait is synchronous and driven from a blocking task fed by a bounded channel of four
chunks, so disk writes never hold a runtime thread and the channel's bound is the
backpressure onto QUIC.

**Hash before rename.** The sender hashes while reading and puts the digest in `end`; the
landing hashes while writing and the two are compared before the part file is renamed, so
a mismatch never leaves a file in the folder. A size over the descriptor is refused at the
first excess chunk, before the rest is read.

**The budget is a semaphore per peer**, sized to the smaller of the peer's `maxStreams`
and this device's own willingness (`send_streams`, 4), and resized when idle if the peer's
limit changed. The receiving side counts its open data streams and their peak for the tests
and does not refuse excess: the sender is the one bound by the spec, and a receiver that
also enforced would race the sender's release of a permit against its own decrement.

**Cancel.** A `Transfer` has one flag read between chunks on both ends. The sender resets
its data streams with code 2 and the control stream last; the receiver resets the control
stream and its data handlers reset theirs. A write to a stream the receiver stopped
surfaces the receiver's code (`lp_transport::stream::peer_stop_code`), so a file the
receiver cancelled is recorded as cancelled, not failed. A control stream that ends early
from the other side is treated as that side's cancel.

**Download folder.** `LIGATURE_DOWNLOAD_DIR` if set, for tests and the loopback run; on
Linux `XDG_DOWNLOAD_DIR` from `~/.config/user-dirs.dirs` (or `$XDG_CONFIG_HOME`), else
`~/Downloads`; on macOS `~/Downloads`. The part file is `<name>.ligature-part` beside the
destination, claimed with `create_new` so two files of one name landing at once get
different numbers, and the final name is re-checked at rename. The modification time is
applied with `File::set_modified`. A full disk shows as `std::io::ErrorKind::StorageFull`
and becomes the `no-space` reason; the advisory free-space check the plan mentioned is
left out, since the standard library has no portable way to ask and the write reports it
anyway.

**No free-space check before accept.** As above: a receiver accepts and reports `no-space`
on the file that hits it. `too-large` is reserved for a store that knows its own ceiling,
which MediaStore may.

**The received-file notification** goes through the attached `Notifications` backend's
`show`, as a copy on behalf of the sending device with the app id
`org.ligatureprotocol.files`, because that is the one path every platform already has and
it carries the origin hint on Linux, so the monitor never forwards it. One notification per
transfer, naming the file or the count and where it landed. Dismissing it sends a `dismiss`
for an id the sender never posted, which every backend ignores.

**`lpctl files`.** `send <device> <path>...`, `get <device> <id>... [--to]`, `offer`,
`offers`, `withdraw`, `cancel`, `transfers`. A device is its key, or its name or a prefix of
its key or fingerprint among the person's other bindings. Paths cross the socket absolute,
since the daemon's working directory is not the user's. Progress is one line on standard
error rewritten in place; Ctrl-C sends `files.cancel` on a second control connection, since
the first is waiting for the response. The exit status is non-zero unless every file was
received.

**`caps` shows the files directions** beside the notification ones.

**Message types in `lp-core`.** `lp_core::files` holds `FilesParams`, `FileDescriptor`,
`Reason` and `FilesMessage`, an internally tagged enum on `type` with `received` as a unit
variant; a chunk's bytes are a CBOR byte string through `wire::Bytes`. `lp-crypto` gains a
streaming SHA-256 (`Sha256Stream`) and `lp-transport` a stream priority call, a
receive-side stop, and the peer's stop code on a write error.

## LP-103: Android

**The library lands files through the host.** `ligature_android::HostFiles` is the
`FileStore` behind the node: `Host::create_download` inserts a pending row in MediaStore's
Downloads collection, opens a descriptor on it for writing and hands the node
`/proc/self/fd/N` as the path to write to; the node writes and hashes through that path as
on the desktops, then `finish_download` publishes the row (clearing `IS_PENDING`) and
returns the name the platform gave it and its content URI, or `abort_download` deletes it.
MediaStore numbers a clashing name itself, so the name rule's numbering is left to it on
this platform, and the modification time is not applied, since the platform owns that
column. Chunks therefore never cross the UniFFI boundary: only the row's metadata does.

**Sending reads the share's descriptor the same way.** `ShareActivity` takes `ACTION_SEND`
and `ACTION_SEND_MULTIPLE` for any type, opens each content URI for reading and hands the
node `/proc/self/fd/N` with the display name, the descriptor's size and the resolver's
type (`OutgoingFile`); content whose size the descriptor cannot report (a pipe) is copied
once to the app's cache to learn it. The descriptors, or the copies, are kept until the
transfer's last event and closed then. Text shared to the app still goes to every
device's clipboard as in milestone 2.

**One device per share.** Files go to one device the user picks from the connected
same-person devices that advertise `receive` (`Status::file_peers`, named from the device
list), in a plain dialog from the share sheet; there is no fan-out to several devices,
since a transfer is one pairwise session.

**Progress is a notification per transfer.** The node announces every transfer, either
direction, through `FilesSync::subscribe` and forwards its progress to the host as
`Event::TransferChanged` at most four times a second and always at its end
(`TransferInfo`: direction, peer name, bytes, files done, the active names, the outcome,
where received files landed). The app shows one notification per transfer on a
`transfers` channel with a progress bar and a **Cancel** action, which a broadcast
receiver turns into `Node::cancel_transfer`; at the end it becomes a received, sent,
cancelled or failed notification, the received one opening the first file through its
content URI.

**The phone takes one file at a time and sends up to four**: `maxStreams` 1 advertised,
since several files landing at once interleave on flash and multiply pending rows, and a
send budget of 4 towards a desktop that allows it.

**`Status` gains `file_peers`** beside `clipboard_peers`, and the one-second status poll
that tells the host about capability changes watches the files descriptor too.
