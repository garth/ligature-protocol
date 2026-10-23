//! Two daemons of one person share a clipboard of several representations
//! (LP-107): small parts are pushed with the text, a large part is announced
//! and fetched only on paste, a second paste costs no transfer, a withdrawn
//! or expired announcement fails the paste cleanly with the text kept, a
//! file selection is fetched into the cache, the echo rule holds over the
//! set, and a device that connects later gets the current value with `get`.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ligatured::clipboard::{Clipboard, FakeClipboard, Part, Value};
use ligatured::clipboard_sync::{ClipboardConfig, ClipboardSync};
use ligatured::daemon::Daemon;
use ligatured::files::DirectoryStore;
use ligatured::files_sync::{FilesConfig, FilesSync};
use ligatured::run::spawn_connection_events;
use lp_core::capability::CLIPBOARD_TYPE;
use lp_core::facts::Role;
use lp_core::files::FILES_TYPE;
use lp_core::paths::Locations;
use lp_core::wire::Timestamp;
use lp_identity::cross_signing::PersonKeys;
use lp_identity::facts;

fn at(offset: i64) -> Timestamp {
    Timestamp::from_unix(1_760_000_000 + offset).unwrap()
}

async fn until(what: &str, check: impl Fn() -> bool) {
    let waited = tokio::time::timeout(Duration::from_secs(20), async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "{what}");
}

struct Device {
    daemon: Arc<Daemon>,
    clipboard: Arc<FakeClipboard>,
    sync: Arc<ClipboardSync>,
    files: Arc<FilesSync>,
    cache: tempfile::TempDir,
    _downloads: tempfile::TempDir,
    _data: tempfile::TempDir,
}

/// A daemon of `alice` named `name`, with a fake clipboard, files, and the
/// given announcement window.
fn device(
    alice: &PersonKeys,
    name: &str,
    window: Duration,
) -> (
    Device,
    tokio::sync::mpsc::Receiver<lp_transport::manager::Event>,
) {
    let data = tempfile::tempdir().unwrap();
    let loc = Locations::resolve(Some(data.path()), None);
    let (daemon, events) = Daemon::open(loc, "127.0.0.1:0".parse().unwrap(), Some(name)).unwrap();
    daemon
        .update_keys(|k| k.set_joined(*alice.person(), Some(&alice.self_signing)))
        .unwrap();
    let downloads = tempfile::tempdir().unwrap();
    let files = daemon.attach_files(
        DirectoryStore::new(downloads.path().to_path_buf()),
        FilesConfig {
            max_chunk_bytes: 4096,
            ..FilesConfig::default()
        },
    );
    let cache = tempfile::tempdir().unwrap();
    let clipboard = FakeClipboard::new();
    let sync = daemon.attach_clipboard_with(
        clipboard.clone(),
        ClipboardConfig {
            window,
            cache_dir: cache.path().join("clipboard"),
            ..ClipboardConfig::default()
        },
    );
    (
        Device {
            daemon,
            clipboard,
            sync,
            files,
            cache,
            _downloads: downloads,
            _data: data,
        },
        events,
    )
}

/// Makes every device a binding of `alice` in every store, then connects
/// each later one to the first.
async fn join(
    alice: &PersonKeys,
    devices: Vec<(
        Device,
        tokio::sync::mpsc::Receiver<lp_transport::manager::Event>,
    )>,
) -> Vec<Device> {
    let person = facts::person_fact(alice, "Alice", 1, at(0)).unwrap();
    let roles: Vec<_> = devices
        .iter()
        .enumerate()
        .map(|(i, (d, _))| {
            facts::role_fact(
                alice,
                Role::Binding,
                *d.daemon.binding().public(),
                true,
                at(1 + i as i64),
            )
            .unwrap()
        })
        .collect();
    let mut out = Vec::new();
    for (d, events) in devices {
        d.daemon.store().insert(&person).unwrap();
        for r in &roles {
            d.daemon.store().insert(r).unwrap();
        }
        spawn_connection_events(d.daemon.clone(), events);
        out.push(d);
    }
    let first = out[0].daemon.manager().endpoint().local_addr().unwrap();
    for d in &out[1..] {
        d.daemon.manager().connect(first).await.unwrap();
    }
    let n = out.len();
    until("clipboard and files offered all round", || {
        out.iter().all(|d| {
            d.daemon.registry().peers_offering(CLIPBOARD_TYPE).len() == n - 1
                && d.daemon.registry().peers_offering(FILES_TYPE).len() == n - 1
        })
    })
    .await;
    out
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

/// Counts the transfers `rx` has seen start since the last call.
fn started(
    rx: &mut tokio::sync::broadcast::Receiver<Arc<ligatured::files_sync::Transfer>>,
) -> usize {
    let mut n = 0;
    while rx.try_recv().is_ok() {
        n += 1;
    }
    n
}

fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    names
}

