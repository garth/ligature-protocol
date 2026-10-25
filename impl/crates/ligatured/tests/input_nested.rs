//! Injection into a real compositor (milestone 7, LP-143): a daemon whose
//! injector drives a nested Hyprland through the virtual pointer and
//! keyboard protocols, driven by a second daemon with the in-memory capture.
//! Ignored by default, since it needs a nested compositor on an isolated
//! socket: see `docs/milestones/07-implementation-decisions.md` (LP-143) for
//! the launcher, then
//!
//! ```sh
//! LP_NESTED_DISPLAY=lp-test LP_NESTED_HYPR=<instance signature> \
//!   cargo test -p ligatured --test input_nested -- --ignored --nocapture
//! ```
//!
//! The compositor's configuration binds F9, Shift+F11, F11 and `eacute` to touch marker files
//! under `/tmp/lp-nested/`, which is how a key and a typed character are seen.

#![cfg(target_os = "linux")]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ligatured::daemon::Daemon;
use ligatured::input::{FakeCapture, FakeInject};
use ligatured::input_linux::LinuxInject;
use lp_core::facts::Role;
use lp_core::input::{BTN_LEFT, Edge, INPUT_TYPE, InputEvent};
use lp_core::paths::Locations;
use lp_core::wire::Timestamp;
use lp_identity::cross_signing::PersonKeys;
use lp_identity::facts;
use lp_transport::manager::Event;

fn at(offset: i64) -> Timestamp {
    Timestamp::from_unix(1_760_000_000 + offset).unwrap()
}

