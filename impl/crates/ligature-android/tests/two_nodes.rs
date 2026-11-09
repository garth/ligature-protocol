//! Two nodes in one process with fake hosts: the first creates the person,
//! the second joins through the ceremony (offer, code, both confirm), then a
//! clipboard change the host reports on one is set through the other's host,
//! and a stop ends it.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ligature_android::{
    CameraInfo, Config, DownloadTarget, DriveAction, DriveScreen, Event, Host, HostClipValue,
    LocalNotification, LogLevel, Node, NotificationCopy, OutgoingFile, PublishedDownload,
    ScreenOffer, TransferInfo,
};

/// A share request as the host hears it: the viewer, the width, height and
/// rate asked, and the channels of sound.
type ShareRequest = (String, u32, u32, u32, u8);

/// A camera request as the host hears it: the viewer, the camera, the
/// width, height and rate asked, and the channels of the microphone.
type CameraRequest = (String, String, u32, u32, u32, u8);

#[derive(Default)]
struct FakeHost {
    clipboard: Mutex<Option<String>>,
    /// The last whole value set, with its items.
    value: Mutex<Option<HostClipValue>>,
    events: Mutex<Vec<Event>>,
    shown: Mutex<Vec<NotificationCopy>>,
    dismissed_copies: Mutex<Vec<(String, String)>>,
    dismissed_own: Mutex<Vec<String>>,
    /// Where this host's downloads land, when it receives files.
    downloads: Option<tempfile::TempDir>,
    /// Pending downloads by handle: the final path.
    pending: Mutex<Vec<(u64, std::path::PathBuf)>>,
    aborted: Mutex<Vec<u64>>,
    /// Frames of a watched share: whether each was a keyframe, and its size.
    screen_frames: Mutex<Vec<(bool, usize)>>,
    /// Packets of a watched share's sound: each one's number and size.
    screen_audio: Mutex<Vec<(u64, usize)>>,
    /// Casts offered to this phone.
    offers: Mutex<Vec<ScreenOffer>>,
    /// Share requests: the viewer, the size and rate asked, and the channels
    /// of sound.
    share_requests: Mutex<Vec<ShareRequest>>,
    share_keyframes: Mutex<u32>,
    share_stops: Mutex<u32>,
    /// Camera requests, and the keyframes and stops asked of the camera.
    camera_requests: Mutex<Vec<CameraRequest>>,
    camera_keyframes: Mutex<u32>,
    camera_stops: Mutex<u32>,
    /// What a drive of this phone asked of it.
    driven: Mutex<Vec<DriveAction>>,
}

impl FakeHost {
    fn with_downloads() -> Self {
        Self {
            downloads: Some(tempfile::tempdir().unwrap()),
            ..Self::default()
        }
    }

    fn transfers(&self) -> Vec<TransferInfo> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                Event::TransferChanged { info } => Some(info.clone()),
                _ => None,
            })
            .collect()
    }

    fn clipboard(&self) -> Option<String> {
        self.clipboard.lock().unwrap().clone()
    }

    fn code(&self) -> Option<String> {
        self.events.lock().unwrap().iter().find_map(|e| match e {
            Event::PairingCode { code } => Some(code.clone()),
            _ => None,
        })
    }

    fn finished(&self) -> Vec<Option<String>> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                Event::PairingFinished { error } => Some(error.clone()),
                _ => None,
            })
            .collect()
    }
}

impl Host for FakeHost {
    fn set_clipboard(&self, text: String) -> bool {
        *self.clipboard.lock().unwrap() = Some(text);
        true
    }

    fn set_clipboard_value(&self, value: HostClipValue) -> bool {
        *self.clipboard.lock().unwrap() = value.text.clone();
        *self.value.lock().unwrap() = Some(value);
        true
    }

    fn show_notification(&self, copy: NotificationCopy) -> bool {
        let mut shown = self.shown.lock().unwrap();
        shown.retain(|c| !(c.origin == copy.origin && c.id == copy.id));
        shown.push(copy);
        true
    }

    fn dismiss_notification_copy(&self, origin: String, id: String) -> bool {
        self.shown
            .lock()
            .unwrap()
            .retain(|c| !(c.origin == origin && c.id == id));
        self.dismissed_copies.lock().unwrap().push((origin, id));
        true
    }

    fn dismiss_own_notification(&self, id: String) -> bool {
        self.dismissed_own.lock().unwrap().push(id);
        true
    }

    fn create_download(&self, name: String, _mime: String, _size: u64) -> Option<DownloadTarget> {
        // A real path rather than a descriptor: the node opens it the same way.
        let dir = self.downloads.as_ref()?;
        let path = dir.path().join(format!("{name}.pending"));
        std::fs::write(&path, b"").ok()?;
        let mut pending = self.pending.lock().unwrap();
        let id = pending.len() as u64 + 1;
        pending.push((id, dir.path().join(&name)));
        Some(DownloadTarget {
            id,
            path: path.to_string_lossy().into_owned(),
        })
    }

    fn finish_download(&self, id: u64) -> Option<PublishedDownload> {
        let final_path = self
            .pending
            .lock()
            .unwrap()
            .iter()
            .find(|(i, _)| *i == id)?
            .1
            .clone();
        let pending = final_path.with_file_name(format!(
            "{}.pending",
            final_path.file_name()?.to_string_lossy()
        ));
        std::fs::rename(&pending, &final_path).ok()?;
        Some(PublishedDownload {
            name: final_path.file_name()?.to_string_lossy().into_owned(),
            location: format!("content://downloads/{id}"),
        })
    }

    fn abort_download(&self, id: u64) {
        self.aborted.lock().unwrap().push(id);
    }

    fn screen_frame(&self, _peer: String, key: bool, bytes: Vec<u8>) {
        self.screen_frames.lock().unwrap().push((key, bytes.len()));
    }

    fn screen_audio(&self, _peer: String, seq: u64, _at: u64, bytes: Vec<u8>) {
        self.screen_audio.lock().unwrap().push((seq, bytes.len()));
    }

    fn screen_share_requested(
        &self,
        viewer: String,
        width: u32,
        height: u32,
        frame_rate: u32,
        audio_channels: u8,
    ) {
        self.share_requests.lock().unwrap().push((
            viewer,
            width,
            height,
            frame_rate,
            audio_channels,
        ));
    }

    fn screen_offered(&self, offer: ScreenOffer) {
        self.offers.lock().unwrap().push(offer);
    }

    fn screen_share_stop(&self) {
        *self.share_stops.lock().unwrap() += 1;
    }

    fn drive(&self, action: DriveAction) {
        self.driven.lock().unwrap().push(action);
    }

    fn drive_screen(&self) -> DriveScreen {
        DriveScreen {
            width: 1000,
            height: 2000,
        }
    }

    fn screen_share_keyframe(&self) {
        *self.share_keyframes.lock().unwrap() += 1;
    }

    fn camera_share_requested(
        &self,
        viewer: String,
        camera: String,
        width: u32,
        height: u32,
        frame_rate: u32,
        audio_channels: u8,
    ) {
        self.camera_requests.lock().unwrap().push((
            viewer,
            camera,
            width,
            height,
            frame_rate,
            audio_channels,
        ));
    }

    fn camera_share_stop(&self) {
        *self.camera_stops.lock().unwrap() += 1;
    }

    fn camera_share_keyframe(&self) {
        *self.camera_keyframes.lock().unwrap() += 1;
    }

    fn on_event(&self, event: Event) {
        self.events.lock().unwrap().push(event);
    }

