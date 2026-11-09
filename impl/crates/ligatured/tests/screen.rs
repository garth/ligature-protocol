//! Two daemons of one person with a synthetic screen capture (milestone 8,
//! LP-148): the viewer lists the sharer's displays, watches one with the
//! picture fitted to what it asked, a large keyframe arrives in chunks and
//! whole, a keyframe request reaches the encoder, and a share ends cleanly
//! from the viewer, from the sharer, from a withdrawn consent and from a
//! dropped connection, with the next share working. A device that does not
//! share cannot be watched. Milestone 9 (LP-199): a cast is offered by the
//! sharer and taken or declined by whoever shows pictures on the receiver, a
//! window is shared by id, and sound rides on the share's stream between the
//! frame chunks on the frames' clock.

use std::sync::Arc;
use std::time::Duration;

use ligatured::daemon::Daemon;
use ligatured::screen::{
    AudioPacket, CastSource, EncodedFrame, FakeCapture, ScreenSync, ShareControl, Viewed,
};
use lp_core::facts::Role;
use lp_core::paths::Locations;
use lp_core::screen::{
    AudioFormat, CHUNK_BYTES, Display, Format, Offer, RefuseReason, SCREEN_TYPE, ScreenParams,
    SourceKind, Start, StopReason, Window,
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
            scale: Some(2.0),
        },
        Display {
            id: "HDMI-A-1".to_owned(),
            name: Some("Side".to_owned()),
            width: 1920,
            height: 1080,
            primary: false,
            scale: None,
        },
    ]
}

fn windows() -> Vec<Window> {
    vec![Window {
        id: "w-1".to_owned(),
        name: Some("Editor".to_owned()),
        app: Some("org.example.editor".to_owned()),
        width: 1600,
        height: 1000,
        scale: Some(2.0),
    }]
}

/// A sharer with two displays and a window, and a viewer that takes up to
/// 1280 by 720 and plays stereo Opus.
async fn pair() -> Pair {
    pair_decoding(vec![Format::H264]).await
}

/// The same, the viewer decoding `decodes`.
async fn pair_decoding(decodes: Vec<Format>) -> Pair {
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
    let capture = FakeCapture::with_windows(displays(), windows());
    let sharer_sync = sharer.attach_screen(Some(capture.clone()), None);
    let viewer_sync = viewer.attach_screen(
        None,
        Some(ScreenParams {
            views: true,
            decodes,
            audio_decodes: vec![AudioFormat::OPUS_STEREO],
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
        window: None,
        formats: Vec::new(),
        width: 1920,
        height: 1080,
        frame_rate: 60,
        audio: Vec::new(),
    }
}

/// The next frame, passing over sound.
async fn next_frame(watching: &mut ligatured::screen::Watching) -> (u64, bool, Vec<u8>) {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), watching.frames.recv())
            .await
            .expect("a frame in time")
            .expect("the share still runs")
        {
            Viewed::Frame {
                seq, key, bytes, ..
            } => return (seq, key, bytes),
            Viewed::Audio { .. } => continue,
            Viewed::Stopped(reason) => panic!("stopped: {reason}"),
        }
    }
}

/// The next packet of sound, passing over frames.
async fn next_audio(watching: &mut ligatured::screen::Watching) -> (u64, u64, Vec<u8>) {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), watching.frames.recv())
            .await
            .expect("a packet in time")
            .expect("the share still runs")
        {
            Viewed::Audio { seq, at, bytes } => return (seq, at, bytes),
            Viewed::Frame { .. } => continue,
            Viewed::Stopped(reason) => panic!("stopped: {reason}"),
        }
    }
}

async fn stopped_with(watching: &mut ligatured::screen::Watching) -> StopReason {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), watching.frames.recv())
            .await
            .expect("an end in time")
        {
            Some(Viewed::Stopped(reason)) => return reason,
            Some(Viewed::Frame { .. } | Viewed::Audio { .. }) => continue,
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
    assert_eq!(watching.started.display.as_deref(), Some("DP-1"));
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
    assert_eq!(again.started.display.as_deref(), Some("DP-1"));
    again.control.send(ShareControl::Stop).unwrap();
    assert_eq!(stopped_with(&mut again).await, StopReason::Viewer);
}