async fn until(what: &str, mut check: impl FnMut() -> bool) {
    let waited = tokio::time::timeout(Duration::from_secs(10), async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(50)).await;
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

/// The nested compositor's cursor position, through its own `hyprctl`.
fn cursorpos(instance: &str) -> (f64, f64) {
    let out = std::process::Command::new("hyprctl")
        .args(["-i", instance, "cursorpos"])
        .output()
        .expect("hyprctl");
    let text = String::from_utf8_lossy(&out.stdout);
    let mut parts = text
        .trim()
        .split(',')
        .map(|p| p.trim().parse::<f64>().unwrap_or(0.0));
    (parts.next().unwrap_or(0.0), parts.next().unwrap_or(0.0))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs a nested Hyprland on LP_NESTED_DISPLAY"]
async fn a_drive_moves_the_nested_compositor_s_pointer_and_types_into_it() {
    let display = std::env::var("LP_NESTED_DISPLAY").expect("LP_NESTED_DISPLAY");
    let instance = std::env::var("LP_NESTED_HYPR").expect("LP_NESTED_HYPR");
    let runtime_dir = std::path::PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap());
    for marker in ["f9", "eacute", "f10", "q", "shift_f11", "f11"] {
        let _ = std::fs::remove_file(Path::new("/tmp/lp-nested").join(marker));
    }

    let alice = PersonKeys::generate().unwrap();
    let person = facts::person_fact(&alice, "Alice", 1, at(0)).unwrap();
    let mut daemons = Vec::new();
    for name in ["driven", "controller"] {
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
    let (controller, c_events, _cd) = daemons.pop().unwrap();
    let (driven, d_events, _dd) = daemons.pop().unwrap();

    // The driven daemon injects into the nested compositor for real.
    let inject = LinuxInject::start_with(
        runtime_dir,
        Some(display.into()),
        Some(std::path::PathBuf::from(env!("CARGO_BIN_EXE_ligatured"))),
    );
    let driven_sync = driven.attach_input(None, Some(inject.clone()));
    let capture = FakeCapture::new(1000, 500);
    let controller_sync =
        controller.attach_input(Some(capture.clone()), Some(FakeInject::new(1000, 500)));
    drive_events(driven.clone(), d_events);
    drive_events(controller.clone(), c_events);
    controller
        .manager()
        .connect(driven.manager().endpoint().local_addr().unwrap())
        .await
        .unwrap();
    until("the helper is up and advertised", || {
        controller.registry().peers().iter().any(|p| {
            p.peer == *driven.binding().public()
                && p.capabilities.iter().any(|d| {
                    lp_core::input::InputParams::from_descriptor(d).is_some_and(|p| p.accepts)
                })
        })
    })
    .await;
    assert_eq!(controller.registry().peers_offering(INPUT_TYPE).len(), 1);
    let (w, h) = ligatured::input::Inject::screen(&*inject);
    eprintln!("nested screen: {w}x{h}");
    assert!(w > 100 && h > 100, "the nested compositor's logical size");
    let (w, h) = (f64::from(w), f64::from(h));

    // Cross the right edge half way down: the pointer lands on the left
    // edge of the nested screen at half its height.
    controller_sync
        .set_edge(Edge::Right, *driven.binding().public())
        .unwrap();
    capture.hit_edge(Edge::Right, 0.5);
    until("driven", || driven_sync.driven_by().is_some()).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let (x, y) = cursorpos(&instance);
    eprintln!("after enter: {x},{y}");
    assert!(
        x < 5.0 && (y - h / 2.0).abs() < 5.0,
        "entered at the left edge, half way: {x},{y}"
    );

    // Move right by 100 controller pixels, scaled by the width ratio.
    capture.feed(InputEvent::Motion { dx: 100.0, dy: 0.0 });
    let expected = 100.0 * w / 1000.0;
    until("the pointer moved by the scaled delta", || {
        (cursorpos(&instance).0 - expected).abs() < 10.0
    })
    .await;
    let (x2, y2) = cursorpos(&instance);
    eprintln!("after motion: {x2},{y2} (expected x {expected})");
    assert!((y2 - y).abs() < 5.0);

    // A key the nested compositor binds, a click, and a character typed.
    capture.feed(InputEvent::Key {
        code: 67,
        pressed: true,
    });
    capture.feed(InputEvent::Key {
        code: 67,
        pressed: false,
    });
    capture.feed(InputEvent::Button {
        code: BTN_LEFT,
        pressed: true,
    });
    capture.feed(InputEvent::Button {
        code: BTN_LEFT,
        pressed: false,
    });
    until("F9 reached the compositor", || {
        Path::new("/tmp/lp-nested/f9").exists()
    })
    .await;
    // Shift held while F11 is tapped: the compositor sees the Shift
    // modifier only if the virtual keyboard reports it (LP-143), so the
    // Shift+F11 binding fires and the plain F11 one does not.
    for (code, pressed) in [(42, true), (87, true), (87, false), (42, false)] {
        capture.feed(InputEvent::Key { code, pressed });
    }
    until("Shift+F11 reached the compositor", || {
        Path::new("/tmp/lp-nested/shift_f11").exists()
    })
    .await;
    assert!(
        !Path::new("/tmp/lp-nested/f11").exists(),
        "F11 arrived without Shift"
    );
    // A character the seat's layout has, through the temporary keymap.
    controller_sync
        .send(
            vec![InputEvent::Text {
                text: "q".to_owned(),
            }],
            None,
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let devices = std::process::Command::new("hyprctl")
        .args(["-i", &instance, "devices"])
        .output()
        .expect("hyprctl");
    eprintln!("devices:\n{}", String::from_utf8_lossy(&devices.stdout));
    until("the typed q reached the compositor", || {
        Path::new("/tmp/lp-nested/q").exists()
    })
    .await;
    controller_sync
        .send(
            vec![InputEvent::Text {
                text: "é".to_owned(),
            }],
            None,
        )
        .await
        .unwrap();
    until("the typed character reached the compositor", || {
        Path::new("/tmp/lp-nested/eacute").exists()
    })
    .await;
    // A key the compositor's own keymap has, after the text swapped keymaps.
    capture.feed(InputEvent::Key {
        code: 68,
        pressed: true,
    });
    capture.feed(InputEvent::Key {
        code: 68,
        pressed: false,
    });
    until(
        "F10 reached the compositor after the keymap came back",
        || Path::new("/tmp/lp-nested/f10").exists(),
    )
    .await;

    // Back off the left edge: the drive ends and the controller's capture is
    // released at the height the pointer left.
    capture.feed(InputEvent::Motion {
        dx: -200.0,
        dy: 0.0,
    });
    until("the drive ended", || !controller_sync.is_driving()).await;
    assert_eq!(capture.releases().len(), 1);
    let (edge, back) = capture.releases()[0].expect("by the edge");
    assert_eq!(edge, Edge::Right);
    assert!((back - 0.5).abs() < 0.02, "{back}");
}
