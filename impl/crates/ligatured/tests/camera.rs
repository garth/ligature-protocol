//! Two daemons of one person with a fake camera (milestone 10, LP-206): the
//! camera family is the screen family under its own names, run by the same
//! sync. The viewer lists the sharer's cameras, watches one by default and by
//! id with its microphone, presents one to its programs through a fake
//! presenter, and a camera cast to it is presented or shown; shares end from
//! each side, and the control socket carries all of it under `camera.*`.

use std::sync::Arc;
use std::time::Duration;

use ligatured::daemon::Daemon;
use ligatured::screen::{
    AudioPacket, CastSource, EncodedFrame, FakeCapture, FakePresenter, ScreenSync, ShareControl,
    Viewed, Watching,
};
use lp_core::camera::{CAMERA_TYPE, Camera, Facing, camera_params};
use lp_core::facts::Role;
use lp_core::paths::Locations;
use lp_core::screen::{
    AudioFormat, Format, Offer, SCREEN_TYPE, ScreenParams, SourceKind, Start, StopReason,
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
    presenter: Arc<FakePresenter>,
    _dirs: Vec<tempfile::TempDir>,
}

fn cameras() -> Vec<Camera> {
    vec![
        Camera {
            id: "back".to_owned(),
            name: Some("Back camera".to_owned()),
            facing: Facing::Back,
            width: 4080,
            height: 3072,
            frame_rates: vec![30, 60],
        },
        Camera {
            id: "front".to_owned(),
            name: Some("Front camera".to_owned()),
            facing: Facing::Front,
            width: 1920,
            height: 1080,
            frame_rates: vec![30],
        },
    ]
}

/// A sharer with two cameras and a microphone, and a viewer that takes up to
/// 1280 by 720, plays mono Opus and presents.
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
    let capture = FakeCapture::of_cameras(cameras());
    capture.set_audio_encodes(vec![AudioFormat::OPUS_MONO]);
    let sharer_sync = sharer.attach_camera(Some(capture.clone()), None, None);
    let presenter = FakePresenter::new();
    let viewer_sync = viewer.attach_camera(
        None,
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
    drive_events(sharer.clone(), sharer_events);
    drive_events(viewer.clone(), viewer_events);
    viewer
        .manager()
        .connect(sharer.manager().endpoint().local_addr().unwrap())
        .await
        .unwrap();
    until("manifests exchanged", || {
        viewer.registry().peers_offering(CAMERA_TYPE).len() == 1
            && sharer.registry().peers_offering(CAMERA_TYPE).len() == 1
    })
    .await;
    Pair {
        sharer,
        capture,
        sharer_sync,
        viewer,
        viewer_sync,
        presenter,
        _dirs: vec![sharer_dir, viewer_dir],
    }
}

fn start(camera: Option<&str>, audio: bool) -> Start {
    Start {
        display: camera.map(str::to_owned),
        window: None,
        formats: Vec::new(),
        width: 1920,
        height: 1080,
        frame_rate: 60,
        audio: if audio {
            vec![AudioFormat::OPUS_MONO]
        } else {
            Vec::new()
        },
    }
}

async fn next(watching: &mut Watching) -> Viewed {
    tokio::time::timeout(Duration::from_secs(5), watching.frames.recv())
        .await
        .expect("something in time")
        .expect("the share still runs")
}

async fn stopped_with(watching: &mut Watching) -> StopReason {
    loop {
        if let Viewed::Stopped(reason) = next(watching).await {
            return reason;
        }
    }
}

