//! Two daemons of one person move files over control and data streams
//! (LP-102): a push of several files lands them under the name rule, the
//! receiver's `maxStreams` bounds how many run at once, a fetch of an
//! offered id yields the same bytes and a withdrawn one is declined as gone,
//! a cancel from either end leaves no part file, a file whose bytes do not
//! match its descriptor is refused, a file whose landed bytes do not hash to
//! the sender's `end` is dropped with its part file, and a data frame over the
//! 1 MiB bound fails the file and leaves nothing behind (LP-133).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ligatured::daemon::Daemon;
use ligatured::files::{Destination, DirectoryStore, FileSource, FileStore, Landed, Landing};
use ligatured::files_sync::{FileState, FilesConfig, FilesSync, TransferEnd};
use ligatured::run::spawn_connection_events;
use lp_core::Error;
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
    let (a, b, _alice) = pair_with(a_streams, b_streams, |dir| DirectoryStore::new(dir)).await;
    (a, b)
}

/// As [`pair`], with the second device's store made by `make_store` from
/// its download directory, and the person returned so a test can trust a
/// further key.
async fn pair_with(
    a_streams: u32,
    b_streams: u32,
    make_store: impl Fn(std::path::PathBuf) -> Arc<dyn FileStore>,
) -> (Device, Device, PersonKeys) {
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
        let store = if out.is_empty() {
            DirectoryStore::new(downloads.path().to_path_buf())
        } else {
            make_store(downloads.path().to_path_buf())
        };
        let files = daemon.attach_files(
            store,
            FilesConfig {
                max_streams: streams,
                max_chunk_bytes: 4096,
                send_streams: 8,
                ..FilesConfig::default()
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
    (a, b, alice)
}

/// A download directory whose disk lies: the first byte of every file is
/// flipped on the way in, so what lands never hashes to what was sent.
struct CorruptingStore {
    dir: std::path::PathBuf,
}

impl FileStore for CorruptingStore {
    fn available(&self) -> bool {
        std::fs::create_dir_all(&self.dir).is_ok()
    }

    fn open(&self, file: &FileDescriptor) -> Result<Box<dyn Landing>, Error> {
        Ok(Box::new(CorruptingLanding {
            inner: DirectoryStore::open_in(&self.dir, file)?,
            flipped: false,
        }))
    }

    fn describe(&self) -> String {
        format!("{} (corrupting)", self.dir.display())
    }
}

struct CorruptingLanding {
    inner: Box<dyn Landing>,
    flipped: bool,
}

impl Landing for CorruptingLanding {
    fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if self.flipped || bytes.is_empty() {
            return self.inner.write(bytes);
        }
        self.flipped = true;
        let mut lied = bytes.to_vec();
        lied[0] ^= 0xff;
        self.inner.write(&lied)
    }

    fn digest(&self) -> String {
        self.inner.digest()
    }

    fn finish(self: Box<Self>, modified: Option<Timestamp>) -> Result<Landed, Error> {
        self.inner.finish(modified)
    }

    fn abort(self: Box<Self>) {
        self.inner.abort();
    }
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
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let on_b = loop {
        if let Some(t) = b
            .files
            .transfers()
            .into_iter()
            .find(|t| t.done_bytes() > 64 * 1024)
        {
            break t;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "B saw the transfer under way"
        );
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

/// A file whose landed bytes do not hash to the sender's `end` is refused as
/// a hash mismatch: the part file goes, nothing is put in place, the sender
/// is told, and the next transfer works. The receiver's disk is what lies
/// here, since a sender hashes what it sends.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_file_whose_landed_bytes_do_not_match_the_hash_is_dropped_with_its_part_file() {
    let (a, b, _alice) = pair_with(2, 2, |dir| Arc::new(CorruptingStore { dir })).await;
    // The receiver forgets a transfer the moment it ends, so its record is
    // taken as it starts rather than looked for after.
    let mut b_started = b.files.subscribe();
    let bytes = pattern(10_000, 3);
    let file = FileDescriptor::new("f1", "photo.bin", bytes.len() as u64);
    let transfer = a
        .files
        .push(
            b.daemon.binding().public(),
            vec![(file, FileSource::Bytes(Arc::new(bytes)))],
        )
        .unwrap();
    let outcome = transfer.wait().await;
    assert_eq!(outcome.finished, Some(TransferEnd::Complete), "{outcome:?}");
    assert!(
        matches!(
            &outcome.files[0].state,
            FileState::Failed {
                reason: Some(Reason::HashMismatch),
                ..
            }
        ),
        "the sender is told why: {outcome:?}"
    );
    let received = tokio::time::timeout(Duration::from_secs(10), b_started.recv())
        .await
        .expect("the receiver started a transfer")
        .unwrap();
    let recorded = tokio::time::timeout(Duration::from_secs(10), received.wait())
        .await
        .expect("the receiver's transfer ended");
    assert!(
        matches!(
            recorded.files[0].state,
            FileState::Failed {
                reason: Some(Reason::HashMismatch),
                ..
            }
        ),
        "the receiver recorded the failure: {recorded:?}"
    );
    assert!(
        listing(b.downloads.path()).is_empty(),
        "no file and no part file: {:?}",
        listing(b.downloads.path())
    );
    until("receiver's streams closed", || {
        b.files.open_data_streams() == 0
    })
    .await;

    // The other way still works: a's disk is honest.
    let honest = FileDescriptor::new("f2", "ok.bin", 3);
    let transfer = b
        .files
        .push(
            a.daemon.binding().public(),
            vec![(honest, FileSource::Bytes(Arc::new(vec![1, 2, 3])))],
        )
        .unwrap();
    let outcome = transfer.wait().await;
    assert!(
        matches!(outcome.files[0].state, FileState::Received { .. }),
        "{outcome:?}"
    );
    assert_eq!(listing(a.downloads.path()), ["ok.bin"]);
    a.daemon.shutdown();
    b.daemon.shutdown();
}

/// A data frame whose length prefix claims more than the 1 MiB bound is
/// refused before any of it is read into memory: the file fails on the
/// receiver, its part file goes, the data stream closes, and the sender's
/// stream is stopped. The sender here is a bare peer the receiver trusts,
/// since the daemon's own sender never encodes such a frame (ADR 0036).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_data_frame_over_the_bound_fails_the_file_and_leaves_nothing() {
    use lp_core::files::{FilesBegin, FilesMessage, FilesOffer};
    use lp_core::frame::{MAX_FRAME, StreamHeader, encode};
    use lp_crypto::signing::SigningKey;
    use lp_transport::manager::{Manager, open_on};
    use lp_transport::quic::Endpoint;

    let (a, b, alice) = pair_with(2, 2, |dir| DirectoryStore::new(dir)).await;
    // A third binding of Alice's, driven by hand.
    let raw_key = SigningKey::generate().unwrap();
    b.daemon
        .store()
        .insert(&facts::role_fact(&alice, Role::Binding, *raw_key.public(), false, at(9)).unwrap())
        .unwrap();
    let ep = Endpoint::bind("127.0.0.1:0".parse().unwrap(), &raw_key).unwrap();
    let (raw, _raw_events) = Manager::start(ep);
    let conn = raw
        .connect(b.daemon.manager().endpoint().local_addr().unwrap())
        .await
        .unwrap();

    // The control stream: an offer of one 3 MiB file, accepted.
    let file = FileDescriptor::new("big", "big.bin", 3 * 1024 * 1024);
    let mut ctrl = open_on(&conn, FILES_TYPE).await.unwrap();
    ctrl.send(&FilesMessage::Offer(FilesOffer {
        transfer: "t-over".to_owned(),
        files: vec![file],
    }))
    .await
    .unwrap();
    let answer = tokio::time::timeout(Duration::from_secs(10), ctrl.recv::<FilesMessage>())
        .await
        .expect("an answer in time")
        .unwrap();
    assert!(
        matches!(answer, Some(FilesMessage::Accept(_))),
        "accepted: {answer:?}"
    );

    // The data stream by hand: header, begin, then a frame claiming 2 MiB.
    let (mut send, mut recv) = conn.raw().open_bi().await.unwrap();
    send.write_all(&encode(&StreamHeader::new(FILES_TYPE), MAX_FRAME).unwrap())
        .await
        .unwrap();
    send.write_all(
        &encode(
            &FilesMessage::Begin(FilesBegin {
                transfer: "t-over".to_owned(),
                id: "big".to_owned(),
            }),
            MAX_FRAME,
        )
        .unwrap(),
    )
    .await
    .unwrap();
    let claimed = (2 * 1024 * 1024u32).to_be_bytes();
    send.write_all(&claimed).await.unwrap();
    let _ = send.write_all(&[0u8; 4096]).await;

    until("the receiver failed the file", || {
        b.files
            .transfers()
            .iter()
            .any(|t| t.transfer == "t-over" && matches!(t.files[0].state, FileState::Failed { .. }))
    })
    .await;
    let progress = b
        .files
        .transfers()
        .into_iter()
        .find(|t| t.transfer == "t-over")
        .unwrap();
    assert_eq!(progress.files[0].done, 0, "nothing of the frame was taken");
    until("receiver's streams closed", || {
        b.files.open_data_streams() == 0
    })
    .await;
    assert!(
        listing(b.downloads.path()).is_empty(),
        "no part file: {:?}",
        listing(b.downloads.path())
    );
    // The receiver reset the stream with the transport's code for a frame it
    // cannot decode (ADR 0038 decision 1): the sender sees code 4, never a
    // `received`.
    let answer = tokio::time::timeout(Duration::from_secs(10), recv.read_to_end(64 * 1024))
        .await
        .expect("the stream ends in time");
    let text = format!("{answer:?}");
    assert!(
        text.contains("Reset(4)"),
        "expected a reset with MALFORMED_CODE after a bad frame: {text}"
    );
    ctrl.finish();
    raw.shutdown();
    a.daemon.shutdown();
    b.daemon.shutdown();
}