#[tokio::test]
async fn small_parts_are_pushed_and_a_large_one_is_fetched_only_on_paste() {
    let alice = PersonKeys::generate().unwrap();
    let hour = Duration::from_secs(3600);
    let devices = join(
        &alice,
        vec![device(&alice, "a", hour), device(&alice, "b", hour)],
    )
    .await;
    let (a, b) = (&devices[0], &devices[1]);
    let mut b_transfers = b.files.subscribe();

    // Text with HTML: both pushed in one set.
    let mut rich = Value::text("hello");
    rich.parts = vec![Part::new("text/html", b"<b>hello</b>".to_vec())];
    a.clipboard.user_copies_value(rich);
    until("html on b", || {
        b.clipboard
            .read_value()
            .unwrap()
            .is_some_and(|v| v.text.as_deref() == Some("hello") && v.parts.len() == 1)
    })
    .await;
    assert_eq!(
        b.clipboard.paste("text/html").await.unwrap(),
        b"<b>hello</b>"
    );
    assert!(b.sync.promises().is_empty());
    assert_eq!(
        started(&mut b_transfers),
        0,
        "nothing transferred for a pushed value"
    );

    // A 1 MiB image: above the push bound, so announced and fetched on paste.
    let png = pattern(1024 * 1024, 7);
    let mut image = Value::text("a picture");
    image.parts = vec![Part::new("image/png", png.clone())];
    a.clipboard.user_copies_value(image);
    until("promise on b", || !b.sync.promises().is_empty()).await;
    assert_eq!(
        b.clipboard.read_text().unwrap().as_deref(),
        Some("a picture")
    );
    assert_eq!(b.sync.promises()[0].mime, "image/png");
    assert_eq!(
        started(&mut b_transfers),
        0,
        "nothing transferred before the paste"
    );
    assert_eq!(a.files.offers().len(), 1, "a offers the part");
    assert_eq!(b.clipboard.paste("image/png").await.unwrap(), png);
    assert_eq!(started(&mut b_transfers), 1, "one transfer for the paste");
    assert_eq!(b.clipboard.paste("image/png").await.unwrap(), png);
    assert_eq!(started(&mut b_transfers), 0, "a second paste costs nothing");

    // The echo rule over the set: a sent one value each, b applied two,
    // and nothing came back.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(a.sync.counters(), (2, 0));
    assert_eq!(b.sync.counters(), (0, 2));

    // A copies something else: the offer is withdrawn and the promise is
    // gone from b's clipboard, which now holds the new text.
    a.clipboard.user_copies("next");
    until("next on b", || {
        b.clipboard.read_text().unwrap().as_deref() == Some("next")
    })
    .await;
    assert!(a.files.offers().is_empty(), "the old part is withdrawn");
    assert!(b.sync.promises().is_empty());
    assert!(b.clipboard.paste("image/png").await.is_err());
    for d in &devices {
        d.daemon.shutdown();
    }
}

#[tokio::test]
async fn an_expired_announcement_fails_the_paste_and_keeps_the_text() {
    let alice = PersonKeys::generate().unwrap();
    let devices = join(
        &alice,
        vec![
            device(&alice, "a", Duration::from_millis(400)),
            device(&alice, "b", Duration::from_secs(3600)),
        ],
    )
    .await;
    let (a, b) = (&devices[0], &devices[1]);
    let png = pattern(1024 * 1024, 3);
    let mut image = Value::text("expiring");
    image.parts = vec![Part::new("image/png", png)];
    a.clipboard.user_copies_value(image);
    until("promise on b", || !b.sync.promises().is_empty()).await;
    until("a withdrew after the window", || {
        a.files.offers().is_empty()
    })
    .await;
    let err = b.clipboard.paste("image/png").await.unwrap_err();
    assert!(
        err.to_string().contains("gone") || err.to_string().contains("ended"),
        "{err}"
    );
    assert_eq!(
        b.clipboard.read_text().unwrap().as_deref(),
        Some("expiring")
    );
    assert!(b.sync.promises().is_empty(), "the promise is dropped");
    assert!(b.clipboard.paste("image/png").await.is_err());
    for d in &devices {
        d.daemon.shutdown();
    }
}

