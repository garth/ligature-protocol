//! Two daemons of one person with a synthetic screen capture (milestone 8,
//! LP-148): the viewer lists the sharer's displays, watches one with the
//! picture fitted to what it asked, a large keyframe arrives in chunks and
//! whole, a keyframe request reaches the encoder, and a share ends cleanly
//! from the viewer, from the sharer, from a withdrawn consent and from a
//! dropped connection, with the next share working. A device that does not
//! share cannot be watched.

use std::sync::Arc;
use std::time::Duration;

use ligatured::daemon::Daemon;
use ligatured::screen::{EncodedFrame, FakeCapture, ScreenSync, ShareControl, Viewed};
use lp_core::facts::Role;
use lp_core::paths::Locations;
use lp_core::screen::{
    CHUNK_BYTES, Codec, Display, RefuseReason, SCREEN_TYPE, ScreenParams, Start, StopReason,
};
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

struct Pair {
    sharer: Arc<Daemon>,
    capture: Arc<FakeCapture>,
    sharer_sync: Arc<ScreenSync>,
    viewer: Arc<Daemon>,
    viewer_sync: Arc<ScreenSync>,
    _dirs: Vec<tempfile::TempDir>,
}

fn displays() -> Vec<Display> {
    vec![
        Display {
            id: "DP-1".to_owned(),
            name: Some("Main".to_owned()),
            width: 2560,
            height: 1440,
            primary: true,
        },
        Display {
            id: "HDMI-A-1".to_owned(),
            name: Some("Side".to_owned()),
            width: 1920,
            height: 1080,
            primary: false,
        },
    ]
}

