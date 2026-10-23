# Milestone 5 implementation decisions

Choices that are not part of the protocol but that a clipboard of several representations
needs settled. Each is recorded under the ticket that made it; the protocol-level choices
are in [05-rich-clipboard.md](05-rich-clipboard.md) section 3.

## LP-107: the daemon core, `lpctl` and the two-daemon test

**A value, not a string.** `ligatured::clipboard::Value` is what the local clipboard holds
as the platform read it: the plain text, the other typed parts with their bytes, and a
selection of files on this device's disk (`LocalFile`: path, base name, size, type, time).
A platform reports a `Change` carrying as much of the value as it could read; the Linux
helper, the Mac and the phone report text only until LP-108 and LP-109 teach them more.
The `Clipboard` trait keeps `read_text` and `set_text` and gains `read_value`, `set_value`,
`mime_types` and `holds_promises`, each defaulting to the text behaviour, so a text-only
platform still implements only the milestone 1 methods.

**What is set carries promises.** `Incoming` is what a peer sent, as the daemon puts it on
the local clipboard: the text, the pushed parts with bytes, the announced parts as
`Promise`s (type and descriptor) and the promised files, with a `Fetch` the platform calls
when an application asks for a promised type. The fetch is asynchronous (`FetchFuture`);
a platform thread blocks on it through the runtime's handle, and the in-memory fake awaits
it directly. A second request for the same id is answered from what the first fetched.

**The fingerprint is the echo rule.** `Fingerprint` maps each representation (`text/plain`
for the text, the type for a part, `file:<name>` for a file) to a SHA-256, from the bytes
for a local value and for a pushed part, from the announced hash for an announced part. The
echo filter keeps the fingerprint of the last value set and the last value sent, and a local
change whose fingerprint is within the last set (every type it reports hashes the same) is
the echo of our own set, so a platform that reads back fewer types than were set, such as
the phone reporting text only, never re-sends. A `set` equal to what was last sent is not
applied.

**One `set` per receiver.** `ClipboardSync::build_for` takes the receiver's params
(`ClipboardParams::from_descriptor`, text only when the peer advertises nothing new): the
text goes as `text` when it fits the push bound, parts the receiver lists go largest first
while the budget lasts, the rest are announced when the receiver has `fetch` and the daemon
has `FilesSync` to offer through, and files are announced on the same condition. A text
above the bound is announced as a `text/plain` part. The announced bytes are bounded by
`ClipboardConfig::keep_bytes` (64 MiB), the largest dropped first. Offers are registered
once per local value and reused for every receiver, under the part's type or `file:<n>`,
with the descriptor named `clipboard.<ext>` for a part and the file's own name for a file,
and withdrawn when the local value changes or `ClipboardConfig::window` (one hour) passes.