/// Whether the viewer has the sharer's descriptor listing `format` as encoded.
fn offered(p: &Pair, format: Format) -> bool {
    p.viewer
        .registry()
        .peers()
        .into_iter()
        .filter(|caps| caps.peer == *p.sharer.binding().public())
        .flat_map(|caps| caps.capabilities)
        .filter_map(|d| ScreenParams::from_descriptor(&d))
        .any(|params| params.encodes.contains(&format))
}

/// The viewer offers what it decodes in its order, and the sharer picks the
/// first it encodes now (ADR 0044).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_sharer_picks_the_first_format_of_the_viewer_s_offer_it_encodes() {
    let p = pair_decoding(vec![Format::H264_444, Format::H264]).await;
    let sharer_key = *p.sharer.binding().public();

    // The sharer encodes 4:2:0 alone: the viewer's first choice is passed over.
    let mut watching = p
        .viewer_sync
        .watch(&sharer_key, start(Some("DP-1")))
        .await
        .unwrap();
    assert_eq!(watching.started.format, Format::H264);
    watching.control.send(ShareControl::Stop).unwrap();
    assert_eq!(stopped_with(&mut watching).await, StopReason::Viewer);

    // It encodes both, 4:2:0 first: the viewer's order decides.
    p.capture.set_encodes(vec![Format::H264, Format::H264_444]);
    p.sharer_sync.advertise().await;
    until("the viewer sees 4:4:4 offered", || {
        offered(&p, Format::H264_444)
    })
    .await;
    let mut watching = p
        .viewer_sync
        .watch(&sharer_key, start(Some("HDMI-A-1")))
        .await
        .unwrap();
    assert_eq!(watching.started.format, Format::H264_444);
    watching.control.send(ShareControl::Stop).unwrap();
    assert_eq!(stopped_with(&mut watching).await, StopReason::Viewer);
    until("both shares stopped", || p.capture.stops() == 2).await;

    // Narrowed to 4:4:4, but the sharer's encoder changed since its
    // descriptor: the sharer refuses rather than send what was not offered.
    p.capture.set_encodes(vec![Format::H264]);
    let narrowed = Start {
        formats: vec![Format::H264_444],
        ..start(Some("DP-1"))
    };
    let err = p
        .viewer_sync
        .watch(&sharer_key, narrowed.clone())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("no-encoder"), "{err}");

    // Once the descriptor says so, the viewer does not ask at all.
    p.sharer_sync.advertise().await;
    until("the viewer sees 4:4:4 withdrawn", || {
        !offered(&p, Format::H264_444)
    })
    .await;
    let err = p
        .viewer_sync
        .watch(&sharer_key, narrowed)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("no picture format"), "{err}");
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

/// A viewer's control client that goes away while the capture sends nothing
/// (a stalled capture, a still screen without keepalive) ends the watch and
/// the share at once, so the display can be watched again rather than being
/// refused as busy.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_that_goes_while_no_frames_come_ends_the_share() {
    use ligatured::control::{Client, Request, serve, types};
    let p = pair().await;
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("control.sock");
    let _server = serve(p.viewer.clone(), &socket).unwrap();
    let mut client = Client::connect(&socket).await.unwrap();
    let mut req = Request::new(types::SCREEN_START, 0);
    req.key = Some(*p.sharer.binding().public());
    req.display = Some("DP-1".to_owned());
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let watch = tokio::spawn(async move {
        let mut started_tx = Some(started_tx);
        let _ = client
            .call(req, |event| {
                if event.started.is_some()
                    && let Some(tx) = started_tx.take()
                {
                    let _ = tx.send(());
                }
                None
            })
            .await;
    });
    tokio::time::timeout(Duration::from_secs(5), started_rx)
        .await
        .expect("started in time")
        .unwrap();
    assert_eq!(p.capture.running(), vec!["DP-1".to_owned()]);
    // The client goes; the capture has sent no frame.
    watch.abort();
    let _ = watch.await;
    until("the viewer stopped watching", || {
        p.viewer_sync.status().watching.is_empty()
    })
    .await;
    until("the sharer stopped sharing", || {
        p.capture.stops() == 1 && p.sharer_sync.status().sharing.is_empty()
    })
    .await;
}