    fn log(&self, level: LogLevel, target: String, message: String) {
        eprintln!("{level:?} {target}: {message}");
    }
}

fn until(what: &str, mut check: impl FnMut() -> bool) {
    // The two nodes find each other over real mDNS, which is slow when the
    // whole workspace's daemons are advertising at once.
    let deadline = Instant::now() + Duration::from_secs(30);
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for: {what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn config(dir: &tempfile::TempDir, name: &str) -> Config {
    Config {
        data_dir: dir.path().to_string_lossy().into_owned(),
        device_name: name.to_owned(),
        debug_logging: false,
        cache_dir: Some(dir.path().join("cache").to_string_lossy().into_owned()),
    }
}

#[test]
fn a_phone_joins_and_the_clipboard_flows_both_ways() {
    let laptop_dir = tempfile::tempdir().unwrap();
    let phone_dir = tempfile::tempdir().unwrap();
    let laptop_host = Arc::new(FakeHost::with_downloads());
    let phone_host = Arc::new(FakeHost::with_downloads());

    let laptop = Node::start(config(&laptop_dir, "laptop"), laptop_host.clone()).unwrap();
    assert!(!laptop.status().initialised);
    assert!(laptop.devices().is_err(), "no person yet");
    let person = laptop.init("Alice".to_owned()).unwrap();
    let s = laptop.status();
    assert!(s.initialised);
    assert_eq!(s.person.as_deref(), Some(person.as_str()));
    assert_eq!(s.name.as_deref(), Some("Alice"));
    assert_eq!(s.device_name, "laptop");
    assert!(laptop.init("Bob".to_owned()).is_err(), "once only");
    assert_eq!(laptop.devices().unwrap().len(), 2, "binding and principal");

    // The phone is new: it shows an offer, the laptop introduces it.
    let phone = Node::start(config(&phone_dir, "phone"), phone_host.clone()).unwrap();
    assert!(phone.start_introduce("x".to_owned(), true).is_err());
    let joining = phone.start_join().unwrap();
    until("offer", || joining.offer().is_some());
    let offer = joining.offer().unwrap();
    assert!(offer.starts_with("ligature:pair/1?"));
    // One ceremony at a time: a second one ends at once, the first goes on.
    let second = phone.start_join().unwrap();
    until("second ceremony refused", || second.is_finished());
    assert!(second.error().unwrap().contains("already in progress"));
    assert!(!joining.is_finished());

    let introducing = laptop.start_introduce(offer, true).unwrap();
    until("codes", || {
        laptop_host.code().is_some() && phone_host.code().is_some()
    });
    assert_eq!(
        laptop_host.code(),
        phone_host.code(),
        "same code both sides"
    );
    introducing.confirm();
    joining.confirm();
    until("both finished", || {
        introducing.is_finished() && joining.is_finished()
    });
    assert_eq!(introducing.error(), None);
    assert_eq!(joining.error(), None);
    assert!(
        phone_host.finished().contains(&None),
        "the host was told the ceremony succeeded"
    );
    assert!(phone.status().initialised);
    assert_eq!(phone.status().person.as_deref(), Some(person.as_str()));
    until("laptop lists the phone", || {
        laptop.devices().unwrap().len() == 4
    });
    until("phone lists the laptop", || {
        phone.devices().unwrap().len() == 4
    });

    // Each offers the clipboard to the other.
    let laptop_key = laptop.status().binding;
    let phone_key = phone.status().binding;
    until("laptop sees the phone's clipboard", || {
        laptop.status().clipboard_peers == vec![phone_key.clone()]
    });
    until("phone sees the laptop's clipboard", || {
        phone.status().clipboard_peers == vec![laptop_key.clone()]
    });

    // The phone's host saw a copy: the laptop's host is asked to set it.
    phone.clipboard_changed("from the phone".to_owned());
    until("laptop clipboard", || {
        laptop_host.clipboard().as_deref() == Some("from the phone")
    });
    assert_eq!(laptop.clipboard_text().as_deref(), Some("from the phone"));
    // And back.
    laptop.clipboard_changed("from the laptop".to_owned());
    until("phone clipboard", || {
        phone_host.clipboard().as_deref() == Some("from the laptop")
    });
    assert!(
        phone_host
            .events
            .lock()
            .unwrap()
            .contains(&Event::ClipboardReceived)
    );

    // Notifications: the phone's listener connects and an app posts; the
    // laptop's host shows a copy with the icon and the phone's name.
    phone.set_notifications_emitting(true);
    until("laptop sees the phone emits", || {
        laptop.daemon().registry().peers().iter().any(|p| {
            p.capabilities.iter().any(|d| {
                lp_core::capability::NotificationsParams::from_descriptor(d)
                    .is_some_and(|params| params.emits)
            })
        })
    });
    phone.notification_posted(LocalNotification {
        id: "0|org.example.chat|1".to_owned(),
        app_id: "org.example.chat".to_owned(),
        app_name: "Chat".to_owned(),
        title: Some("Bob".to_owned()),
        body: Some("lunch?".to_owned()),
        posted_at_unix: 1_760_000_000,
        icon_png: Some(vec![0x89, b'P', b'N', b'G', 9, 9]),
    });
    until("laptop shows the copy", || {
        !laptop_host.shown.lock().unwrap().is_empty()
    });
    let copy = laptop_host.shown.lock().unwrap()[0].clone();
    assert_eq!(copy.origin, phone_key);
    assert_eq!(copy.origin_name, "phone");
    assert_eq!(copy.app_name, "Chat");
    assert_eq!(copy.body.as_deref(), Some("lunch?"));
    assert_eq!(
        copy.icon_png.as_deref(),
        Some(&[0x89, b'P', b'N', b'G', 9, 9][..])
    );
    assert!(
        phone_host.shown.lock().unwrap().is_empty(),
        "no copy of its own"
    );

    // The laptop's user dismisses the copy: the phone cancels its own.
    laptop
        .notification_copy_dismissed(phone_key.clone(), copy.id.clone())
        .unwrap();
    until("phone cancels its own", || {
        phone_host.dismissed_own.lock().unwrap().as_slice() == [copy.id.clone()]
    });
    assert!(
        laptop
            .notification_copy_dismissed("junk".to_owned(), "1".to_owned())
            .is_err()
    );

    // And the other way: the laptop emits, the phone shows, the laptop's
    // notification is withdrawn and the phone's copy clears.
    laptop.set_notifications_emitting(true);
    until("phone sees the laptop emits", || {
        phone.daemon().registry().peers().iter().any(|p| {
            p.capabilities.iter().any(|d| {
                lp_core::capability::NotificationsParams::from_descriptor(d)
                    .is_some_and(|params| params.emits)
            })
        })
    });
    laptop.notification_posted(LocalNotification {
        id: "77".to_owned(),
        app_id: "mail".to_owned(),
        app_name: "Mail".to_owned(),
        title: Some("Invoice".to_owned()),
        body: None,
        posted_at_unix: 1_760_000_001,
        icon_png: None,
    });
    until("phone shows the laptop's", || {
        phone_host
            .shown
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.id == "77" && c.origin == laptop_key && c.icon_png.is_none())
    });
    laptop.notification_removed("77".to_owned());
    until("phone clears the copy", || {
        phone_host.dismissed_copies.lock().unwrap().as_slice()
            == [(laptop_key.clone(), "77".to_owned())]
    });

    // Files: the phone's host hands the node two opened files; they land
    // through the laptop's host, and both hosts see the transfer move and end.
    until("laptop receives files", || {
        phone.status().file_peers == vec![laptop_key.clone()]
    });
    let src = tempfile::tempdir().unwrap();
    let big: Vec<u8> = (0..700_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(src.path().join("big.bin"), &big).unwrap();
    std::fs::write(src.path().join("note.txt"), b"hi").unwrap();
    assert!(phone.send_files("junk".to_owned(), vec![]).is_err());
    let transfer = phone
        .send_files(
            laptop_key.clone(),
            vec![
                OutgoingFile {
                    path: src.path().join("big.bin").to_string_lossy().into_owned(),
                    name: "big.bin".to_owned(),
                    size: big.len() as u64,
                    mime: String::new(),
                    modified: None,
                },
                OutgoingFile {
                    path: src.path().join("note.txt").to_string_lossy().into_owned(),
                    name: "note.txt".to_owned(),
                    size: 2,
                    mime: "text/plain".to_owned(),
                    modified: None,
                },
            ],
        )
        .unwrap();
    until("transfer ended on both hosts", || {
        let ended = |h: &FakeHost| {
            h.transfers()
                .iter()
                .any(|t| t.transfer == transfer && t.outcome.is_some())
        };
        ended(&phone_host) && ended(&laptop_host)
    });
    let sent = phone_host
        .transfers()
        .into_iter()
        .rfind(|t| t.transfer == transfer)
        .unwrap();
    assert!(sent.outgoing && sent.all_received, "{sent:?}");
    assert_eq!(sent.outcome.as_deref(), Some("complete"));
    assert_eq!(sent.peer_name, "laptop");
    assert_eq!(sent.files_total, 2);
    assert_eq!(sent.done_bytes, big.len() as u64 + 2);
    let received = laptop_host
        .transfers()
        .into_iter()
        .rfind(|t| t.transfer == transfer)
        .unwrap();
    assert!(!received.outgoing && received.all_received, "{received:?}");
    assert_eq!(received.peer_name, "phone");
    assert_eq!(received.locations.len(), 2);
    let landed = laptop_host.downloads.as_ref().unwrap().path();
    assert_eq!(std::fs::read(landed.join("big.bin")).unwrap(), big);
    assert_eq!(std::fs::read(landed.join("note.txt")).unwrap(), b"hi");
    assert!(laptop_host.aborted.lock().unwrap().is_empty());
    assert!(phone.transfers().is_empty() && laptop.transfers().is_empty());
    assert!(!phone.cancel_transfer(transfer.clone()));

    // Milestone 5: a rich value from the phone (text, HTML, a 1 MiB image)
    // reaches the laptop's host as a clip with the image promised, fetched
    // only when the provider is asked for it.
    let png: Vec<u8> = (0..1024 * 1024u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(src.path().join("shot.png"), &png).unwrap();
    phone.clipboard_changed_value(
        Some("a shot".to_owned()),
        Some("<b>a shot</b>".to_owned()),
        vec![OutgoingFile {
            path: src.path().join("shot.png").to_string_lossy().into_owned(),
            name: "shot.png".to_owned(),
            size: png.len() as u64,
            mime: "image/png".to_owned(),
            modified: None,
        }],
    );
    until("rich value on the laptop", || {
        laptop_host
            .value
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|v| v.text.as_deref() == Some("a shot") && v.items.len() == 1)
    });
    let value = laptop_host.value.lock().unwrap().clone().unwrap();
    assert_eq!(value.html.as_deref(), Some("<b>a shot</b>"));
    let item = value.items[0].clone();
    assert_eq!(item.mime, "image/png");
    assert!(!item.file);
    assert!(item.key.starts_with("promise-"), "{}", item.key);
    assert_eq!(item.size, png.len() as u64);
    assert!(
        laptop.transfers().is_empty(),
        "nothing fetched before a paste"
    );
    let data = laptop.clipboard_item(item.key.clone()).unwrap();
    let bytes = match (data.bytes, data.path) {
        (Some(b), _) => b,
        (None, Some(p)) => std::fs::read(p).unwrap(),
        (None, None) => panic!("fetched nothing"),
    };
    assert_eq!(bytes, png);
    assert!(laptop.clipboard_item("nope".to_owned()).is_none());

    // A file selection copied on the phone: the laptop's host sees a file
    // item and Save fetches it into its downloads.
    std::fs::write(src.path().join("memo.md"), b"memo!").unwrap();
    phone.clipboard_changed_value(
        Some("memo.md".to_owned()),
        None,
        vec![OutgoingFile {
            path: src.path().join("memo.md").to_string_lossy().into_owned(),
            name: "memo.md".to_owned(),
            size: 5,
            mime: "text/markdown".to_owned(),
            modified: None,
        }],
    );
    until("file promised on the laptop", || {
        laptop_host
            .value
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|v| v.items.iter().any(|i| i.file && i.name == "memo.md"))
    });
    assert_eq!(
        laptop_host.value.lock().unwrap().as_ref().unwrap().items[0].mime,
        "text/markdown"
    );
    let saved = laptop.save_clipboard_files().unwrap().unwrap();
    until("saved into downloads", || {
        laptop_host
            .transfers()
            .iter()
            .any(|t| t.transfer == saved && t.outcome.is_some())
    });
    assert_eq!(std::fs::read(landed.join("memo.md")).unwrap(), b"memo!");
    assert_eq!(laptop.request_clipboard(), 1);

    // Revoking the phone from the laptop drops it.
    laptop.revoke(phone_key.clone()).unwrap();
    until("phone marked revoked", || {
        laptop
            .devices()
            .unwrap()
            .iter()
            .any(|d| d.key == phone_key && d.revoked)
    });
    assert!(laptop.revoke("not a key".to_owned()).is_err());

    phone.stop();
    laptop.stop();
}

/// The phone drives a desktop as a touchpad (milestone 7, LP-145): the
/// desktop is a plain daemon with the in-memory injector, the identities are
/// built as the daemon tests build them, and the node's exported calls run
/// the drive end to end.
#[test]
fn the_phone_is_a_touchpad_for_a_desktop() {
    use ligature_android::{BTN_LEFT, TouchEvent};
    use ligatured::input::FakeInject;
    use lp_core::facts::Role;
    use lp_core::input::{INPUT_TYPE, InputEvent};
    use lp_core::paths::Locations;
    use lp_core::wire::Timestamp;
    use lp_identity::cross_signing::PersonKeys;
    use lp_identity::facts;

    let at = |offset: i64| Timestamp::from_unix(1_760_000_000 + offset).unwrap();
    let phone_dir = tempfile::tempdir().unwrap();
    let phone_host = Arc::new(FakeHost::with_downloads());
    let phone = Node::start(config(&phone_dir, "phone"), phone_host.clone()).unwrap();

    // The desktop: a daemon of the same person with the fake injector, run
    // on a runtime of its own.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let desktop_dir = tempfile::tempdir().unwrap();
    let inject = FakeInject::new(2000, 1000);
    let (desktop, addr) = {
        let _guard = runtime.enter();
        let loc = Locations::resolve(Some(desktop_dir.path()), None);
        let (d, events) =
            ligatured::daemon::Daemon::open(loc, "127.0.0.1:0".parse().unwrap(), Some("desktop"))
                .unwrap();
        d.attach_input(None, Some(inject.clone()));
        ligatured::run::spawn_connection_events(d.clone(), events);
        let addr = d.manager().endpoint().local_addr().unwrap();
        (d, addr)
    };
    let alice = PersonKeys::generate().unwrap();
    let person = facts::person_fact(&alice, "Alice", 1, at(0)).unwrap();
    let phone_role = facts::role_fact(
        &alice,
        Role::Binding,
        *phone.daemon().binding().public(),
        true,
        at(1),
    )
    .unwrap();
    let desktop_role = facts::role_fact(
        &alice,
        Role::Binding,
        *desktop.binding().public(),
        true,
        at(2),
    )
    .unwrap();
    for d in [phone.daemon(), &desktop] {
        d.store().insert(&person).unwrap();
        d.store().insert(&phone_role).unwrap();
        d.store().insert(&desktop_role).unwrap();
        d.update_keys(|k| k.set_joined(*alice.person(), Some(&alice.self_signing)))
            .unwrap();
    }
    let dial = phone.daemon().clone();
    runtime
        .block_on(async move { dial.manager().connect(addr).await })
        .unwrap();
    until("the phone sees the desktop's input", || {
        phone.daemon().registry().peers_offering(INPUT_TYPE).len() == 1
    });
    until("the desktop sees the phone's input", || {
        desktop.registry().peers_offering(INPUT_TYPE).len() == 1
    });

    // The picker: the desktop accepts and types text; the phone offers only.
    let targets = phone.input_targets();
    assert_eq!(targets.len(), 1, "{targets:?}");
    assert_eq!(targets[0].key, desktop.binding().public().to_string());
    // No device record is stored in this test, so the name falls back to
    // the fingerprint.
    assert_eq!(targets[0].name, desktop.binding().public().fingerprint());
    assert!(targets[0].text);
    assert!(phone.input_driving().is_none());

    // Drive: an absolute entry at the middle, then a move, a tap, a scroll,
    // text and a key.
    let screen = phone.input_start(targets[0].key.clone()).unwrap();
    assert_eq!((screen.width, screen.height), (2000, 1000));
    assert_eq!(phone.input_driving(), Some(targets[0].key.clone()));
    until("the desktop is driven", || {
        inject.begun() == vec![(0.5, 0.5)]
    });
    assert!(
        phone.input_start(targets[0].key.clone()).is_err(),
        "one drive at a time"
    );
    phone
        .input_send(vec![
            TouchEvent::Motion {
                dx: 30.0,
                dy: -10.0,
            },
            TouchEvent::Button {
                code: BTN_LEFT,
                pressed: true,
            },
            TouchEvent::Button {
                code: BTN_LEFT,
                pressed: false,
            },
            TouchEvent::Scroll { dx: 0.0, dy: 2.0 },
            TouchEvent::Text {
                text: "héllo".to_owned(),
            },
            TouchEvent::Key {
                code: 28,
                pressed: true,
            },
            TouchEvent::Key {
                code: 28,
                pressed: false,
            },
        ])
        .unwrap();
    until("seven events injected", || inject.injected().len() == 7);
    let injected = inject.injected();
    assert_eq!(
        injected[0],
        InputEvent::Motion {
            dx: 30.0,
            dy: -10.0
        }
    );
    assert_eq!(
        injected[3],
        InputEvent::Scroll {
            dx: 0.0,
            dy: 2.0,
            discrete: true
        }
    );
    assert_eq!(
        injected[4],
        InputEvent::Text {
            text: "héllo".to_owned()
        }
    );
    phone.input_send(Vec::new()).unwrap();

    // Leaving the screen ends the drive: the desktop lifts everything and
    // the host hears that it ended.
    phone.input_stop();
    until("the desktop lifted everything", || inject.ends() == 1);
    assert!(phone.input_driving().is_none());
    until("the host was told", || {
        phone_host
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::InputEnded { peer } if *peer == targets[0].key))
    });
    assert!(
        phone
            .input_send(vec![TouchEvent::Motion { dx: 1.0, dy: 1.0 }])
            .is_err()
    );

    // The desktop going away mid-drive ends it too.
    phone.input_start(targets[0].key.clone()).unwrap();
    until("driven again", || inject.begun().len() == 2);
    desktop.shutdown();
    until("the drive ended with the connection", || {
        phone.input_driving().is_none()
    });
    phone.stop();
    runtime.shutdown_timeout(Duration::from_secs(2));
}

