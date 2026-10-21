//! Two nodes in one process with fake hosts: the first creates the person,
//! the second joins through the ceremony (offer, code, both confirm), then a
//! clipboard change the host reports on one is set through the other's host,
//! and a stop ends it.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ligature_android::{Config, Event, Host, LocalNotification, LogLevel, Node, NotificationCopy};

#[derive(Default)]
struct FakeHost {
    clipboard: Mutex<Option<String>>,
    events: Mutex<Vec<Event>>,
    shown: Mutex<Vec<NotificationCopy>>,
    dismissed_copies: Mutex<Vec<(String, String)>>,
    dismissed_own: Mutex<Vec<String>>,
}

impl FakeHost {
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
    }
}

#[test]
fn a_phone_joins_and_the_clipboard_flows_both_ways() {
    let laptop_dir = tempfile::tempdir().unwrap();
    let phone_dir = tempfile::tempdir().unwrap();
    let laptop_host = Arc::new(FakeHost::default());
    let phone_host = Arc::new(FakeHost::default());

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