**The fetch is a `files` transfer into memory or the cache.** The `Fetcher` of an applied
value asks `FilesSync::fetch` for one id with `Destination::Memory` for a part up to
`ClipboardConfig::memory_limit` (16 MiB) and `Destination::Path` under the cache directory
otherwise and for every file, waits for the transfer, takes what landed from the transfer
(`Transfer::take_landed`, new: a received file's `Landed`, bytes included for memory), and
checks the landed hash against the announced one. A fetch that fails for any reason, a
mismatch, or the sender disconnecting drops the value's promises: the clipboard is set
again to the pushed parts alone, with the fingerprint noted so that set is not an echo.

**The cache is per value and cleared on change.** `ClipboardConfig::cache_dir` is
`$XDG_RUNTIME_DIR/ligature/clipboard` on Linux,
`~/Library/Caches/org.ligatureprotocol.ligatured/clipboard` on macOS and the temporary
directory elsewhere; each applied value gets a numbered directory under it, removed when a
newer value replaces it, and the whole cache is removed when the synchroniser starts.

**`get` on connect, answered with patience.** On attaching a same-person peer the stream
opens with a `get`; the answer is built for the asker like any `set`, or empty when there
is no local value. A `get` tends to arrive before the asker's manifest and before the
answerer's own stream to it is open, so the answer waits up to three seconds for both. An
empty `set` is never applied, so a device that holds nothing does not clear a peer's
clipboard. The local watcher subscribes before anything else runs and the synchroniser
seeds its local value from what the clipboard holds at start, so a daemon that restarts
can answer at once. `ClipboardSync::request` sends a `get` on demand, for the phone's
**Receive now** (LP-109).

**Counters count content.** `counters()` counts `set` messages with content sent and
applied; empty answers to `get` are not counted, which keeps the milestone 1 test exact.

**A milestone 2 peer is told apart by its descriptor.** `ClipboardParams::rich` is set when
the peer's descriptor carries `fetch`, which only this version writes. A `get` goes only to
such a peer (an older one cannot decode it and would end the stream), and an older peer is
sent only sets that carry text, since its decoder requires it; the phone check against the
owner's un-updated daemon found both.

**Types in `lp-core`.** `ClipboardSet` has an optional `text`, `parts` and `files`;
`ClipboardPart` is pushed (`bytes`) or announced (`file`); `ClipboardGet` and the untagged
`ClipboardMessage` decode the stream by shape; `ClipboardParams` carries the descriptor's
params and `clipboard_descriptor_with` builds the descriptor, while the milestone 1
`clipboard_descriptor` stays for the conformance vectors. The fingerprint lives in
`ligatured`, since hashing is `lp-crypto`'s and `lp-core` does not depend on it.

**`lpctl clipboard get`** prints the text and, on standard error, each other part with its
type and size and whether it is a promise, and each promised file; `set` still sets text.

## LP-108: the desktop clipboards

**The Linux helper owns a data-control source.** `wl-clipboard-rs` reads a selection and
copies bytes it holds, but cannot serve a type on request, so the helper
(`ligatured clipboard-helper`) keeps its `Watcher` for reading and adds a second Wayland
connection of its own through `wayland-client`, binding `ext-data-control` or, failing
that, `wlr-data-control`, with one data device per seat. A `set` from the daemon becomes a
source offering the pushed parts (a text under the five plain-text names applications ask
for) and the promised types; a `send` for a pushed type writes the bytes from the helper's
copy, and one for a promised type asks the daemon (`fetch` over the helper's stdio, matched
by id, waited for up to two minutes) and writes what comes back, in a thread per request
with `O_NONBLOCK` cleared. The source loop polls the connection and a wake pipe, so a `set`
from the daemon interrupts it. A promised file list is one promised `text/uri-list` whose
fetch brings every file into the cache and answers with their `file:` URIs; a `text/uri-list`
read from another application becomes the value's files when its URIs are local paths.

**The helper tells its own selection apart by the offered types.** The compositor reports
the helper's own selection to its watcher like any other; the helper skips a change whose
set of types equals what it offered, and reads again once its source is cancelled. Frames
between the daemon and the helper are bounded at 256 MiB rather than the wire's 1 MiB,
since a clipboard value is not a wire message.

**The Mac keeps the pasteboard's own laziness.** `MacClipboard` maps types through a table
(`public.html`, `public.rtf`, `public.png`, `public.tiff`, with `public.utf8-plain-text` for the
text and `public.file-url` for files), writes a value as `NSPasteboardItem`s (the text and pushed
parts as data, a lazy `NSPasteboardItemDataProvider` for the announced types, and one item per
promised file whose `public.file-url` provider fetches the file into the cache and answers with its
URL), and skips its own `changeCount` when polling. The provider is an Objective-C class defined
with `objc2::define_class!` in the `lp-macos` crate, the second crate allowed `unsafe` (the class's
initialisation and thread marking need it, and `ligatured` forbids it), behind a safe `read`,
`write` and `change_count`; the daemon's provider closure holds the value's fetcher and a runtime
handle it blocks on. The plan's `NSFilePromiseProvider` is not used: a fetched file URL is enough
for Finder and needs no completion blocks. This code is checked by CI's macOS build and the run,
since the Linux machine cannot compile for the Mac. The run found that the pasteboard delivers a
request for a lazily offered type to the owning process's main thread, which a `#[tokio::main]`
daemon never services (Preview spun for ever), so on macOS `ligatured` runs its work on another
thread and parks its main thread in the Cocoa run loop (`lp_macos::run_main_loop`).

**`lpctl clipboard put` and `paste`** exist for the run: `put <mime> <path>` puts one typed
part on the local clipboard as if copied, and `paste <mime> <out>` has the daemon write what
an application would get, fetching a promise, to a file, since a value can exceed a control
frame. The fake clipboard's paste fetches; a platform clipboard's answers from the pushed
parts, as its promises are served to applications by the platform.

**A nested compositor for the run.** The desktop checks ran against a nested Hyprland on its
own socket (`Hyprland --socket lp-test --wayland-fd N` with a socket handed over), so the
machine's real clipboard and the owner's daemon were untouched; `LIGATURE_CLIPBOARD_CACHE`
names the cache directory so two daemons on one machine do not share one.

## LP-109: Android

**The clip carries the text and HTML, the provider everything else.** `Host::set_clipboard_value`
takes a `HostClipValue`: the text and the HTML go on the `ClipData` item itself (Android
has first-class fields for both), and every other part, every announced part and every
promised file becomes a content URI of the app's `ClipboardProvider` under a key the node
chose (`part-<type>`, `promise-<id>`, `file-<id>`). The clip's description lists every type,
so an application that pastes images sees `image/png` on offer; the provider is not exported
and grants URI permissions, which the clipboard service hands to the pasting app. Opening a
URI asks the node (`Node::clipboard_item`) for the key: a pushed part's bytes come from
memory through a pipe, a promised part or file is fetched then, from the device that
announced it, and a file is opened from the clipboard cache under the app's cache directory
(`Config::cache_dir`). Pasting is what moves the bytes, as on the desktops, with one
exception the run found: the system's copy overlay asks the provider for a thumbnail the
moment a clip is set, which would fetch an image at copy time, so a thumbnail request (one
carrying `EXTRA_SIZE`) gets a placeholder image and only a plain read fetches. The clip is
also marked as from a remote device (`EXTRA_IS_REMOTE_DEVICE`), which the overlay shows.

**Reading takes what the clip holds.** `NodeHolder.readAndSend` reads the text, the HTML
and every content URI of the primary clip at the moments the app may read (its screen
gaining focus, the tile, the notification action, the service's listener), opens the URIs the
way the share sheet does (`Transfers.open`, descriptors kept until the next read) and hands
them to `Node::clipboard_changed_value`; the node keeps an image up to 64 MiB as a part and
anything else as a file of the selection. A clip whose URIs belong to this app's own provider
is a value a peer sent and is not read back.

**Receive now and Save.** **Receive now** on the clipboard card sends a `get` to every
connected device that offers the clipboard (`Node::request_clipboard`). A file list on the
clipboard raises a notification on the transfers channel with a **Save** action, whose
receiver asks the node (`Node::save_clipboard_files`) to fetch the promised files from the
device that announced them into Downloads through the milestone 4 landing, so the app's
transfer notification shows it like any transfer. The node knows which device to ask through
`ClipboardSync::promised_from`.

**Advertised:** `text/plain`, `text/html`, `image/png`, `text/uri-list`, `fetch` true,
`maxBytes` the default. The two-node host test sends a text, HTML and 1 MiB image value from
the phone node and checks the laptop's host got the image as a promise item fetched only on
`clipboard_item`, then a file selection saved into the laptop's downloads with
`save_clipboard_files`, and `request_clipboard` reaching one peer.