/// A desktop drives the phone (LP-167): only once the app's accessibility
/// service is on, and what it sends becomes actions for the service.
#[test]
fn a_desktop_drives_the_phone_once_its_service_is_on() {
    use ligature_android::BTN_LEFT;
    use ligatured::input::Touchpad;
    use lp_core::facts::Role;
    use lp_core::input::{Enter, INPUT_TYPE, InputEvent, InputParams};
    use lp_core::paths::Locations;
    use lp_core::wire::Timestamp;
    use lp_identity::cross_signing::PersonKeys;
    use lp_identity::facts;

    let at = |offset: i64| Timestamp::from_unix(1_760_000_000 + offset).unwrap();
    let phone_dir = tempfile::tempdir().unwrap();
    let phone_host = Arc::new(FakeHost::with_downloads());
    let phone = Node::start(config(&phone_dir, "phone"), phone_host.clone()).unwrap();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let desktop_dir = tempfile::tempdir().unwrap();
    let (desktop, input, addr) = {
        let _guard = runtime.enter();
        let loc = Locations::resolve(Some(desktop_dir.path()), None);
        let (d, events) =
            ligatured::daemon::Daemon::open(loc, "127.0.0.1:0".parse().unwrap(), Some("desktop"))
                .unwrap();
        let input = d.attach_input(Some(Touchpad::new()), None);
        ligatured::run::spawn_connection_events(d.clone(), events);
        let addr = d.manager().endpoint().local_addr().unwrap();
        (d, input, addr)
    };
    let alice = PersonKeys::generate().unwrap();
    let person = facts::person_fact(&alice, "Alice", 1, at(0)).unwrap();
    let phone_role = facts::role_fact(
        &alice,
        Role::Binding,
        *phone.daemon().binding().public(),
        true,
        at(1),
    )
    .unwrap();
    let desktop_role = facts::role_fact(
        &alice,
        Role::Binding,
        *desktop.binding().public(),
        true,
        at(2),
    )
    .unwrap();
    for d in [phone.daemon(), &desktop] {
        d.store().insert(&person).unwrap();
        d.store().insert(&phone_role).unwrap();
        d.store().insert(&desktop_role).unwrap();
        d.update_keys(|k| k.set_joined(*alice.person(), Some(&alice.self_signing)))
            .unwrap();
    }
    let dial = phone.daemon().clone();
    runtime
        .block_on(async move { dial.manager().connect(addr).await })
        .unwrap();
    let phone_key = *phone.daemon().binding().public();
    let phone_params = || {
        desktop
            .registry()
            .peers()
            .into_iter()
            .find(|caps| caps.peer == phone_key)
            .and_then(|caps| {
                caps.capabilities
                    .iter()
                    .find_map(InputParams::from_descriptor)
            })
    };
    until("the desktop sees the phone's input", || {
        desktop.registry().peers_offering(INPUT_TYPE).len() == 1 && phone_params().is_some()
    });
    assert!(
        !phone_params().unwrap().accepts,
        "with the service off the phone only provides"
    );
    assert!(
        runtime
            .block_on(input.drive(phone_key, Enter::absolute(0.5, 0.5)))
            .is_err(),
        "a drive is refused with the service off"
    );
    assert!(phone_host.driven.lock().unwrap().is_empty());

    phone.set_drivable(true, true);
    until("the phone offers to be driven", || {
        phone_params().is_some_and(|p| p.accepts && p.absolute && p.text)
    });
    let ready = runtime
        .block_on(input.drive(phone_key, Enter::absolute(0.5, 0.5)))
        .unwrap();
    assert_eq!((ready.width, ready.height), (1000, 2000));
    runtime
        .block_on(input.send(
            vec![
                InputEvent::Position { x: 0.1, y: 0.1 },
                InputEvent::Button {
                    code: BTN_LEFT,
                    pressed: true,
                },
                InputEvent::Text {
                    text: "hi".to_owned(),
                },
            ],
            None,
        ))
        .unwrap();
    until("the events arrive as actions", || {
        phone_host.driven.lock().unwrap().len() == 4
    });
    assert_eq!(
        phone_host.driven.lock().unwrap().clone(),
        vec![
            DriveAction::Begin {
                x: 500.0,
                y: 1000.0
            },
            DriveAction::Pointer { x: 100.0, y: 200.0 },
            DriveAction::TouchDown { x: 100.0, y: 200.0 },
            DriveAction::Text {
                text: "hi".to_owned()
            },
        ]
    );

    // The desktop going away lifts the finger it left down.
    desktop.shutdown();
    until("the drive ended with the connection", || {
        phone_host.driven.lock().unwrap().last() == Some(&DriveAction::End)
    });
    assert!(
        phone_host
            .driven
            .lock()
            .unwrap()
            .contains(&DriveAction::TouchUp { x: 100.0, y: 200.0 })
    );
    phone.stop();
    runtime.shutdown_timeout(Duration::from_secs(2));
}