/// What the sharer's registry says of the viewer's camera params.
fn viewer_params_seen(p: &Pair) -> Option<ScreenParams> {
    p.sharer
        .registry()
        .peers()
        .into_iter()
        .filter(|caps| caps.peer == *p.viewer.binding().public())
        .flat_map(|caps| caps.capabilities)
        .find_map(|d| camera_params(&d))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cameras_are_listed_and_one_is_watched_with_its_microphone() {
    let p = pair().await;
    let sharer_key = *p.sharer.binding().public();
    // The family is the camera's own: neither device offers a screen, and
    // the viewer says it presents.
    assert!(p.viewer.registry().peers_offering(SCREEN_TYPE).is_empty());
    assert!(viewer_params_seen(&p).is_some_and(|v| v.views && v.presents && !v.receives));
    assert!(p.viewer_sync.status().presents);
    assert_eq!(p.viewer_sync.cameras(&sharer_key).await.unwrap(), cameras());
    assert_eq!(p.sharer_sync.local_cameras(), cameras());
    assert!(p.viewer_sync.local_cameras().is_empty());

    // By default the first camera, fitted to what the viewer takes, with the
    // microphone since both have a format for it.
    let mut watching = p
        .viewer_sync
        .watch(&sharer_key, start(None, true))
        .await
        .unwrap();
    assert_eq!(watching.started.source_id(), "back");
    assert_eq!(
        (watching.started.width, watching.started.height),
        (956, 720),
        "4080x3072 fitted into 1280x720"
    );
    assert_eq!(watching.started.frame_rate, 30);
    assert_eq!(watching.started.audio, Some(AudioFormat::OPUS_MONO));
    let settings = p.capture.last_settings().expect("a share started");
    assert_eq!(
        (settings.kind, settings.display.as_str(), settings.audio),
        (SourceKind::Camera, "back", Some(AudioFormat::OPUS_MONO))
    );
    until("the capture runs", || {
        p.capture.running() == vec!["back".to_owned()]
    })
    .await;
    assert!(p.capture.emit_audio(
        "back",
        AudioPacket {
            at: 10,
            bytes: vec![5; 20],
        }
    ));
    assert!(p.capture.emit(
        "back",
        EncodedFrame {
            at: 20,
            key: true,
            bytes: vec![0, 0, 0, 1, 0x65, 7],
        }
    ));
    assert!(matches!(
        next(&mut watching).await,
        Viewed::Audio { seq: 0, at: 10, ref bytes } if bytes.len() == 20
    ));
    assert!(matches!(
        next(&mut watching).await,
        Viewed::Frame { seq: 0, key: true, ref bytes, .. } if bytes.len() == 6
    ));

    // The other camera by its id, alongside; the same camera again is busy,
    // and one the sharer lacks is refused.
    let mut front = p
        .viewer_sync
        .watch(&sharer_key, start(Some("front"), false))
        .await
        .unwrap();
    assert_eq!(front.started.source_id(), "front");
    assert_eq!(front.started.audio, None, "no microphone was asked for");
    let err = p
        .viewer_sync
        .watch(&sharer_key, start(Some("back"), false))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("busy"), "{err}");
    let err = p
        .viewer_sync
        .watch(&sharer_key, start(Some("periscope"), false))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("no-display"), "{err}");

    // The viewer stops one; the sharer's user stops the other.
    watching.control.send(ShareControl::Stop).unwrap();
    assert_eq!(stopped_with(&mut watching).await, StopReason::Viewer);
    until("one capture stopped", || p.capture.stops() == 1).await;
    assert_eq!(p.sharer_sync.stop_sharing(), 1);
    assert_eq!(stopped_with(&mut front).await, StopReason::Sharer);
    until("nothing is shared", || {
        p.sharer_sync.status().sharing.is_empty() && p.viewer_sync.status().watching.is_empty()
    })
    .await;

    // A device that shares no camera cannot be watched, by name.
    let err = p
        .sharer_sync
        .cameras(p.viewer.binding().public())
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("does not share its camera"),
        "{err}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_camera_is_presented_and_a_cast_one_is_presented_or_shown() {
    let p = pair().await;
    let (sharer_key, viewer_key) = (*p.sharer.binding().public(), *p.viewer.binding().public());

    // The viewer presents the sharer's camera to its own programs: frames
    // and sound go to the presenter, not to a viewer.
    let presenting = p
        .viewer_sync
        .present(&sharer_key, start(Some("front"), true))
        .await
        .unwrap();
    assert_eq!(presenting.started.source_id(), "front");
    let begun = p.presenter.started();
    assert_eq!(begun.len(), 1);
    assert_eq!(begun[0].0, sharer_key.fingerprint(), "named by the device");
    assert_eq!(begun[0].1, presenting.started);
    assert_eq!(p.presenter.running(), 1);
    assert!(p.viewer_sync.status().watching[0].presented);
    until("the capture runs", || {
        p.capture.running() == vec!["front".to_owned()]
    })
    .await;
    p.capture.emit(
        "front",
        EncodedFrame {
            at: 1,
            key: true,
            bytes: vec![1; 50],
        },
    );
    p.capture.emit_audio(
        "front",
        AudioPacket {
            at: 2,
            bytes: vec![2; 10],
        },
    );
    until("the presenter has a frame and a packet", || {
        p.presenter.frames() == 1 && p.presenter.audio() == 1
    })
    .await;
    // The sharer stops: the presented camera goes away.
    assert_eq!(p.sharer_sync.stop_sharing(), 1);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), presenting.ended)
            .await
            .expect("the presenting ends in time")
            .unwrap(),
        StopReason::Sharer
    );
    until("the device is gone", || p.presenter.running() == 0).await;

    // A cast that asks to be presented is taken by the presenter at once,
    // with nobody asked and nothing that shows casts set.
    let casting = p
        .sharer_sync
        .cast(
            &viewer_key,
            CastSource {
                present: true,
                ..CastSource::default()
            },
            true,
        )
        .await
        .unwrap();
    assert_eq!(casting.started.source_id(), "back", "the first camera");
    assert_eq!(casting.started.audio, Some(AudioFormat::OPUS_MONO));
    until("the presenter runs again", || p.presenter.running() == 1).await;
    assert_eq!(p.presenter.started().len(), 2);
    let status = p.viewer_sync.status();
    assert!(status.watching[0].cast && status.watching[0].presented);
    // The receiver stops it: the caster hears `viewer`.
    assert_eq!(p.viewer_sync.stop_with(&sharer_key, StopReason::Viewer), 1);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), casting.ended)
            .await
            .expect("the cast ends in time")
            .unwrap(),
        StopReason::Viewer
    );
    until("nothing is presented", || p.presenter.running() == 0).await;

    // Without presenting, a cast needs something that shows it; the viewer
    // has none yet.
    let err = p
        .sharer_sync
        .cast(&viewer_key, CastSource::default(), false)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("does not show casts"), "{err}");
    let (offers_tx, mut offers) = tokio::sync::mpsc::unbounded_channel::<(_, Offer)>();
    p.viewer_sync.set_on_offer(Arc::new(move |peer, offer| {
        let _ = offers_tx.send((peer, offer));
    }));
    p.viewer_sync.advertise().await;
    until("the sharer sees the viewer receive", || {
        viewer_params_seen(&p).is_some_and(|v| v.receives)
    })
    .await;
    let caster = p.sharer_sync.clone();
    let casting = tokio::spawn(async move {
        caster
            .cast(
                &viewer_key,
                CastSource {
                    display: Some("front".to_owned()),
                    ..CastSource::default()
                },
                false,
            )
            .await
    });
    let (from, offer) = tokio::time::timeout(Duration::from_secs(5), offers.recv())
        .await
        .expect("an offer in time")
        .expect("the hook is live");
    assert_eq!(from, sharer_key);
    assert_eq!(
        (offer.source.kind, offer.source.id.as_str(), offer.present),
        (SourceKind::Camera, "front", false)
    );
    let mut shown = p.viewer_sync.accept(&sharer_key, false).await.unwrap();
    assert_eq!(shown.started.source_id(), "front");
    let casting = casting.await.unwrap().unwrap();
    assert_eq!(p.presenter.running(), 0, "shown, not presented");
    shown.control.send(ShareControl::Stop).unwrap();
    assert_eq!(stopped_with(&mut shown).await, StopReason::Viewer);
    assert_eq!(casting.ended.await.unwrap(), StopReason::Viewer);

    // A device that no longer presents is not asked to.
    p.presenter.set_available(false);
    p.viewer_sync.advertise().await;
    until("the sharer sees the viewer stop presenting", || {
        viewer_params_seen(&p).is_some_and(|v| !v.presents)
    })
    .await;
    let err = p
        .sharer_sync
        .cast(
            &viewer_key,
            CastSource {
                present: true,
                ..CastSource::default()
            },
            false,
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("does not present"), "{err}");
    let err = p
        .viewer_sync
        .present(&sharer_key, start(None, false))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("cannot present"), "{err}");
}

