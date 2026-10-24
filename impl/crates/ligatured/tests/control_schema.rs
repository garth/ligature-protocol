//! The control messages validate against `schemas/control.schema.json`.

use ligatured::control::{Empty, ErrorBody, Event, Request, Response, types};
use lp_core::wire::TypeName;

fn validator() -> jsonschema::Validator {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../schemas/control.schema.json");
    let schema: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    jsonschema::options().build(&schema).unwrap()
}

fn check<T: serde::Serialize + serde::de::DeserializeOwned>(v: &jsonschema::Validator, value: &T) {
    let mut cbor = Vec::new();
    ciborium::into_writer(value, &mut cbor).unwrap();
    let back: T = ciborium::from_reader(cbor.as_slice()).unwrap();
    let json = serde_json::to_value(&back).unwrap();
    let errors: Vec<String> = v.iter_errors(&json).map(|e| e.to_string()).collect();
    assert!(errors.is_empty(), "{errors:?}\n{json}");
}

#[test]
fn control_messages_validate() {
    let v = validator();
    for t in [
        types::INIT,
        types::STATUS,
        types::DEVICES,
        types::GRANTS,
        types::CAPS,
        types::REVOKE,
        types::PAIR_INTRODUCE,
        types::PAIR_JOIN,
        types::PAIR_CONFIRM,
        types::PAIR_ABORT,
        types::CLIPBOARD_GET,
        types::CLIPBOARD_SET,
        types::CLIPBOARD_PUT,
        types::CLIPBOARD_PASTE,
        types::NOTIFY,
        types::FILES_SEND,
        types::FILES_GET,
        types::FILES_OFFER,
        types::FILES_OFFERS,
        types::FILES_WITHDRAW,
        types::FILES_CANCEL,
        types::FILES_TRANSFERS,
        types::INPUT_LAYOUT,
        types::INPUT_LAYOUT_SET,
        types::INPUT_LAYOUT_UNSET,
        types::INPUT_STATUS,
        types::INPUT_RELEASE,
        types::SCREEN_DISPLAYS,
        types::SCREEN_START,
        types::SCREEN_STOP,
        types::SCREEN_STATUS,
    ] {
        let mut r = Request::new(t, 7);
        r.name = Some("Alice".to_owned());
        r.text = Some("hi".to_owned());
        r.pairing = Some(3);
        r.edge = Some("right".to_owned());
        r.display = Some("DP-1".to_owned());
        r.width = Some(1280);
        r.frame_rate = Some(30);
        check(&v, &r);
    }
    check(&v, &Response::ok(1, &Empty {}).unwrap());
    check(&v, &Response::error(2, ErrorBody::new("not-found", "x")));
    check(
        &v,
        &Event {
            event_type: TypeName::new(types::PAIR_CODE),
            id: 1,
            offer: None,
            code: Some("012345".to_owned()),
            progress: None,
            started: None,
            frame: None,
        },
    );
    check(
        &v,
        &Event {
            event_type: TypeName::new(types::PAIR_OFFER),
            id: 1,
            offer: Some("ligature:pair/1?b=z6Mk".to_owned()),
            code: None,
            progress: None,
            started: None,
            frame: None,
        },
    );
    assert!(
        !v.is_valid(&serde_json::json!({"type": "org.ligatureprotocol.control.nothing", "id": 1}))
    );
}