/// The phone watches a desktop's screen and shares its own (milestone 8,
/// LP-151): the desktop is a daemon with the in-memory capture; the phone's
/// capture is the host's, answered by the test as the app would.
#[test]
fn the_phone_watches_a_desktop_and_shares_its_own_screen() {
    use ligatured::screen::{
        AudioPacket, CastSource, EncodedFrame, FakeCapture, ShareControl, Viewed,
    };
    use lp_core::facts::Role;
    use lp_core::paths::Locations;
    use lp_core::screen::{AudioFormat, Display, SCREEN_TYPE, ScreenParams, Start, StopReason};
    use lp_core::wire::Timestamp;
    use lp_identity::cross_signing::PersonKeys;
    use lp_identity::facts;

    let at = |offset: i64| Timestamp::from_unix(1_760_000_000 + offset).unwrap();
    let phone_dir = tempfile::tempdir().unwrap();
    let phone_host = Arc::new(FakeHost::with_downloads());
    let phone = Node::start(config(&phone_dir, "phone"), phone_host.clone()).unwrap();
    // The app has an encoder, a 1080x2400 screen and an Opus encoder.
    phone.set_screen_share(true, 1080, 2400, 2.625, true);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let desktop_dir = tempfile::tempdir().unwrap();
    let capture = FakeCapture::new(vec![Display {
        id: "DP-1".to_owned(),
        name: None,
        width: 2560,
        height: 1440,
        primary: true,
        scale: None,
    }]);
    let (desktop, desktop_sync, addr) = {
        let _guard = runtime.enter();
        let loc = Locations::resolve(Some(desktop_dir.path()), None);
        let (d, events) =
            ligatured::daemon::Daemon::open(loc, "127.0.0.1:0".parse().unwrap(), Some("desktop"))
                .unwrap();
        let sync = d.attach_screen(
            Some(capture.clone()),
            Some(ScreenParams {
                views: true,
                audio_decodes: vec![AudioFormat::OPUS_STEREO],
                ..ScreenParams::default()
            }),
        );
        ligatured::run::spawn_connection_events(d.clone(), events);
        let addr = d.manager().endpoint().local_addr().unwrap();
        (d, sync, addr)
    };
    let alice = PersonKeys::generate().unwrap();
    let person = facts::person_fact(&alice, "Alice", 1, at(0)).unwrap();
    let roles = [
        facts::role_fact(
            &alice,
            Role::Binding,
            *phone.daemon().binding().public(),
            true,
            at(1),
        )
        .unwrap(),
        facts::role_fact(
            &alice,
            Role::Binding,
            *desktop.binding().public(),
            true,
            at(2),
        )
        .unwrap(),
    ];
    for d in [phone.daemon(), &desktop] {
        d.store().insert(&person).unwrap();
        for r in &roles {
            d.store().insert(r).unwrap();
        }
        d.update_keys(|k| k.set_joined(*alice.person(), Some(&alice.self_signing)))
            .unwrap();
    }
    let dial = phone.daemon().clone();
    runtime
        .block_on(async move { dial.manager().connect(addr).await })
        .unwrap();
    until("both see each other's screen", || {
        phone.daemon().registry().peers_offering(SCREEN_TYPE).len() == 1
            && desktop.registry().peers_offering(SCREEN_TYPE).len() == 1
            && desktop.registry().peers().iter().any(|p| {
                p.capabilities
                    .iter()
                    .any(|d| ScreenParams::from_descriptor(d).is_some_and(|p| p.shares))
            })
    });

    // The phone watches the desktop, fitted to what it takes.
    let targets = phone.screen_targets();
    assert_eq!(targets.len(), 1);
    let desktop_key = desktop.binding().public().to_string();
    assert_eq!(targets[0].key, desktop_key);
    let watch = phone
        .screen_watch(desktop_key.clone(), 1280, 720, 30, false)
        .unwrap();
    assert_eq!(
        (
            watch.display.as_str(),
            watch.width,
            watch.height,
            watch.frame_rate,
            watch.audio_channels
        ),
        ("DP-1", 1280, 720, 30, 0)
    );
    assert!(
        phone
            .screen_watch(desktop_key.clone(), 1280, 720, 30, false)
            .is_err(),
        "one at a time"
    );
    until("the desktop's capture runs", || {
        capture.running() == vec!["DP-1".to_owned()]
    });
    assert!(capture.emit(
        "DP-1",
        EncodedFrame {
            at: 1,
            key: true,
            bytes: vec![1; 300_000],
        },
    ));
    capture.emit(
        "DP-1",
        EncodedFrame {
            at: 2,
            key: false,
            bytes: vec![2; 1000],
        },
    );
    until("two frames reached the host", || {
        phone_host.screen_frames.lock().unwrap().len() == 2
    });
    assert_eq!(
        *phone_host.screen_frames.lock().unwrap(),
        vec![(true, 300_000), (false, 1000)]
    );
    phone.screen_watch_keyframe();
    until("the keyframe request reached the desktop", || {
        capture.keyframe_requests() == 1
    });
    phone.screen_watch_stop();
    until("the host was told the watch ended", || {
        phone_host.events.lock().unwrap().iter().any(|e| {
            matches!(e, Event::ScreenEnded { peer, reason } if *peer == desktop_key && reason == "viewer")
        })
    });
    until("the desktop stopped capturing", || capture.stops() == 1);

    // The desktop watches the phone: the app is asked, consents, encodes.
    let phone_key = *phone.daemon().binding().public();
    // The phone lists its screen with Android's density as its scale (ADR 0045).
    let listed = {
        let sync = desktop_sync.clone();
        runtime
            .block_on(async move { sync.displays(&phone_key).await })
            .unwrap()
    };
    assert_eq!(listed.len(), 1);
    assert_eq!(
        (listed[0].width, listed[0].height, listed[0].scale()),
        (1080, 2400, 2.625)
    );
    let start = Start {
        display: None,
        window: None,
        formats: Vec::new(),
        width: 1080,
        height: 2400,
        frame_rate: 30,
        audio: Vec::new(),
    };
    let watch = {
        let sync = desktop_sync.clone();
        let start = start.clone();
        runtime.spawn(async move { sync.watch(&phone_key, start).await })
    };
    until("the app was asked", || {
        phone_host.share_requests.lock().unwrap().len() == 1
    });
    let (viewer, w, h, rate, channels) = phone_host.share_requests.lock().unwrap()[0].clone();
    assert_eq!(viewer, desktop.binding().public().fingerprint());
    // A 1080x2400 screen fitted within the desktop viewer's 1920x1080, and
    // no sound, since the viewer asked for none.
    assert_eq!((w, h, rate, channels), (486, 1080, 30, 0));
    phone.screen_share_started(true);
    let mut watching = runtime.block_on(watch).unwrap().unwrap();
    assert_eq!(watching.started.display.as_deref(), Some("screen"));
    phone.screen_share_frame(true, vec![9; 50_000]);
    phone.screen_share_frame(false, vec![8; 500]);
    let frames = runtime.block_on(async {
        let mut got = Vec::new();
        while got.len() < 2 {
            match watching.frames.recv().await.unwrap() {
                Viewed::Frame { key, bytes, .. } => got.push((key, bytes.len())),
                Viewed::Audio { .. } => {}
                Viewed::Stopped(r) => panic!("stopped: {r}"),
            }
        }
        got
    });
    assert_eq!(frames, vec![(true, 50_000), (false, 500)]);
    watching.control.send(ShareControl::Keyframe).unwrap();
    until("the app was asked for a keyframe", || {
        *phone_host.share_keyframes.lock().unwrap() == 1
    });
    // The user stops the projection from the phone's notification shade.
    phone.screen_share_ended("consent".to_owned());
    let reason = runtime.block_on(async {
        loop {
            if let Viewed::Stopped(r) = watching.frames.recv().await.unwrap() {
                return r;
            }
        }
    });
    assert_eq!(reason, StopReason::Consent);
    until("the app was told to stop", || {
        *phone_host.share_stops.lock().unwrap() == 1
    });

    // The user refuses the next one.
    let refused = {
        let sync = desktop_sync.clone();
        runtime.spawn(async move { sync.watch(&phone_key, start).await })
    };
    until("the app was asked again", || {
        phone_host.share_requests.lock().unwrap().len() == 2
    });
    phone.screen_share_started(false);
    let err = runtime.block_on(refused).unwrap().unwrap_err();
    assert!(err.to_string().contains("consent"), "{err}");
    // A request that ends unanswered or refused is withdrawn from the app, so
    // Android's dialog does not outlive the viewer who asked.
    until("the app was told the request is over", || {
        *phone_host.share_stops.lock().unwrap() == 2
    });

    // Milestone 9 (LP-202). The desktop has sound to give: the phone asks
    // for it with the picture and the packets reach the host.
    capture.set_audio_encodes(vec![AudioFormat::OPUS_STEREO]);
    {
        let sync = desktop_sync.clone();
        runtime.block_on(async move { sync.advertise().await });
    }
    until("the phone sees the desktop's sound", || {
        phone.daemon().registry().peers().iter().any(|p| {
            p.capabilities.iter().any(|d| {
                ScreenParams::from_descriptor(d).is_some_and(|p| !p.audio_encodes.is_empty())
            })
        })
    });
    let watch = phone
        .screen_watch(desktop_key.clone(), 1280, 720, 30, true)
        .unwrap();
    assert_eq!(watch.audio_channels, 2);
    until("the desktop's capture runs again", || {
        capture.running() == vec!["DP-1".to_owned()]
    });
    assert!(capture.emit_audio(
        "DP-1",
        AudioPacket {
            at: 5,
            bytes: vec![7; 40],
        },
    ));
    until("a packet of sound reached the host", || {
        *phone_host.screen_audio.lock().unwrap() == vec![(0, 40)]
    });
    phone.screen_watch_stop();
    let ended = |n: usize| {
        phone_host
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, Event::ScreenEnded { .. }))
            .count()
            == n
    };
    until("the second watch ended", || ended(2));

    // The desktop casts to the phone: the app is told of the offer and takes
    // it, and the share runs as a watch does.
    assert!(
        phone.screen_cast_targets().is_empty(),
        "the desktop does not receive yet"
    );
    let cast = {
        let sync = desktop_sync.clone();
        runtime.spawn(async move { sync.cast(&phone_key, CastSource::default(), true).await })
    };
    until("the app was told of the offer", || {
        phone_host.offers.lock().unwrap().len() == 1
    });
    let offer = phone_host.offers.lock().unwrap()[0].clone();
    assert_eq!(offer.peer, desktop_key);
    // No device record names the desktop in this test, so its fingerprint
    // stands in, as everywhere a name is shown.
    assert_eq!(offer.peer_name, desktop.binding().public().fingerprint());
    assert_eq!(
        (
            offer.kind.as_str(),
            offer.name.as_str(),
            offer.width,
            offer.height,
            offer.audio
        ),
        ("display", "DP-1", 2560, 1440, true)
    );
    let watch = phone
        .screen_offer_accept(desktop_key.clone(), true)
        .unwrap();
    assert_eq!((watch.display.as_str(), watch.audio_channels), ("DP-1", 2));
    let casting = runtime.block_on(cast).unwrap().unwrap();
    assert_eq!(casting.started.audio, Some(AudioFormat::OPUS_STEREO));
    phone.screen_watch_stop();
    assert_eq!(runtime.block_on(casting.ended).unwrap(), StopReason::Viewer);
    until("the received cast ended", || ended(3));
    // And one the app declines.
    let cast = {
        let sync = desktop_sync.clone();
        runtime.spawn(async move { sync.cast(&phone_key, CastSource::default(), false).await })
    };
    until("the app was told of the second offer", || {
        phone_host.offers.lock().unwrap().len() == 2
    });
    assert!(!phone_host.offers.lock().unwrap()[1].audio);
    phone.screen_offer_decline(desktop_key.clone()).unwrap();
    let err = runtime.block_on(cast).unwrap().unwrap_err();
    assert!(err.to_string().contains("declined"), "{err}");
    assert!(phone.screen_offer_decline(desktop_key.clone()).is_err());

    // The phone casts to the desktop, once something there shows casts: the
    // app is asked for the projection with sound, and frames and packets
    // flow to whoever took the offer.
    let (taken_tx, taken) = std::sync::mpsc::channel();
    {
        let (sync, handle) = (desktop_sync.clone(), runtime.handle().clone());
        desktop_sync.set_on_offer(Arc::new(move |peer, _offer| {
            let (sync, taken_tx) = (sync.clone(), taken_tx.clone());
            handle.spawn(async move {
                let _ = taken_tx.send(sync.accept(&peer, true).await);
            });
        }));
        let sync = desktop_sync.clone();
        runtime.block_on(async move { sync.advertise().await });
    }
    until("the phone sees the desktop receive", || {
        phone.screen_cast_targets().len() == 1
    });
    assert_eq!(phone.screen_cast_targets()[0].key, desktop_key);
    phone.screen_cast(desktop_key.clone(), true).unwrap();
    until("the app was asked for the projection", || {
        phone_host.share_requests.lock().unwrap().len() == 3
    });
    assert_eq!(
        phone_host.share_requests.lock().unwrap()[2].4,
        2,
        "with two channels of sound"
    );
    phone.screen_share_started(true);
    let mut received = taken
        .recv_timeout(Duration::from_secs(10))
        .expect("the desktop took the cast")
        .unwrap();
    assert_eq!(received.started.display.as_deref(), Some("screen"));
    assert_eq!(received.started.audio, Some(AudioFormat::OPUS_STEREO));
    until("the app was told the cast runs", || {
        phone_host
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::ScreenCastStarted { peer } if *peer == desktop_key))
    });
    phone.screen_share_audio(vec![4; 30]);
    phone.screen_share_frame(true, vec![3; 100]);
    let (mut frames, mut packets) = (0, 0);
    runtime.block_on(async {
        while frames == 0 || packets == 0 {
            match received.frames.recv().await.unwrap() {
                Viewed::Frame { bytes, .. } => {
                    assert_eq!(bytes.len(), 100);
                    frames += 1;
                }
                Viewed::Audio { bytes, .. } => {
                    assert_eq!(bytes.len(), 30);
                    packets += 1;
                }
                Viewed::Stopped(r) => panic!("stopped: {r}"),
            }
        }
    });
    // The user stops the cast on the phone.
    phone.screen_cast_stop();
    let reason = runtime.block_on(async {
        loop {
            if let Viewed::Stopped(r) = received.frames.recv().await.unwrap() {
                return r;
            }
        }
    });
    assert_eq!(reason, StopReason::Sharer);
    until("the app was told the cast ended", || {
        phone_host.events.lock().unwrap().iter().any(|e| {
            matches!(e, Event::ScreenCastEnded { peer, reason } if *peer == desktop_key && reason == "sharer")
        })
    });

    phone.stop();
    desktop.shutdown();
    runtime.shutdown_timeout(Duration::from_secs(2));
}

