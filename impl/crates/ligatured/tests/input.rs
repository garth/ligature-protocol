//! Two daemons of one person with in-memory input backends (milestone 7,
//! LP-142): the pointer crosses the configured edge and drives the other
//! device with motion, keys and scroll, comes back by the edge it entered,
//! an absolute entry types text, the release chord and a disconnect mid-drive
//! lift everything, and a device without the family is never driven.

use std::sync::Arc;
use std::time::Duration;

use ligatured::daemon::Daemon;
use ligatured::input::{Change, FakeCapture, FakeInject, InputSync};
use lp_core::facts::Role;
use lp_core::input::{BTN_LEFT, Edge, Enter, INPUT_TYPE, InputEvent, KEY_LEFTCTRL, KEY_RIGHTCTRL};
use lp_core::paths::Locations;
use lp_core::wire::Timestamp;
use lp_identity::cross_signing::PersonKeys;
use lp_identity::facts;
use lp_transport::manager::Event;

fn at(offset: i64) -> Timestamp {
    Timestamp::from_unix(1_760_000_000 + offset).unwrap()
}

async fn until(what: &str, mut check: impl FnMut() -> bool) {
    let waited = tokio::time::timeout(Duration::from_secs(5), async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "{what}");
}

fn drive_events(d: Arc<Daemon>, mut events: tokio::sync::mpsc::Receiver<Event>) {
    tokio::spawn(async move {
        while let Some(ev) = events.recv().await {
            match ev {
                Event::Connected(conn) => {
                    if d.recognise(conn.peer())
                        .map(|r| r.is_trusted())
                        .unwrap_or(false)
                    {
                        let d = d.clone();
                        tokio::spawn(async move { d.on_trusted(&conn).await });
                    }
                }
                Event::Disconnected(peer) => d.on_disconnected(&peer),
            }
        }
    });
}

struct Device {
    daemon: Arc<Daemon>,
    capture: Arc<FakeCapture>,
    inject: Arc<FakeInject>,
    sync: Arc<InputSync>,
    _dir: tempfile::TempDir,
}