/// The control socket under `camera.*`, as `lpctl camera` and `lpview
/// --camera` use it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cameras_over_the_control_socket() {
    use ligatured::control::{CameraList, Client, Empty, Request, ScreenStopped, serve, types};
    use ligatured::screen::ScreenStatus;
    let p = pair().await;
    let (sharer_key, viewer_key) = (*p.sharer.binding().public(), *p.viewer.binding().public());
    let dir = tempfile::tempdir().unwrap();
    let (sharer_socket, viewer_socket) = (dir.path().join("s.sock"), dir.path().join("v.sock"));
    let _sharer_server = serve(p.sharer.clone(), &sharer_socket).unwrap();
    let _viewer_server = serve(p.viewer.clone(), &viewer_socket).unwrap();

    // The listing: another device's, and this device's own.
    let mut client = Client::connect(&viewer_socket).await.unwrap();
    let mut req = Request::new(types::CAMERA_LIST, 0);
    req.key = Some(sharer_key);
    let listed: CameraList = client.request(req).await.unwrap();
    assert_eq!(listed.cameras, cameras());
    let own: CameraList = client
        .request(Request::new(types::CAMERA_LIST, 0))
        .await
        .unwrap();
    assert!(own.cameras.is_empty());
    let mut sharer_client = Client::connect(&sharer_socket).await.unwrap();
    let own: CameraList = sharer_client
        .request(Request::new(types::CAMERA_LIST, 0))
        .await
        .unwrap();
    assert_eq!(own.cameras, cameras());
    // A daemon with no screen attached says so for the screen's requests.
    let none = sharer_client
        .call(Request::new(types::SCREEN_STATUS, 0), |_| None)
        .await
        .unwrap();
    assert!(none.error.is_some());

    // `camera.start`: the events carry the camera's type names.
    let mut watcher = Client::connect(&viewer_socket).await.unwrap();
    let mut req = Request::new(types::CAMERA_START, 0);
    req.key = Some(sharer_key);
    req.camera = Some("front".to_owned());
    req.audio = Some(true);
    let (seen_tx, mut seen) = tokio::sync::mpsc::unbounded_channel::<String>();
    let watching = tokio::spawn(async move {
        let response = watcher
            .call(req, |event| {
                let kind = event.event_type.as_str().rsplit('.').next().unwrap_or("");
                let family = event.event_type.as_str().contains(".camera.");
                let _ = seen_tx.send(format!("{kind} {family}"));
                None
            })
            .await
            .unwrap();
        response.into_ok::<ScreenStopped>().unwrap()
    });
    let next_seen = |seen: &mut tokio::sync::mpsc::UnboundedReceiver<String>| {
        let got = seen.try_recv();
        got.ok()
    };
    until("started is seen", || {
        next_seen(&mut seen).is_some_and(|s| s == "started true")
    })
    .await;
    until("the capture runs", || p.capture.running().len() == 1).await;
    p.capture.emit_audio(
        "front",
        AudioPacket {
            at: 1,
            bytes: vec![1],
        },
    );
    until("audio is seen", || {
        next_seen(&mut seen).is_some_and(|s| s == "audio true")
    })
    .await;
    p.capture.emit(
        "front",
        EncodedFrame {
            at: 2,
            key: true,
            bytes: vec![3; 9],
        },
    );
    until("a frame is seen", || {
        next_seen(&mut seen).is_some_and(|s| s == "frame true")
    })
    .await;
    let status: ScreenStatus = client
        .request(Request::new(types::CAMERA_STATUS, 0))
        .await
        .unwrap();
    assert!(status.presents && status.watching.len() == 1 && !status.watching[0].presented);
    let mut stop = Request::new(types::CAMERA_STOP, 0);
    stop.key = Some(viewer_key);
    let _: Empty = sharer_client.request(stop).await.unwrap();
    assert_eq!(watching.await.unwrap().reason, StopReason::Sharer);

    // `camera.present`: started, then the response when it ends.
    let mut presenter_client = Client::connect(&viewer_socket).await.unwrap();
    let mut req = Request::new(types::CAMERA_PRESENT, 0);
    req.key = Some(sharer_key);
    let (started_tx, started) = tokio::sync::oneshot::channel();
    let presenting = tokio::spawn(async move {
        let mut tx = Some(started_tx);
        let response = presenter_client
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
    let started = tokio::time::timeout(Duration::from_secs(5), started)
        .await
        .expect("started in time")
        .unwrap();
    assert_eq!(started.source_id(), "back");
    assert_eq!(
        started.audio,
        Some(AudioFormat::OPUS_MONO),
        "the microphone by default"
    );
    assert_eq!(p.presenter.running(), 1);
    let mut stop = Request::new(types::CAMERA_STOP, 0);
    stop.key = Some(sharer_key);
    let _: Empty = client.request(stop).await.unwrap();
    assert_eq!(presenting.await.unwrap().reason, StopReason::Viewer);
    until("nothing is presented", || p.presenter.running() == 0).await;

    // `camera.cast` with `present`: the receiver presents it.
    let mut caster = Client::connect(&sharer_socket).await.unwrap();
    let mut req = Request::new(types::CAMERA_CAST, 0);
    req.key = Some(viewer_key);
    req.camera = Some("front".to_owned());
    req.present = Some(true);
    let casting = tokio::spawn(async move {
        let response = caster.call(req, |_| None).await.unwrap();
        response.into_ok::<ScreenStopped>().unwrap()
    });
    until("the cast is presented", || p.presenter.running() == 1).await;
    assert_eq!(p.presenter.started().last().unwrap().1.source_id(), "front");
    let mut stop = Request::new(types::CAMERA_STOP, 0);
    stop.key = Some(viewer_key);
    let _: Empty = sharer_client.request(stop).await.unwrap();
    assert_eq!(casting.await.unwrap().reason, StopReason::Sharer);
}