/// What the sharer's registry says about the viewer's screen params.
fn viewer_params_seen(p: &Pair) -> Option<ScreenParams> {
    p.sharer
        .registry()
        .peers()
        .into_iter()
        .filter(|caps| caps.peer == *p.viewer.binding().public())
        .flat_map(|caps| caps.capabilities)
        .find_map(|d| ScreenParams::from_descriptor(&d))
}

/// Milestone 9: the sharer casts a window with sound; the receiver's hook is
/// told, takes it, and the share runs as a watch does, sound between the
/// frames on the frames' clock, until the receiver stops it. A cast to a
/// device that does not receive, a declined cast and a cast with no common
/// format are refused with their reasons; a cast stopped by the caster ends
/// at the receiver as `sharer`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cast_is_offered_taken_and_runs_with_sound() {
    let p = pair().await;
    let (sharer_key, viewer_key) = (*p.sharer.binding().public(), *p.viewer.binding().public());
    p.capture.set_audio_encodes(vec![AudioFormat::OPUS_STEREO]);
    p.sharer_sync.advertise().await;

    // Nothing on the viewer shows casts yet: it does not receive.
    let err = p
        .sharer_sync
        .cast(&viewer_key, CastSource::default(), true)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("does not take a cast"), "{err}");
    // And a viewer cannot cast: it shares nothing.
    let err = p
        .viewer_sync
        .cast(&sharer_key, CastSource::default(), true)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("does not share"), "{err}");

    // Something that shows casts is set, and the sharer learns of it.
    let (offers_tx, mut offers) = tokio::sync::mpsc::unbounded_channel::<(_, Offer)>();
    p.viewer_sync.set_on_offer(Arc::new(move |peer, offer| {
        let _ = offers_tx.send((peer, offer));
    }));
    p.viewer_sync.advertise().await;
    until("the sharer sees the viewer receive", || {
        viewer_params_seen(&p).is_some_and(|v| v.receives)
    })
    .await;
    assert!(p.viewer_sync.status().receives);

    // The sharer casts its window with sound; the hook hears the offer.
    let caster = p.sharer_sync.clone();
    let casting = tokio::spawn(async move {
        caster
            .cast(
                &viewer_key,
                CastSource {
                    display: None,
                    window: Some("w-1".to_owned()),
                },
                true,
            )
            .await
    });
    let (from, offer) = tokio::time::timeout(Duration::from_secs(5), offers.recv())
        .await
        .expect("an offer in time")
        .expect("the hook is live");
    assert_eq!(from, sharer_key);
    assert_eq!(offer.source.kind, SourceKind::Window);
    assert_eq!(offer.source.id, "w-1");
    assert_eq!(offer.source.app.as_deref(), Some("org.example.editor"));
    assert_eq!(offer.formats, [Format::H264]);
    assert_eq!(offer.audio, [AudioFormat::OPUS_STEREO]);
    let pending = p.viewer_sync.offers();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].peer, sharer_key);

    // The receiver takes it: the picture fitted to what it takes, the
    // window named, the sound agreed, and both sides call it a cast.
    let mut watching = p.viewer_sync.accept(&sharer_key, true).await.unwrap();
    assert_eq!(watching.started.window.as_deref(), Some("w-1"));
    assert_eq!(watching.started.display, None);
    assert_eq!(watching.started.kind(), SourceKind::Window);
    assert_eq!(
        (watching.started.width, watching.started.height),
        (1152, 720),
        "1600x1000 fitted into 1280x720"
    );
    assert_eq!(watching.started.frame_rate, 30);
    assert_eq!(watching.started.audio, Some(AudioFormat::OPUS_STEREO));
    let casting = casting.await.unwrap().unwrap();
    assert_eq!(casting.started, watching.started);
    let settings = p.capture.last_settings().expect("a share started");
    assert_eq!(settings.kind, SourceKind::Window);
    assert_eq!(settings.audio, Some(AudioFormat::OPUS_STEREO));
    until("the capture runs", || {
        p.capture.running() == vec!["w-1".to_owned()]
    })
    .await;
    assert!(p.viewer_sync.offers().is_empty());
    let status = p.sharer_sync.status();
    assert!(status.sharing[0].cast);
    assert_eq!(status.sharing[0].started.window.as_deref(), Some("w-1"));
    let status = p.viewer_sync.status();
    assert!(status.watching[0].cast);

    // Sound and picture arrive, each on the share's clock; of what piled
    // up together the sound goes first, since it never waits for a frame.
    assert!(p.capture.emit_audio(
        "w-1",
        AudioPacket {
            at: 1_000,
            bytes: vec![1, 2, 3],
        }
    ));
    assert!(p.capture.emit(
        "w-1",
        EncodedFrame {
            at: 2_000,
            key: true,
            bytes: vec![0, 0, 0, 1, 0x65],
        }
    ));
    assert!(p.capture.emit_audio(
        "w-1",
        AudioPacket {
            at: 21_000,
            bytes: vec![4, 5, 6],
        }
    ));
    let (seq, at, bytes) = next_audio(&mut watching).await;
    assert_eq!((seq, at, bytes), (0, 1_000, vec![1, 2, 3]));
    let (seq, at, bytes) = next_audio(&mut watching).await;
    assert_eq!((seq, at, bytes), (1, 21_000, vec![4, 5, 6]));
    let (seq, key, bytes) = next_frame(&mut watching).await;
    assert_eq!((seq, key, bytes), (0, true, vec![0, 0, 0, 1, 0x65]));
    until("both sides counted the sound", || {
        p.sharer_sync
            .status()
            .sharing
            .first()
            .is_some_and(|s| s.audio == 2)
            && p.viewer_sync
                .status()
                .watching
                .first()
                .is_some_and(|w| w.audio == 2)
    })
    .await;

    // The receiver stops: the caster hears `viewer`.
    watching.control.send(ShareControl::Stop).unwrap();
    assert_eq!(stopped_with(&mut watching).await, StopReason::Viewer);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), casting.ended)
            .await
            .expect("the cast ends in time")
            .unwrap(),
        StopReason::Viewer
    );
    until("the capture stopped", || p.capture.stops() == 1).await;

    // A cast declined.
    let caster = p.sharer_sync.clone();
    let casting =
        tokio::spawn(async move { caster.cast(&viewer_key, CastSource::default(), false).await });
    let (_, offer) = tokio::time::timeout(Duration::from_secs(5), offers.recv())
        .await
        .expect("an offer in time")
        .expect("the hook is live");
    assert_eq!(offer.source.kind, SourceKind::Display);
    assert_eq!(offer.source.id, "DP-1", "the primary display by default");
    assert!(offer.audio.is_empty(), "no sound was offered");
    p.viewer_sync.decline(&sharer_key).unwrap();
    let err = casting.await.unwrap().unwrap_err();
    assert!(err.to_string().contains("declined"), "{err}");
    assert!(p.viewer_sync.offers().is_empty());
    assert!(
        p.viewer_sync.decline(&sharer_key).is_err(),
        "nothing left to decline"
    );

    // An empty window id lets the platform's picker choose: the fake picks
    // its first window.
    let caster = p.sharer_sync.clone();
    let casting = tokio::spawn(async move {
        caster
            .cast(
                &viewer_key,
                CastSource {
                    display: None,
                    window: Some(String::new()),
                },
                false,
            )
            .await
    });
    let (_, offer) = tokio::time::timeout(Duration::from_secs(5), offers.recv())
        .await
        .expect("an offer in time")
        .expect("the hook is live");
    assert_eq!(
        (offer.source.kind, offer.source.id.as_str()),
        (SourceKind::Window, "w-1")
    );
    p.viewer_sync.decline(&sharer_key).unwrap();
    assert!(casting.await.unwrap().is_err());

    // A cast the receiver could not decode is not even offered.
    p.capture.set_encodes(vec![Format::H264_444]);
    let err = p
        .sharer_sync
        .cast(&viewer_key, CastSource::default(), true)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("no picture format"), "{err}");
    p.capture.set_encodes(vec![Format::H264]);
    let err = p
        .sharer_sync
        .cast(
            &viewer_key,
            CastSource {
                display: None,
                window: Some("nope".to_owned()),
            },
            true,
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("no window"), "{err}");

    // A cast the caster stops ends at the receiver as `sharer`, with the
    // receiver's picture silent since it asked for no sound.
    let caster = p.sharer_sync.clone();
    let casting =
        tokio::spawn(async move { caster.cast(&viewer_key, CastSource::default(), true).await });
    let _ = tokio::time::timeout(Duration::from_secs(5), offers.recv())
        .await
        .expect("an offer in time");
    let mut watching = p.viewer_sync.accept(&sharer_key, false).await.unwrap();
    assert_eq!(watching.started.display.as_deref(), Some("DP-1"));
    assert_eq!(watching.started.audio, None, "no sound was asked for");
    let casting = casting.await.unwrap().unwrap();
    until("the capture runs again", || p.capture.running().len() == 1).await;
    assert_eq!(p.sharer_sync.stop_sharing(), 1);
    assert_eq!(stopped_with(&mut watching).await, StopReason::Sharer);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), casting.ended)
            .await
            .expect("the cast ends in time")
            .unwrap(),
        StopReason::Sharer
    );
    until("nothing is shared", || {
        p.sharer_sync.status().sharing.is_empty()
    })
    .await;
}

