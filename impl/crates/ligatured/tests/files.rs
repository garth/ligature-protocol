//! Two daemons of one person move files over control and data streams
//! (LP-102): a push of several files lands them under the name rule, the
//! receiver's `maxStreams` bounds how many run at once, a fetch of an
//! offered id yields the same bytes and a withdrawn one is declined as gone,
//! a cancel from either end leaves no part file, and a file whose bytes do not
//! match its descriptor is refused.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ligatured::daemon::Daemon;
use ligatured::files::{Destination, DirectoryStore, FileSource};
use ligatured::files_sync::{FileState, FilesConfig, FilesSync, TransferEnd};
use ligatured::run::spawn_connection_events;
use lp_core::facts::Role;
use lp_core::files::{FILES_TYPE, FileDescriptor, Reason};
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
    files: Arc<FilesSync>,
    downloads: tempfile::TempDir,
    _data: tempfile::TempDir,
}

/// Two daemons of one person, connected, each landing files in its own
/// download directory with the given stream bound and a small chunk so a
/// modest file takes hundreds of chunks.
async fn pair(a_streams: u32, b_streams: u32) -> (Device, Device) {
    let alice = PersonKeys::generate().unwrap();
    let person = facts::person_fact(&alice, "Alice", 1, at(0)).unwrap();
    let mut daemons = Vec::new();
    for name in ["linux", "mac"] {
        let dir = tempfile::tempdir().unwrap();
        let loc = Locations::resolve(Some(dir.path()), None);
        let (d, events) = Daemon::open(loc, "127.0.0.1:0".parse().unwrap(), Some(name)).unwrap();
        daemons.push((d, events, dir));
    }
    let roles: Vec<_> = daemons
        .iter()
        .enumerate()
        .map(|(i, (d, _, _))| {
            facts::role_fact(
                &alice,
                Role::Binding,
                *d.binding().public(),
                true,
                at(1 + i as i64),
            )
            .unwrap()
        })
        .collect();
    for (d, _, _) in &daemons {
        d.store().insert(&person).unwrap();
        for r in &roles {
            d.store().insert(r).unwrap();
        }
        d.update_keys(|k| k.set_joined(*alice.person(), Some(&alice.self_signing)))
            .unwrap();
    }
    let mut out = Vec::new();
    for ((daemon, events, data), streams) in daemons.into_iter().zip([a_streams, b_streams]) {
        let downloads = tempfile::tempdir().unwrap();
        let files = daemon.attach_files(
            DirectoryStore::new(downloads.path().to_path_buf()),
            FilesConfig {
                max_streams: streams,
                max_chunk_bytes: 4096,
                send_streams: 8,
            },
        );
        spawn_connection_events(daemon.clone(), events);
        out.push(Device {
            daemon,
            files,
            downloads,
            _data: data,
        });
    }
    let b = out.pop().unwrap();
    let a = out.pop().unwrap();
    a.daemon
        .manager()
        .connect(b.daemon.manager().endpoint().local_addr().unwrap())
        .await
        .unwrap();
    until("files offered both ways", || {
        a.daemon.registry().peers_offering(FILES_TYPE).len() == 1
            && b.daemon.registry().peers_offering(FILES_TYPE).len() == 1
    })
    .await;
    (a, b)
}