/// Three daemons of one person, connected: `a` (a 1000 by 500 screen) and
/// `b` (2000 by 1000) with both input sides, `c` without input at all.
async fn three() -> (Device, Device, Arc<Daemon>, tempfile::TempDir) {
    let alice = PersonKeys::generate().unwrap();
    let person = facts::person_fact(&alice, "Alice", 1, at(0)).unwrap();
    let mut daemons = Vec::new();
    for name in ["a", "b", "c"] {
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
    let (c, c_events, c_dir) = daemons.pop().unwrap();
    let (b, b_events, b_dir) = daemons.pop().unwrap();
    let (a, a_events, a_dir) = daemons.pop().unwrap();
    let a_capture = FakeCapture::new(1000, 500);
    let a_inject = FakeInject::new(1000, 500);
    let b_capture = FakeCapture::new(2000, 1000);
    let b_inject = FakeInject::new(2000, 1000);
    let a_sync = a.attach_input(Some(a_capture.clone()), Some(a_inject.clone()));
    let b_sync = b.attach_input(Some(b_capture.clone()), Some(b_inject.clone()));
    drive_events(a.clone(), a_events);
    drive_events(b.clone(), b_events);
    drive_events(c.clone(), c_events);
    for (x, y) in [(&a, &b), (&a, &c), (&b, &c)] {
        x.manager()
            .connect(y.manager().endpoint().local_addr().unwrap())
            .await
            .unwrap();
    }
    until("a and b see each other's input", || {
        a.registry().peers_offering(INPUT_TYPE).len() == 1
            && b.registry().peers_offering(INPUT_TYPE).len() == 1
            && a.registry().peers().len() == 2
    })
    .await;
    (
        Device {
            daemon: a,
            capture: a_capture,
            inject: a_inject,
            sync: a_sync,
            _dir: a_dir,
        },
        Device {
            daemon: b,
            capture: b_capture,
            inject: b_inject,
            sync: b_sync,
            _dir: b_dir,
        },
        c,
        c_dir,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_pointer_crosses_an_edge_drives_the_other_device_and_comes_back() {
    let (a, b, c, _c_dir) = three().await;
    // c offers no input, and nobody offers it to c's view either way.
    assert!(
        !c.registry()
            .local()
            .iter()
            .any(|d| d.capability_type.as_str() == INPUT_TYPE)
    );
    until("c sees both offering input", || {
        c.registry().peers_offering(INPUT_TYPE).len() == 2
    })
    .await;

    // The layout: a's right edge leads to b. The capture is armed with it.
    a.sync
        .set_edge(Edge::Right, *b.daemon.binding().public())
        .unwrap();
    assert_eq!(a.capture.armed(), vec![Edge::Right]);
    assert_eq!(a.sync.layout().len(), 1);
    // And it survives a restart: the rows are in the store.
    assert_eq!(a.daemon.store().input_layout().unwrap().len(), 1);

    let mut b_changes = b.sync.subscribe();
    a.capture.hit_edge(Edge::Right, 0.4);
    until("b is driven", || b.sync.driven_by().is_some()).await;
    until("a's capture is taken", || {
        a.capture.taken() == Some(Edge::Right)
    })
    .await;
    assert_eq!(
        b_changes.recv().await.unwrap(),
        Change::DrivenBy(*a.daemon.binding().public())
    );
    // b placed its pointer on its left edge at 0.4.
    assert_eq!(b.inject.begun(), vec![(0.0, 0.4)]);
    let status = a.sync.status();
    let driving = status.driving.unwrap();
    assert_eq!(driving.peer, *b.daemon.binding().public());
    assert_eq!((driving.width, driving.height), (2000, 1000));
    assert_eq!(driving.edge, Some(Edge::Right));

    // Motion is scaled by the ratio of the widths (2000 / 1000); keys,
    // buttons and scroll pass through in order.
    a.capture.feed(InputEvent::Motion { dx: 10.0, dy: 5.0 });
    a.capture.feed(InputEvent::key(30, true));
    a.capture.feed(InputEvent::key(30, false));
    a.capture.feed(InputEvent::Button {
        code: BTN_LEFT,
        pressed: true,
    });
    a.capture.feed(InputEvent::Button {
        code: BTN_LEFT,
        pressed: false,
    });
    a.capture.feed(InputEvent::Scroll {
        dx: 0.0,
        dy: -1.0,
        discrete: true,
    });
    until("six events injected on b", || {
        b.inject.injected().len() == 6
    })
    .await;
    let injected = b.inject.injected();
    assert_eq!(injected[0], InputEvent::Motion { dx: 20.0, dy: 10.0 });
    assert_eq!(injected[1], InputEvent::key(30, true));
    assert_eq!(
        injected[3],
        InputEvent::Button {
            code: BTN_LEFT,
            pressed: true
        }
    );
    assert_eq!(
        injected[5],
        InputEvent::Scroll {
            dx: 0.0,
            dy: -1.0,
            discrete: true
        }
    );
    // The first batch carried a seq, so the round trip has a sample.
    until("a measured the round trip", || {
        a.sync.round_trip().is_some()
    })
    .await;
    let rt = a.sync.round_trip().unwrap();
    assert!(rt.median_ms < 1000.0, "{rt:?}");

    // The pointer moves back off b's left edge, the one it came in by: b
    // says leave, lifts everything, and a puts its pointer back on its right
    // edge at the height b reported.
    a.capture.feed(InputEvent::Motion {
        dx: -30.0,
        dy: 100.0,
    });
    until("a is no longer driving", || !a.sync.is_driving()).await;
    until("b is no longer driven", || b.sync.driven_by().is_none()).await;
    assert_eq!(b.inject.ends(), 1);
    let releases = a.capture.releases();
    assert_eq!(releases.len(), 1);
    let (edge, back_at) = releases[0].expect("put back by the edge");
    assert_eq!(edge, Edge::Right);
    // Started at 0.4 of b's 1000 px height, moved 5 then 100 controller px
    // down, scaled by two to 10 and 200: 0.4 + 0.01 + 0.2.
    assert!((back_at - 0.61).abs() < 0.001, "{back_at}");
    assert!(a.capture.taken().is_none());
    // The motion that crossed the edge was not injected.
    assert_eq!(b.inject.injected().len(), 6);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_absolute_entry_types_text_and_release_ends_it() {
    let (a, b, _c, _c_dir) = three().await;
    b.inject.clear();
    let ready = a
        .sync
        .drive(*b.daemon.binding().public(), Enter::absolute(0.5, 0.25))
        .await
        .unwrap();
    assert_eq!((ready.width, ready.height), (2000, 1000));
    until("b is driven", || b.sync.driven_by().is_some()).await;
    assert_eq!(b.inject.begun(), vec![(0.5, 0.25)]);
    // A touchpad sends events already in the driven device's terms.
    a.sync
        .send(
            vec![
                InputEvent::Position { x: 0.1, y: 0.9 },
                InputEvent::Motion { dx: 4.0, dy: 4.0 },
                InputEvent::Text {
                    text: "héllo".to_owned(),
                },
            ],
            None,
        )
        .await
        .unwrap();
    until("three events injected", || b.inject.injected().len() == 3).await;
    assert_eq!(
        b.inject.injected()[1],
        InputEvent::Motion { dx: 4.0, dy: 4.0 }
    );
    assert_eq!(
        b.inject.injected()[2],
        InputEvent::Text {
            text: "héllo".to_owned()
        }
    );
    // No edge to leave by: the user leaves the touchpad screen.
    a.sync.release_local(None).await;
    until("b is released", || b.sync.driven_by().is_none()).await;
    assert_eq!(b.inject.ends(), 1);
    assert!(!a.sync.is_driving());
    assert_eq!(a.capture.releases(), vec![None]);
    // And a second drive works.
    a.sync
        .drive(*b.daemon.binding().public(), Enter::absolute(0.0, 0.0))
        .await
        .unwrap();
    until("b is driven again", || b.sync.driven_by().is_some()).await;
    a.sync.release_local(None).await;
    until("b is released again", || b.sync.driven_by().is_none()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_release_chord_gives_the_input_back() {
    let (a, b, _c, _c_dir) = three().await;
    a.sync
        .set_edge(Edge::Left, *b.daemon.binding().public())
        .unwrap();
    a.capture.hit_edge(Edge::Left, 0.5);
    until("b is driven", || b.sync.driven_by().is_some()).await;
    a.capture.feed(InputEvent::key(KEY_LEFTCTRL, true));
    a.capture.feed(InputEvent::key(KEY_RIGHTCTRL, true));
    until("a let go", || !a.sync.is_driving()).await;
    until("b lifted everything", || b.inject.ends() == 1).await;
    assert_eq!(a.capture.releases(), vec![None]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_driven_device_never_follows_its_own_layout() {
    let (a, b, c, _c_dir) = three().await;
    a.sync
        .set_edge(Edge::Right, *b.daemon.binding().public())
        .unwrap();
    // b's own layout leads its right edge to c, which takes no input, and its
    // left edge back to a.
    b.sync
        .set_edge(Edge::Left, *a.daemon.binding().public())
        .unwrap();
    b.sync.set_edge(Edge::Right, *c.binding().public()).unwrap();
    a.capture.hit_edge(Edge::Right, 0.5);
    until("b is driven", || b.sync.driven_by().is_some()).await;
    // The pointer a drives reaches b's edges, which b's capture sees as it
    // would the user's own pointer (a Mac's event tap does): b starts nothing.
    b.capture.hit_edge(Edge::Left, 0.5);
    b.capture.hit_edge(Edge::Right, 0.5);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(!b.sync.is_driving());
    assert_eq!(b.capture.taken(), None);
    // On Linux the compositor captured the input when the pointer crossed
    // the barrier, before b decided to ignore the hit: b gives it back each
    // time, or the desktop is locked out while it is driven (the lockout of
    // 2026-10-24, the phone driving Linux into its right edge).
    until("b gave the input back for both hits", || {
        b.capture.releases() == vec![Some((Edge::Left, 0.5)), Some((Edge::Right, 0.5))]
    })
    .await;
    assert_eq!(b.sync.driven_by(), Some(*a.daemon.binding().public()));
    assert!(a.sync.is_driving());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_device_driven_by_a_touchpad_still_follows_its_layout() {
    // A touchpad (the phone) drives b with an absolute entry: b's pointer is
    // then the touchpad's, as if a mouse were plugged into b, and b's edge
    // leads on to the next desktop. Only a drive that entered by an edge is
    // kept from following b's layout, since that is what could loop.
    let (a, b, _c, _c_dir) = three().await;
    a.sync
        .drive(*b.daemon.binding().public(), Enter::absolute(0.5, 0.5))
        .await
        .unwrap();
    until("b is driven", || b.sync.driven_by().is_some()).await;
    b.sync
        .set_edge(Edge::Right, *a.daemon.binding().public())
        .unwrap();
    b.capture.hit_edge(Edge::Right, 0.5);
    until("b drives on from its edge", || b.sync.is_driving()).await;
    assert_eq!(b.capture.taken(), Some(Edge::Right));
    assert!(
        b.capture.releases().is_empty(),
        "the input was not given back"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_edge_hit_that_cannot_drive_gives_the_input_back() {
    // On Linux the compositor has already captured the keyboard and mouse
    // when it reports the edge hit: a drive that does not start must hand
    // them back, or the desktop is locked out.
    let (a, _b, c, _c_dir) = three().await;
    a.sync.set_edge(Edge::Right, *c.binding().public()).unwrap();
    a.capture.hit_edge(Edge::Right, 0.3);
    until("a gave the input back", || {
        a.capture.releases() == vec![Some((Edge::Right, 0.3))]
    })
    .await;
    assert!(!a.sync.is_driving());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn input_captured_with_no_drive_is_given_back_at_once() {
    let (a, _b, _c, _c_dir) = three().await;
    // Captured events with nothing driven: the platform holds the input for
    // no one. The first gives it back; the rest of the burst do not repeat it.
    for pressed in [true, false] {
        a.capture.feed(InputEvent::key(30, pressed));
    }
    until("a gave the input back", || {
        a.capture.releases() == vec![None]
    })
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(a.capture.releases(), vec![None]);
    assert!(!a.sync.is_driving());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_disconnect_mid_drive_lifts_everything_and_returns_the_pointer() {
    let (a, b, _c, _c_dir) = three().await;
    a.sync
        .set_edge(Edge::Bottom, *b.daemon.binding().public())
        .unwrap();
    a.capture.hit_edge(Edge::Bottom, 0.75);
    until("b is driven", || b.sync.driven_by().is_some()).await;
    assert_eq!(b.inject.begun(), vec![(0.75, 0.0)]);
    a.capture.feed(InputEvent::key(42, true));
    until("the key is held on b", || b.inject.injected().len() == 1).await;
    // The driven device's daemon stops: a gets its pointer back at once.
    b.daemon.manager().drop_peer(a.daemon.binding().public());
    until("a is no longer driving", || !a.sync.is_driving()).await;
    until("b is no longer driven", || b.sync.driven_by().is_none()).await;
    assert_eq!(b.inject.ends(), 1, "everything held was lifted");
    assert_eq!(a.capture.releases().len(), 1);
    assert!(a.capture.taken().is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_device_without_the_family_is_never_driven() {
    let (a, _b, c, _c_dir) = three().await;
    a.sync.set_edge(Edge::Top, *c.binding().public()).unwrap();
    a.capture.hit_edge(Edge::Top, 0.5);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!a.sync.is_driving());
    assert!(a.capture.taken().is_none());
    let err = a
        .sync
        .drive(*c.binding().public(), Enter::absolute(0.5, 0.5))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("offers no input"), "{err}");
}
