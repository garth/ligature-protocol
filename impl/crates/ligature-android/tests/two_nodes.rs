//! Two nodes in one process with fake hosts: the first creates the person,
//! the second joins through the ceremony (offer, code, both confirm), then a
//! clipboard change the host reports on one is set through the other's host,
//! and a stop ends it.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ligature_android::{
    Config, DownloadTarget, Event, Host, HostClipValue, LocalNotification, LogLevel, Node,
    NotificationCopy, OutgoingFile, PublishedDownload, TransferInfo,
};

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