#[tokio::test]
async fn a_file_selection_is_fetched_into_the_cache_on_paste() {
    let alice = PersonKeys::generate().unwrap();
    let hour = Duration::from_secs(3600);
    let devices = join(
        &alice,
        vec![device(&alice, "a", hour), device(&alice, "b", hour)],
    )
    .await;
    let (a, b) = (&devices[0], &devices[1]);
    let mut b_transfers = b.files.subscribe();
    let src = tempfile::tempdir().unwrap();
    let mut files = Vec::new();
    for (name, bytes) in [
        ("notes.txt", pattern(20_000, 1)),
        ("photo.jpg", pattern(300_000, 2)),
    ] {
        let path = src.path().join(name);
        std::fs::write(&path, &bytes).unwrap();
        files.push(ligatured::clipboard_sync::local_file(&path).unwrap());
    }
    let selection = Value {
        text: Some("notes.txt\nphoto.jpg".to_owned()),
        parts: Vec::new(),
        files,
    };
    a.clipboard.user_copies_value(selection);
    until("files promised on b", || b.sync.promised_files().len() == 2).await;
    assert_eq!(started(&mut b_transfers), 0);
    let paths = b.clipboard.paste_files().await.unwrap();
    assert_eq!(started(&mut b_transfers), 2, "one transfer per file");
    assert_eq!(paths.len(), 2);
    for (p, (name, bytes)) in paths.iter().zip([
        ("notes.txt", pattern(20_000, 1)),
        ("photo.jpg", pattern(300_000, 2)),
    ]) {
        assert_eq!(p.file_name().unwrap().to_string_lossy(), name);
        assert!(
            p.starts_with(b.cache.path()),
            "{} is under the cache",
            p.display()
        );
        assert_eq!(std::fs::read(p).unwrap(), bytes);
    }
    // The next value clears the cache.
    a.clipboard.user_copies("done");
    until("done on b", || {
        b.clipboard.read_text().unwrap().as_deref() == Some("done")
    })
    .await;
    until("cache cleared", || {
        listing(&b.cache.path().join("clipboard")).is_empty()
    })
    .await;
    assert!(b.sync.promised_files().is_empty());
    for d in &devices {
        d.daemon.shutdown();
    }
}

#[tokio::test]
async fn a_device_that_connects_later_gets_the_current_value_with_get() {
    let alice = PersonKeys::generate().unwrap();
    let hour = Duration::from_secs(3600);
    let (a, ea) = device(&alice, "a", hour);
    let (b, eb) = device(&alice, "b", hour);
    // a holds a value before anyone connects.
    let mut rich = Value::text("already here");
    rich.parts = vec![Part::new("text/html", b"<i>already here</i>".to_vec())];
    a.clipboard.user_copies_value(rich);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let devices = join(&alice, vec![(a, ea), (b, eb)]).await;
    let (a, b) = (&devices[0], &devices[1]);
    until("b asked and got a's value", || {
        b.clipboard
            .read_value()
            .unwrap()
            .is_some_and(|v| v.text.as_deref() == Some("already here") && v.parts.len() == 1)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(a.sync.counters(), (1, 0), "a answered one get with content");
    assert_eq!(b.sync.counters(), (0, 1));
    // Asking again yields the same value, which is not applied twice.
    assert!(b.sync.request(a.daemon.binding().public()).await);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(a.sync.counters(), (2, 0));
    assert_eq!(
        b.sync.counters(),
        (0, 1),
        "the same value again is not applied"
    );
    for d in &devices {
        d.daemon.shutdown();
    }
}

/// A value is recorded as applied before the platform gets it, so a read
/// the moment it lands finds its promises; a platform that refuses the
/// value leaves nothing recorded (LP-107).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_value_is_recorded_before_the_platform_gets_it_and_undone_if_refused() {
    let alice = PersonKeys::generate().unwrap();
    let hour = Duration::from_secs(3600);
    let devices = join(
        &alice,
        vec![device(&alice, "a", hour), device(&alice, "b", hour)],
    )
    .await;
    let (a, b) = (&devices[0], &devices[1]);

    // B's platform reads the promises back the instant the value is set.
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sync = b.sync.clone();
    let seen_in_hook = seen.clone();
    b.clipboard.on_set(move |incoming| {
        if incoming.has_promises() {
            seen_in_hook
                .lock()
                .unwrap()
                .push((sync.promised_from(), sync.promised_files().len()));
        }
    });
    let png = pattern(1024 * 1024, 3);
    let mut image = Value::text("a picture");
    image.parts = vec![Part::new("image/png", png)];
    a.clipboard.user_copies_value(image);
    until("promise on b", || !b.sync.promises().is_empty()).await;
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "the hook ran once for the announced value");
    assert_eq!(
        seen[0].0,
        Some(*a.daemon.binding().public()),
        "the sender was recorded before the platform was told"
    );

    // B's platform refuses the next value: nothing stays recorded.
    b.clipboard.refuse_next_set();
    let mut another = Value::text("another");
    another.parts = vec![Part::new("image/png", pattern(1024 * 1024, 4))];
    a.clipboard.user_copies_value(another);
    until("the refused set was applied as nothing", || {
        b.sync.counters().1 == 1 && b.sync.promised_from().is_none()
    })
    .await;
    assert!(
        b.sync.promises().is_empty(),
        "a refused value leaves no promise"
    );
}