fn write_file(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

fn sources(paths: &[std::path::PathBuf]) -> Vec<(FileDescriptor, FileSource)> {
    paths
        .iter()
        .map(|p| FileSource::describe_path(&FilesSync::new_id().unwrap(), p).unwrap())
        .collect()
}

fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_push_lands_files_under_the_name_rule_one_stream_at_a_time() {
    let (a, b) = pair(4, 1).await;
    let src = tempfile::tempdir().unwrap();
    let big = pattern(2 * 1024 * 1024 + 7, 1);
    let paths = vec![
        write_file(src.path(), "big.bin", &big),
        write_file(src.path(), "notes.txt", b"hello"),
        write_file(src.path(), "empty", b""),
    ];
    let transfer = a
        .files
        .push(b.daemon.binding().public(), sources(&paths))
        .unwrap();
    let outcome = transfer.wait().await;
    assert_eq!(outcome.finished, Some(TransferEnd::Complete), "{outcome:?}");
    assert!(outcome.all_received(), "{outcome:?}");
    assert_eq!(outcome.done_bytes(), big.len() as u64 + 5);
    assert_eq!(
        listing(b.downloads.path()),
        ["big.bin", "empty", "notes.txt"]
    );
    assert_eq!(
        std::fs::read(b.downloads.path().join("big.bin")).unwrap(),
        big
    );
    assert_eq!(
        std::fs::read(b.downloads.path().join("notes.txt")).unwrap(),
        b"hello"
    );
    // The receiver took one file at a time and every stream was accounted for.
    assert_eq!(b.files.peak_data_streams(), 1);
    until("receiver's streams closed", || {
        b.files.open_data_streams() == 0
    })
    .await;
    until("both ends forgot the transfer", || {
        b.files.transfers().is_empty() && a.files.transfers().is_empty()
    })
    .await;
    // The modification time travelled.
    let sent = std::fs::metadata(&paths[1]).unwrap().modified().unwrap();
    let landed = std::fs::metadata(b.downloads.path().join("notes.txt"))
        .unwrap()
        .modified()
        .unwrap();
    let diff = sent
        .duration_since(landed)
        .unwrap_or_else(|e| e.duration())
        .as_secs();
    assert!(diff <= 1, "modified time off by {diff}s");

    // The same names again land beside the first ones, numbered.
    let again = a
        .files
        .push(b.daemon.binding().public(), sources(&paths[1..]))
        .unwrap();
    assert!(again.wait().await.all_received());
    assert_eq!(
        listing(b.downloads.path()),
        [
            "big.bin",
            "empty",
            "empty (2)",
            "notes (2).txt",
            "notes.txt"
        ]
    );
    // The receiver recorded the implicit grant of its files capability.
    let grants = ligatured::grants::list(&b.daemon).unwrap();
    assert!(
        grants
            .grants
            .iter()
            .any(|g| g.capability == FILES_TYPE && g.grantee == *a.daemon.binding().public())
    );
    a.daemon.shutdown();
    b.daemon.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_receiver_allowing_four_streams_gets_small_files_before_a_large_one_finishes() {
    let (a, b) = pair(1, 4).await;
    let src = tempfile::tempdir().unwrap();
    let big = pattern(24 * 1024 * 1024, 2);
    let mut paths = vec![write_file(src.path(), "big.bin", &big)];
    for i in 0..10 {
        paths.push(write_file(
            src.path(),
            &format!("small-{i}.txt"),
            &pattern(100 + i, i as u8),
        ));
    }
    let transfer = a
        .files
        .push(b.daemon.binding().public(), sources(&paths))
        .unwrap();
    // While the big file is still moving, the small ones are already done.
    until("small files received while big is active", || {
        let p = transfer.progress();
        let big = &p.files[0];
        let small_done = p.files[1..]
            .iter()
            .filter(|f| matches!(f.state, FileState::Received { .. }))
            .count();
        small_done == 10 && big.state == FileState::Active
    })
    .await;
    let outcome = transfer.wait().await;
    assert!(outcome.all_received(), "{outcome:?}");
    assert!(
        b.files.peak_data_streams() > 1 && b.files.peak_data_streams() <= 4,
        "peak {}",
        b.files.peak_data_streams()
    );
    until("receiver's streams closed", || {
        b.files.open_data_streams() == 0
    })
    .await;
    assert_eq!(listing(b.downloads.path()).len(), 11);
    a.daemon.shutdown();
    b.daemon.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fetch_of_an_offered_id_yields_the_bytes_and_a_withdrawn_one_is_gone() {
    let (a, b) = pair(2, 2).await;
    let src = tempfile::tempdir().unwrap();
    let bytes = pattern(300_000, 3);
    let path = write_file(src.path(), "offered.bin", &bytes);
    let offered = a.files.offer_path(&path).unwrap();
    assert_eq!(a.files.offers().len(), 1);
    // Into the download folder.
    let fetched = b
        .files
        .fetch(
            a.daemon.binding().public(),
            vec![offered.id.clone(), "no-such-id".to_owned()],
            Destination::Downloads,
        )
        .unwrap();
    let outcome = fetched.wait().await;
    assert_eq!(outcome.finished, Some(TransferEnd::Complete), "{outcome:?}");
    assert_eq!(outcome.files.len(), 2);
    assert!(matches!(outcome.files[0].state, FileState::Received { .. }));
    assert_eq!(outcome.files[0].name, "offered.bin");
    assert_eq!(
        outcome.files[1].state,
        FileState::Failed {
            reason: Some(Reason::Gone),
            message: "no longer offered".to_owned(),
        }
    );
    assert_eq!(
        std::fs::read(b.downloads.path().join("offered.bin")).unwrap(),
        bytes
    );
    // Into memory, for a capability inside the daemon.
    let into_memory = b
        .files
        .fetch(
            a.daemon.binding().public(),
            vec![offered.id.clone()],
            Destination::Memory,
        )
        .unwrap();
    assert!(into_memory.wait().await.all_received());
    assert_eq!(
        listing(b.downloads.path()),
        ["offered.bin"],
        "memory leaves no file"
    );
    // To a chosen path.
    let to = b.downloads.path().join("elsewhere").join("copy.bin");
    let into_path = b
        .files
        .fetch(
            a.daemon.binding().public(),
            vec![offered.id.clone()],
            Destination::Path(to.clone()),
        )
        .unwrap();
    assert!(into_path.wait().await.all_received());
    assert_eq!(std::fs::read(&to).unwrap(), bytes);
    // Withdrawn: declined as gone, nothing written.
    assert!(a.files.withdraw(&offered.id));
    assert!(!a.files.withdraw(&offered.id));
    let before = listing(b.downloads.path());
    let gone = b
        .files
        .fetch(
            a.daemon.binding().public(),
            vec![offered.id.clone()],
            Destination::Downloads,
        )
        .unwrap();
    let outcome = gone.wait().await;
    assert_eq!(
        outcome.finished,
        Some(TransferEnd::Declined {
            reason: Reason::Gone
        }),
        "{outcome:?}"
    );
    assert_eq!(listing(b.downloads.path()), before);
    // The sender recorded the implicit grant of its files capability.
    let grants = ligatured::grants::list(&a.daemon).unwrap();
    assert!(
        grants
            .grants
            .iter()
            .any(|g| g.capability == FILES_TYPE && g.grantee == *b.daemon.binding().public())
    );
    a.daemon.shutdown();
    b.daemon.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancel_from_either_end_leaves_no_part_file_and_the_next_transfer_works() {
    let (a, b) = pair(2, 2).await;
    let src = tempfile::tempdir().unwrap();
    let big = pattern(8 * 1024 * 1024, 4);
    let paths = vec![
        write_file(src.path(), "big.bin", &big),
        write_file(src.path(), "after.txt", b"after"),
    ];
    // The sender cancels once bytes are moving.
    let transfer = a
        .files
        .push(b.daemon.binding().public(), sources(&paths))
        .unwrap();
    until("bytes moving", || {
        transfer.progress().done_bytes() > 64 * 1024
    })
    .await;
    assert!(a.files.cancel(&transfer.id()));
    let outcome = transfer.wait().await;
    assert_eq!(
        outcome.finished,
        Some(TransferEnd::Cancelled),
        "{outcome:?}"
    );
    until("receiver's transfer ended", || {
        b.files.transfers().is_empty()
    })
    .await;
    until("receiver's streams closed", || {
        b.files.open_data_streams() == 0
    })
    .await;
    let left = listing(b.downloads.path());
    assert!(
        left.iter()
            .all(|n| !n.ends_with(".ligature-part") && n != "big.bin"),
        "{left:?}"
    );
    // The receiver cancels the next one.
    let transfer = a
        .files
        .push(b.daemon.binding().public(), sources(&paths))
        .unwrap();
    let on_b = loop {
        if let Some(t) = b
            .files
            .transfers()
            .into_iter()
            .find(|t| t.done_bytes() > 64 * 1024)
        {
            break t;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    assert!(b.files.cancel(&on_b.transfer));
    let outcome = transfer.wait().await;
    assert_eq!(
        outcome.finished,
        Some(TransferEnd::Cancelled),
        "{outcome:?}"
    );
    until("receiver's streams closed", || {
        b.files.open_data_streams() == 0
    })
    .await;
    until("receiver's transfer ended", || {
        b.files.transfers().is_empty()
    })
    .await;
    let left = listing(b.downloads.path());
    assert!(
        left.iter()
            .all(|n| !n.ends_with(".ligature-part") && n != "big.bin"),
        "{left:?}"
    );
    // The next transfer between the two works.
    let transfer = a
        .files
        .push(b.daemon.binding().public(), sources(&paths[1..]))
        .unwrap();
    assert!(transfer.wait().await.all_received());
    assert!(listing(b.downloads.path()).contains(&"after.txt".to_owned()));
    a.daemon.shutdown();
    b.daemon.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_file_whose_bytes_do_not_match_its_descriptor_is_refused() {
    let (a, b) = pair(2, 2).await;
    // Declared as 10 bytes, 12 are behind it: the receiver refuses it at the
    // eleventh byte and keeps nothing; the honest file beside it lands.
    let lying = FileDescriptor::new("lie", "lie.bin", 10);
    let honest = FileDescriptor::new("ok", "ok.bin", 3);
    let transfer = a
        .files
        .push(
            b.daemon.binding().public(),
            vec![
                (lying, FileSource::Bytes(Arc::new(vec![7u8; 12]))),
                (honest, FileSource::Bytes(Arc::new(vec![1, 2, 3]))),
            ],
        )
        .unwrap();
    let outcome = transfer.wait().await;
    assert_eq!(outcome.finished, Some(TransferEnd::Complete), "{outcome:?}");
    // Whichever end notices first: the sender reading past the declared size
    // or the receiver seeing the excess chunk.
    assert!(
        matches!(
            &outcome.files[0].state,
            FileState::Failed {
                reason: Some(Reason::SizeMismatch),
                ..
            }
        ),
        "{outcome:?}"
    );
    assert!(matches!(outcome.files[1].state, FileState::Received { .. }));
    assert_eq!(listing(b.downloads.path()), ["ok.bin"]);
    until("receiver's streams closed", || {
        b.files.open_data_streams() == 0
    })
    .await;
    a.daemon.shutdown();
    b.daemon.shutdown();
}