/// A sharer with two displays and a viewer that takes up to 1280 by 720.
async fn pair() -> Pair {
    let alice = PersonKeys::generate().unwrap();
    let person = facts::person_fact(&alice, "Alice", 1, at(0)).unwrap();
    let mut daemons = Vec::new();
    for name in ["sharer", "viewer"] {
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
    let (viewer, viewer_events, viewer_dir) = daemons.pop().unwrap();
    let (sharer, sharer_events, sharer_dir) = daemons.pop().unwrap();
    let capture = FakeCapture::new(displays());
    let sharer_sync = sharer.attach_screen(Some(capture.clone()), None);
    let viewer_sync = viewer.attach_screen(
        None,
        Some(ScreenParams {
            views: true,
            max_width: 1280,
            max_height: 720,
            max_frame_rate: 30,
            ..ScreenParams::default()
        }),
    );
    drive_events(sharer.clone(), sharer_events);
    drive_events(viewer.clone(), viewer_events);
    viewer
        .manager()
        .connect(sharer.manager().endpoint().local_addr().unwrap())
        .await
        .unwrap();
    until("manifests exchanged", || {
        viewer.registry().peers_offering(SCREEN_TYPE).len() == 1
            && sharer.registry().peers_offering(SCREEN_TYPE).len() == 1
    })
    .await;
    Pair {
        sharer,
        capture,
        sharer_sync,
        viewer,
        viewer_sync,
        _dirs: vec![sharer_dir, viewer_dir],
    }
}

fn start(display: Option<&str>) -> Start {
    Start {
        display: display.map(str::to_owned),
        codec: Codec::H264,
        width: 1920,
        height: 1080,
        frame_rate: 60,
    }
}

async fn next_frame(watching: &mut ligatured::screen::Watching) -> (u64, bool, Vec<u8>) {
    match tokio::time::timeout(Duration::from_secs(5), watching.frames.recv())
        .await
        .expect("a frame in time")
        .expect("the share still runs")
    {
        Viewed::Frame {
            seq, key, bytes, ..
        } => (seq, key, bytes),
        Viewed::Stopped(reason) => panic!("stopped: {reason}"),
    }
}

async fn stopped_with(watching: &mut ligatured::screen::Watching) -> StopReason {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), watching.frames.recv())
            .await
            .expect("an end in time")
        {
            Some(Viewed::Stopped(reason)) => return reason,
            Some(Viewed::Frame { .. }) => continue,
            None => panic!("the channel closed without a stop"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn displays_are_listed_and_a_share_fits_chunks_and_answers_keyframes() {
    let p = pair().await;
    let sharer_key = *p.sharer.binding().public();
    let listed = p.viewer_sync.displays(&sharer_key).await.unwrap();
    assert_eq!(listed, displays());
    // The viewer offers no sharing, so the sharer cannot watch it.
    let err = p
        .sharer_sync
        .displays(p.viewer.binding().public())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("does not share"), "{err}");

    // The primary display, fitted: asked 1920x1080 at 60, the viewer takes
    // 1280x720 at 30, the display is 2560x1440.
    let mut watching = p.viewer_sync.watch(&sharer_key, start(None)).await.unwrap();
    assert_eq!(watching.started.display, "DP-1");
    assert_eq!(
        (
            watching.started.width,
            watching.started.height,
            watching.started.frame_rate
        ),
        (1280, 720, 30)
    );
    until("the capture runs", || {
        p.capture.running() == vec!["DP-1".to_owned()]
    })
    .await;
    let status = p.sharer_sync.status();
    assert_eq!(status.sharing.len(), 1);
    assert_eq!(status.sharing[0].peer, *p.viewer.binding().public());
    assert_eq!(p.viewer_sync.status().watching.len(), 1);

    // A keyframe larger than two chunks arrives whole and in order.
    let big: Vec<u8> = (0..CHUNK_BYTES * 2 + 1234)
        .map(|i| (i % 253) as u8)
        .collect();
    assert!(p.capture.emit(
        "DP-1",
        EncodedFrame {
            at: 1,
            key: true,
            bytes: big.clone(),
        }
    ));
    let (seq, key, bytes) = next_frame(&mut watching).await;
    assert_eq!((seq, key), (0, true));
    assert_eq!(bytes, big);
    p.capture.emit(
        "DP-1",
        EncodedFrame {
            at: 2,
            key: false,
            bytes: vec![9; 100],
        },
    );
    let (seq, key, bytes) = next_frame(&mut watching).await;
    assert_eq!((seq, key, bytes.len()), (1, false, 100));
    until("the sharer counted two frames", || {
        p.sharer_sync.status().sharing[0].frames == 2
    })
    .await;
    assert_eq!(p.viewer_sync.status().watching[0].frames, 2);

    // The viewer asks for a keyframe: the encoder hears it.
    watching.control.send(ShareControl::Keyframe).unwrap();
    until("the keyframe request reached the capture", || {
        p.capture.keyframe_requests() == 1
    })
    .await;

    // A second share of the same display is busy; the other display is free.
    let err = p
        .viewer_sync
        .watch(&sharer_key, start(Some("DP-1")))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("busy"), "{err}");
    let err = p
        .viewer_sync
        .watch(&sharer_key, start(Some("nope")))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("no-display"), "{err}");
    let mut side = p
        .viewer_sync
        .watch(&sharer_key, start(Some("HDMI-A-1")))
        .await
        .unwrap();
    assert_eq!((side.started.width, side.started.height), (1280, 720));
    until("both captures run", || p.capture.running().len() == 2).await;

    // The viewer stops the first: the capture is stopped and the status
    // forgets it; the second still runs.
    watching.control.send(ShareControl::Stop).unwrap();
    assert_eq!(stopped_with(&mut watching).await, StopReason::Viewer);
    until("the first capture stopped", || p.capture.stops() == 1).await;
    until("the sharer forgot the first share", || {
        p.sharer_sync.status().sharing.len() == 1
    })
    .await;
    assert_eq!(p.viewer_sync.status().watching.len(), 1);
    p.capture.emit(
        "HDMI-A-1",
        EncodedFrame {
            at: 3,
            key: true,
            bytes: vec![1, 2, 3],
        },
    );
    assert_eq!(next_frame(&mut side).await.2, vec![1, 2, 3]);

    // And the same display can be watched again.
    let mut again = p.viewer_sync.watch(&sharer_key, start(None)).await.unwrap();
    assert_eq!(again.started.display, "DP-1");
    again.control.send(ShareControl::Stop).unwrap();
    assert_eq!(stopped_with(&mut again).await, StopReason::Viewer);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_share_ends_from_the_sharer_from_consent_and_from_a_dropped_connection() {
    let p = pair().await;
    let sharer_key = *p.sharer.binding().public();

    // The sharer's user stops every share.
    let mut w = p.viewer_sync.watch(&sharer_key, start(None)).await.unwrap();
    until("the capture runs", || p.capture.running().len() == 1).await;
    assert_eq!(p.sharer_sync.stop_sharing(), 1);
    assert_eq!(stopped_with(&mut w).await, StopReason::Sharer);
    until("the capture stopped", || p.capture.stops() == 1).await;
    until("nothing is shared", || {
        p.sharer_sync.status().sharing.is_empty()
    })
    .await;
    assert!(p.viewer_sync.status().watching.is_empty());

    // The platform withdraws its consent mid-share.
    let mut w = p.viewer_sync.watch(&sharer_key, start(None)).await.unwrap();
    until("the capture runs again", || p.capture.running().len() == 1).await;
    p.capture.end("DP-1", StopReason::Consent);
    assert_eq!(stopped_with(&mut w).await, StopReason::Consent);
    until("nothing is shared again", || {
        p.sharer_sync.status().sharing.is_empty()
    })
    .await;

    // A refusal: the platform says no.
    p.capture.refuse_next(RefuseReason::Consent);
    let err = p
        .viewer_sync
        .watch(&sharer_key, start(None))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("consent"), "{err}");
    assert!(
        !err.to_string().starts_with("trust chain"),
        "a refusal is not a trust failure: {err}"
    );

    // The connection drops mid-share: both sides end it.
    let mut w = p.viewer_sync.watch(&sharer_key, start(None)).await.unwrap();
    until("the capture runs a third time", || {
        p.capture.running().len() == 1
    })
    .await;
    p.sharer.manager().drop_peer(p.viewer.binding().public());
    let reason = stopped_with(&mut w).await;
    assert!(
        matches!(reason, StopReason::Error | StopReason::Viewer),
        "{reason}"
    );
    // Three stops: the sharer's, the one after the withdrawn consent, and
    // this one; the refusal never started a share.
    until("the capture stopped with the connection", || {
        p.capture.stops() == 3
    })
    .await;
    until("nothing is shared after the drop", || {
        p.sharer_sync.status().sharing.is_empty()
    })
    .await;
    assert!(p.viewer_sync.status().watching.is_empty());
}