/// Milestone 9: a viewer watches a window by id and is told of the windows
/// beside the displays; a window the sharer lacks is `no-display`; a watch
/// that asks for sound gets it only when both sides have a format, and one
/// that asks for none is silent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_window_is_watched_by_id_and_sound_comes_only_when_asked_and_agreed() {
    let p = pair().await;
    let sharer_key = *p.sharer.binding().public();
    let listed = p.viewer_sync.sources(&sharer_key).await.unwrap();
    assert_eq!(listed.displays.as_deref(), Some(displays().as_slice()));
    assert_eq!(listed.windows.as_deref(), Some(windows().as_slice()));

    // The sharer has no sound to give: the share is silent though asked.
    let with_sound = Start {
        audio: vec![AudioFormat::OPUS_STEREO],
        display: None,
        window: Some("w-1".to_owned()),
        ..start(None)
    };
    let mut watching = p
        .viewer_sync
        .watch(&sharer_key, with_sound.clone())
        .await
        .unwrap();
    assert_eq!(watching.started.window.as_deref(), Some("w-1"));
    assert_eq!(watching.started.audio, None);
    assert_eq!(
        p.capture.last_settings().map(|s| (s.kind, s.audio)),
        Some((SourceKind::Window, None))
    );
    watching.control.send(ShareControl::Stop).unwrap();
    assert_eq!(stopped_with(&mut watching).await, StopReason::Viewer);
    until("the capture stopped", || p.capture.stops() == 1).await;

    // Now it has: the sound is agreed, and the sharer's packets reach the
    // viewer on the frames' clock.
    p.capture.set_audio_encodes(vec![AudioFormat::OPUS_STEREO]);
    p.sharer_sync.advertise().await;
    until("the viewer sees the sound offered", || {
        offered_audio(&p, AudioFormat::OPUS_STEREO)
    })
    .await;
    let mut watching = p.viewer_sync.watch(&sharer_key, with_sound).await.unwrap();
    assert_eq!(watching.started.audio, Some(AudioFormat::OPUS_STEREO));
    assert!(p.capture.emit_audio(
        "w-1",
        AudioPacket {
            at: 0,
            bytes: vec![9],
        }
    ));
    let (seq, at, bytes) = next_audio(&mut watching).await;
    assert_eq!((seq, bytes), (0, vec![9]));
    assert!(
        at < 5_000_000,
        "a packet stamped nothing gets the sync's clock"
    );
    watching.control.send(ShareControl::Stop).unwrap();
    assert_eq!(stopped_with(&mut watching).await, StopReason::Viewer);
    until("the capture stopped again", || p.capture.stops() == 2).await;

    // Asked for none: silent, whatever both could do.
    let mut watching = p
        .viewer_sync
        .watch(&sharer_key, start(Some("HDMI-A-1")))
        .await
        .unwrap();
    assert_eq!(watching.started.audio, None);
    assert_eq!(watching.started.display.as_deref(), Some("HDMI-A-1"));
    watching.control.send(ShareControl::Stop).unwrap();
    assert_eq!(stopped_with(&mut watching).await, StopReason::Viewer);

    // A window the sharer lacks.
    let err = p
        .viewer_sync
        .watch(
            &sharer_key,
            Start {
                window: Some("w-9".to_owned()),
                ..start(None)
            },
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("no-display"), "{err}");
}