#[test]
fn the_phone_shares_and_casts_its_cameras_and_watches_a_desktop_s() {
    use ligatured::screen::{
        AudioPacket, CastSource, EncodedFrame, FakeCapture, FakePresenter, ShareControl, Viewed,
    };
    use lp_core::camera::{CAMERA_TYPE, Camera, Facing, camera_params};
    use lp_core::facts::Role;
    use lp_core::paths::Locations;
    use lp_core::screen::{AudioFormat, Format, SCREEN_TYPE, ScreenParams, Start, StopReason};
    use lp_core::wire::Timestamp;
    use lp_identity::cross_signing::PersonKeys;
    use lp_identity::facts;

    let at = |offset: i64| Timestamp::from_unix(1_760_000_000 + offset).unwrap();
    let phone_dir = tempfile::tempdir().unwrap();
    let phone_host = Arc::new(FakeHost::with_downloads());
    let phone = Node::start(config(&phone_dir, "phone"), phone_host.clone()).unwrap();
    let phone_cameras = || {
        vec![
            CameraInfo {
                id: "0".to_owned(),
                name: "Back camera".to_owned(),
                facing: "back".to_owned(),
                width: 1920,
                height: 1080,
                frame_rates: vec![60, 30],
            },
            CameraInfo {
                id: "1".to_owned(),
                name: "Front camera".to_owned(),
                facing: "front".to_owned(),
                width: 1280,
                height: 720,
                frame_rates: vec![30],
            },
        ]
    };
    // The app knows the cameras, but the user has not allowed the camera.
    phone.set_camera_share(false, false, phone_cameras());

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let desktop_dir = tempfile::tempdir().unwrap();
    let capture = FakeCapture::of_cameras(vec![Camera {
        id: "desk-cam".to_owned(),
        name: Some("Desk camera".to_owned()),
        facing: Facing::External,
        width: 1280,
        height: 720,
        frame_rates: vec![30],
    }]);
    capture.set_audio_encodes(vec![AudioFormat::OPUS_MONO]);
    let presenter = FakePresenter::new();
    let (desktop, desktop_sync, addr) = {
        let _guard = runtime.enter();
        let loc = Locations::resolve(Some(desktop_dir.path()), None);
        let (d, events) =
            ligatured::daemon::Daemon::open(loc, "127.0.0.1:0".parse().unwrap(), Some("desktop"))
                .unwrap();
        let sync = d.attach_camera(
            Some(capture.clone()),
            Some(ScreenParams {
                views: true,
                decodes: vec![Format::H264],
                audio_decodes: vec![AudioFormat::OPUS_MONO],
                max_width: 1280,
                max_height: 720,
                max_frame_rate: 30,
                ..ScreenParams::default()
            }),
            Some(presenter.clone()),
        );
        ligatured::run::spawn_connection_events(d.clone(), events);
        let addr = d.manager().endpoint().local_addr().unwrap();
        (d, sync, addr)
    };
    let alice = PersonKeys::generate().unwrap();
    let person = facts::person_fact(&alice, "Alice", 1, at(0)).unwrap();
    let roles = [
        facts::role_fact(
            &alice,
            Role::Binding,
            *phone.daemon().binding().public(),
            true,
            at(1),
        )
        .unwrap(),
        facts::role_fact(
            &alice,
            Role::Binding,
            *desktop.binding().public(),
            true,
            at(2),
        )
        .unwrap(),
    ];
    for d in [phone.daemon(), &desktop] {
        d.store().insert(&person).unwrap();
        for r in &roles {
            d.store().insert(r).unwrap();
        }
        d.update_keys(|k| k.set_joined(*alice.person(), Some(&alice.self_signing)))
            .unwrap();
    }
    let dial = phone.daemon().clone();
    runtime
        .block_on(async move { dial.manager().connect(addr).await })
        .unwrap();
    let phone_key = *phone.daemon().binding().public();
    let desktop_key = desktop.binding().public().to_string();
    let phone_params = || {
        desktop
            .registry()
            .peers()
            .into_iter()
            .filter(|p| p.peer == phone_key)
            .flat_map(|p| p.capabilities)
            .find_map(|d| camera_params(&d))
    };
    until("both see each other's camera family", || {
        phone.daemon().registry().peers_offering(CAMERA_TYPE).len() == 1 && phone_params().is_some()
    });
    // The phone shows a camera and takes a cast, presents none, and shares
    // none until the camera is allowed. The desktop offers no screen here.
    let params = phone_params().unwrap();
    assert!(params.views && params.receives && !params.presents && !params.shares);
    assert!(desktop.registry().peers_offering(SCREEN_TYPE).len() <= 1);
    phone.set_camera_share(true, true, phone_cameras());
    until(
        "the phone shares its cameras once the camera is allowed",
        || phone_params().is_some_and(|p| p.shares && p.audio_encodes == [AudioFormat::OPUS_MONO]),
    );

    // The phone watches the desktop's camera with its microphone: frames
    // and packets reach the app as a watched screen's do.
    let targets = phone.camera_targets();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].key, desktop_key);
    let listed = phone.camera_list(desktop_key.clone()).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(
        (
            listed[0].id.as_str(),
            listed[0].name.as_str(),
            listed[0].facing.as_str()
        ),
        ("desk-cam", "Desk camera", "external")
    );
    let watch = phone
        .camera_watch(desktop_key.clone(), None, 1920, 1080, 30, true)
        .unwrap();
    assert_eq!(
        (
            watch.display.as_str(),
            watch.width,
            watch.height,
            watch.audio_channels
        ),
        ("desk-cam", 1280, 720, 1)
    );
    // One watch screen: a second watch, of a screen or a camera, waits.
    assert!(
        phone
            .camera_watch(desktop_key.clone(), None, 1920, 1080, 30, false)
            .is_err()
    );
    until("the desktop's camera runs", || {
        capture.running() == vec!["desk-cam".to_owned()]
    });
    assert!(capture.emit_audio(
        "desk-cam",
        AudioPacket {
            at: 0,
            bytes: vec![5; 25],
        }
    ));
    assert!(capture.emit(
        "desk-cam",
        EncodedFrame {
            at: 0,
            key: true,
            bytes: vec![0, 0, 0, 1, 0x65, 9],
        }
    ));
    until("the app got the camera's picture and sound", || {
        *phone_host.screen_frames.lock().unwrap() == vec![(true, 6)]
            && phone_host.screen_audio.lock().unwrap().len() == 1
    });
    phone.screen_watch_stop();
    until("the desktop's camera stops", || {
        capture.running().is_empty()
    });

    // The desktop lists the phone's cameras and watches the front one with
    // the microphone: the app is asked for that camera.
    let cameras = runtime
        .block_on({
            let sync = desktop_sync.clone();
            async move { sync.cameras(&phone_key).await }
        })
        .unwrap();
    assert_eq!(
        cameras
            .iter()
            .map(|c| (c.id.as_str(), c.facing))
            .collect::<Vec<_>>(),
        [("0", Facing::Back), ("1", Facing::Front)]
    );
    let start = Start {
        display: Some("1".to_owned()),
        window: None,
        formats: Vec::new(),
        width: 1920,
        height: 1080,
        frame_rate: 60,
        audio: vec![AudioFormat::OPUS_MONO],
    };
    let watch = {
        let sync = desktop_sync.clone();
        let start = start.clone();
        runtime.spawn(async move { sync.watch(&phone_key, start).await })
    };
    until("the app was asked for the camera", || {
        phone_host.camera_requests.lock().unwrap().len() == 1
    });
    let request = phone_host.camera_requests.lock().unwrap()[0].clone();
    assert_eq!(request.0, desktop.binding().public().fingerprint());
    assert_eq!(
        (
            request.1.as_str(),
            request.2,
            request.3,
            request.4,
            request.5
        ),
        ("1", 1280, 720, 30, 1)
    );
    phone.camera_share_started(true);
    let mut watching = runtime.block_on(watch).unwrap().unwrap();
    assert_eq!(watching.started.source_id(), "1");
    assert_eq!(watching.started.audio, Some(AudioFormat::OPUS_MONO));
    phone.camera_share_audio(vec![2; 40]);
    phone.camera_share_frame(true, vec![7; 20_000]);
    let (mut frames, mut packets) = (0, 0);
    runtime.block_on(async {
        while frames == 0 || packets == 0 {
            match watching.frames.recv().await.unwrap() {
                Viewed::Frame { key, bytes, .. } => {
                    assert!(key);
                    assert_eq!(bytes.len(), 20_000);
                    frames += 1;
                }
                Viewed::Audio { bytes, .. } => {
                    assert_eq!(bytes.len(), 40);
                    packets += 1;
                }
                Viewed::Stopped(r) => panic!("stopped: {r}"),
            }
        }
    });
    watching.control.send(ShareControl::Keyframe).unwrap();
    until("the app was asked for a keyframe", || {
        *phone_host.camera_keyframes.lock().unwrap() == 1
    });
    // The user taps Stop sharing on the phone.
    phone.camera_share_ended("sharer".to_owned());
    let reason = runtime.block_on(async {
        loop {
            if let Viewed::Stopped(r) = watching.frames.recv().await.unwrap() {
                return r;
            }
        }
    });
    assert_eq!(reason, StopReason::Sharer);
    until("the app was told to stop the camera", || {
        *phone_host.camera_stops.lock().unwrap() == 1
    });
    // A camera the phone does not have is refused before the app is asked,
    // and one the app cannot start is refused for consent.
    let missing = Start {
        display: Some("7".to_owned()),
        ..start.clone()
    };
    let refused = runtime.block_on({
        let sync = desktop_sync.clone();
        async move { sync.watch(&phone_key, missing).await }
    });
    assert!(refused.is_err());
    assert_eq!(phone_host.camera_requests.lock().unwrap().len(), 1);
    let refused = {
        let sync = desktop_sync.clone();
        runtime.spawn(async move { sync.watch(&phone_key, start).await })
    };
    until("the app was asked again", || {
        phone_host.camera_requests.lock().unwrap().len() == 2
    });
    phone.camera_share_started(false);
    assert!(runtime.block_on(refused).unwrap().is_err());

    // The phone casts its back camera to the desktop as a webcam: the
    // desktop presents it at once, with nobody asked there.
    let cast_targets = phone.camera_cast_targets();
    assert_eq!(cast_targets.len(), 1);
    assert_eq!(cast_targets[0].key, desktop_key);
    assert!(cast_targets[0].presents && !cast_targets[0].shows);
    phone
        .camera_cast(desktop_key.clone(), None, true, true)
        .unwrap();
    until("the app was asked for the camera to cast", || {
        phone_host.camera_requests.lock().unwrap().len() == 3
    });
    let request = phone_host.camera_requests.lock().unwrap()[2].clone();
    assert_eq!(
        (request.1.as_str(), request.5),
        ("0", 1),
        "the first camera, with the microphone"
    );
    phone.camera_share_started(true);
    until("the desktop presents the phone's camera", || {
        presenter.started().len() == 1
    });
    until("the app was told the cast runs", || {
        phone_host
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::CameraCastStarted { peer } if *peer == desktop_key))
    });
    phone.camera_share_frame(true, vec![1; 300]);
    phone.camera_share_audio(vec![1; 30]);
    until("the presenter has the picture and the sound", || {
        presenter.frames() == 1 && presenter.audio() == 1
    });
    phone.camera_cast_stop();
    until("the app was told the cast ended", || {
        phone_host.events.lock().unwrap().iter().any(|e| {
            matches!(e, Event::CameraCastEnded { peer, reason } if *peer == desktop_key && reason == "sharer")
        })
    });
    until("the desktop presents nothing", || presenter.running() == 0);

    // The desktop casts its camera to the phone: the app is offered a
    // camera, takes it through the camera's call, and watches it.
    let cast = {
        let sync = desktop_sync.clone();
        runtime.spawn(async move { sync.cast(&phone_key, CastSource::default(), true).await })
    };
    until("the app was offered the camera", || {
        phone_host.offers.lock().unwrap().len() == 1
    });
    let offer = phone_host.offers.lock().unwrap()[0].clone();
    assert_eq!(
        (offer.kind.as_str(), offer.name.as_str(), offer.audio),
        ("camera", "Desk camera", true)
    );
    assert_eq!(offer.peer, desktop_key);
    // It is the camera's offer, not the screen's.
    assert!(
        phone
            .screen_offer_accept(desktop_key.clone(), true)
            .is_err()
    );
    let taken = phone
        .camera_offer_accept(desktop_key.clone(), true)
        .unwrap();
    assert_eq!(
        (taken.display.as_str(), taken.audio_channels),
        ("desk-cam", 1)
    );
    let casting = runtime.block_on(cast).unwrap().unwrap();
    phone.screen_watch_stop();
    assert_eq!(runtime.block_on(casting.ended).unwrap(), StopReason::Viewer);
    assert!(phone.camera_offer_decline(desktop_key).is_err());

    phone.stop();
    desktop.shutdown();
    runtime.shutdown_timeout(Duration::from_secs(2));
}