/// Whether the viewer has the sharer's descriptor listing `format` as an
/// audio format it encodes.
fn offered_audio(p: &Pair, format: AudioFormat) -> bool {
    p.viewer
        .registry()
        .peers()
        .into_iter()
        .filter(|caps| caps.peer == *p.sharer.binding().public())
        .flat_map(|caps| caps.capabilities)
        .filter_map(|d| ScreenParams::from_descriptor(&d))
        .any(|params| params.audio_encodes.contains(&format))
}

/// Milestone 9 over the control socket, as `lpctl screen cast` and `lpview
/// --receive` use it: the sharer's `screen.cast` waits for the receiver,
/// whose hook is told and whose `screen.accept` takes the cast; `started`,
/// `audio` and `frame` events reach the receiver's client, `screen.status`
/// shows the cast on both sides, and `screen.stop` on the sharer ends it,
/// answering both calls.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cast_runs_over_the_control_socket() {
    use ligatured::control::{Client, Request, ScreenStopped, serve, types};
    use ligatured::screen::ScreenStatus;
    let p = pair().await;
    let (sharer_key, viewer_key) = (*p.sharer.binding().public(), *p.viewer.binding().public());
    p.capture.set_audio_encodes(vec![AudioFormat::OPUS_STEREO]);
    p.sharer_sync.advertise().await;
    let (offers_tx, mut offers) = tokio::sync::mpsc::unbounded_channel::<(_, Offer)>();
    p.viewer_sync.set_on_offer(Arc::new(move |peer, offer| {
        let _ = offers_tx.send((peer, offer));
    }));
    p.viewer_sync.advertise().await;
    until("the sharer sees the viewer receive", || {
        viewer_params_seen(&p).is_some_and(|v| v.receives)
    })
    .await;

    let dir = tempfile::tempdir().unwrap();
    let (sharer_socket, viewer_socket) = (dir.path().join("s.sock"), dir.path().join("v.sock"));
    let _sharer_server = serve(p.sharer.clone(), &sharer_socket).unwrap();
    let _viewer_server = serve(p.viewer.clone(), &viewer_socket).unwrap();

    // The caster's client: `screen.cast` of the window, with sound.
    let mut caster = Client::connect(&sharer_socket).await.unwrap();
    let mut req = Request::new(types::SCREEN_CAST, 0);
    req.key = Some(viewer_key);
    req.window = Some("w-1".to_owned());
    let (cast_started_tx, cast_started) = tokio::sync::oneshot::channel();
    let casting = tokio::spawn(async move {
        let mut tx = Some(cast_started_tx);
        let response = caster
            .call(req, |event| {
                if let (Some(started), Some(tx)) = (&event.started, tx.take()) {
                    let _ = tx.send(started.clone());
                }
                None
            })
            .await
            .unwrap();
        response.into_ok::<ScreenStopped>().unwrap()
    });
    let (from, offer) = tokio::time::timeout(Duration::from_secs(5), offers.recv())
        .await
        .expect("an offer in time")
        .expect("the hook is live");
    assert_eq!((from, offer.source.id.as_str()), (sharer_key, "w-1"));

    // The receiver's client, as lpview --receive: `screen.accept`.
    let mut receiver = Client::connect(&viewer_socket).await.unwrap();
    let mut req = Request::new(types::SCREEN_ACCEPT, 0);
    req.key = Some(sharer_key);
    let (seen_tx, mut seen) = tokio::sync::mpsc::unbounded_channel::<String>();
    let receiving = tokio::spawn(async move {
        let response = receiver
            .call(req, |event| {
                if let Some(started) = &event.started {
                    let _ = seen_tx.send(format!(
                        "started {} {:?}",
                        started.source_id(),
                        started.audio
                    ));
                }
                if let Some(audio) = &event.audio {
                    let _ = seen_tx.send(format!("audio {} {}", audio.seq, audio.at));
                }
                if let Some(frame) = &event.frame {
                    let _ = seen_tx.send(format!("frame {} {}", frame.seq, frame.last));
                }
                None
            })
            .await
            .unwrap();
        response.into_ok::<ScreenStopped>().unwrap()
    });
    assert_eq!(
        next_seen(&mut seen).await,
        "started w-1 Some(AudioFormat { codec: Opus, channels: 2 })"
    );
    let cast_started = tokio::time::timeout(Duration::from_secs(5), cast_started)
        .await
        .expect("the caster's started in time")
        .unwrap();
    assert_eq!(cast_started.window.as_deref(), Some("w-1"));
    until("the capture runs", || p.capture.running().len() == 1).await;
    assert!(p.capture.emit_audio(
        "w-1",
        AudioPacket {
            at: 500,
            bytes: vec![1],
        }
    ));
    assert_eq!(next_seen(&mut seen).await, "audio 0 500");
    assert!(p.capture.emit(
        "w-1",
        EncodedFrame {
            at: 900,
            key: true,
            bytes: vec![2; 10],
        }
    ));
    assert_eq!(next_seen(&mut seen).await, "frame 0 true");

    // Status on both sides.
    let mut status_client = Client::connect(&sharer_socket).await.unwrap();
    let status: ScreenStatus = status_client
        .request(Request::new(types::SCREEN_STATUS, 0))
        .await
        .unwrap();
    assert!(status.sharing[0].cast);
    assert_eq!(status.sharing[0].audio, 1);
    let mut status_client = Client::connect(&viewer_socket).await.unwrap();
    let status: ScreenStatus = status_client
        .request(Request::new(types::SCREEN_STATUS, 0))
        .await
        .unwrap();
    assert!(status.receives);
    assert!(status.watching[0].cast);
    assert!(status.offered.is_empty());

    // `lpctl screen stop` on the caster ends the cast; both calls answer.
    let mut stop = Request::new(types::SCREEN_STOP, 0);
    stop.key = Some(viewer_key);
    let mut stopper = Client::connect(&sharer_socket).await.unwrap();
    let _: ligatured::control::Empty = stopper.request(stop).await.unwrap();
    assert_eq!(casting.await.unwrap().reason, StopReason::Sharer);
    assert_eq!(receiving.await.unwrap().reason, StopReason::Sharer);
    until("the capture stopped", || p.capture.stops() == 1).await;

    // A decline over the socket, and nothing to decline after it.
    let mut req = Request::new(types::SCREEN_CAST, 0);
    req.key = Some(viewer_key);
    req.audio = Some(false);
    let mut caster = Client::connect(&sharer_socket).await.unwrap();
    let casting = tokio::spawn(async move { caster.call(req, |_| None).await });
    let _ = tokio::time::timeout(Duration::from_secs(5), offers.recv())
        .await
        .expect("an offer in time");
    let mut decline = Request::new(types::SCREEN_DECLINE, 0);
    decline.key = Some(sharer_key);
    let mut decliner = Client::connect(&viewer_socket).await.unwrap();
    let _: ligatured::control::Empty = decliner.request(decline.clone()).await.unwrap();
    let response = casting.await.unwrap().unwrap();
    let err = response.into_ok::<ScreenStopped>().unwrap_err();
    assert!(err.to_string().contains("declined"), "{err}");
    let again: Result<ligatured::control::Empty, _> = decliner.request(decline).await;
    assert!(again.is_err());
}

/// The next event a control client saw, within five seconds.
async fn next_seen(seen: &mut tokio::sync::mpsc::UnboundedReceiver<String>) -> String {
    tokio::time::timeout(Duration::from_secs(5), seen.recv())
        .await
        .expect("an event in time")
        .expect("the client is live")
}
