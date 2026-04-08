use qserde::{from_bytes, load, to_bytes, Archived, Deserialize, DeserializeExt, Serialize};

#[qserde::Archive]
#[derive(Debug, PartialEq)]
struct SessionState {
    id: u64,
    name: String,
    tags: Vec<String>,
}

#[test]
fn roundtrips_with_trait_helpers() {
    let state = SessionState {
        id: 7,
        name: "alpha".to_string(),
        tags: vec!["a".to_string(), "b".to_string()],
    };

    let bytes = state.serialize().expect("serialize should succeed");
    let restored = SessionState::deserialize(&bytes).expect("deserialize should succeed");

    assert_eq!(restored, state);
}

#[test]
fn roundtrips_with_free_functions() {
    let state = SessionState {
        id: 42,
        name: "beta".to_string(),
        tags: vec!["x".to_string()],
    };

    let bytes = to_bytes(&state).expect("serialize should succeed");
    let restored = from_bytes::<SessionState>(&bytes).expect("deserialize should succeed");

    assert_eq!(restored, state);
}

#[test]
fn roundtrips_with_extremely_easy_api() {
    let state = SessionState {
        id: 99,
        name: "gamma".to_string(),
        tags: vec!["fast".to_string(), "easy".to_string()],
    };

    let bytes = state.dump().expect("dump should succeed");
    let restored = bytes.load::<SessionState>().expect("load should succeed");
    let restored2 = load::<SessionState>(&bytes).expect("free load should succeed");
    let packet = Archived::<SessionState>::from_value(&state).expect("archive should succeed");

    assert_eq!(restored, state);
    assert_eq!(restored2, state);
    assert_eq!(packet.load().expect("packet load should succeed"), state);
    assert_eq!(packet.as_bytes(), bytes.as_slice());
}

#[test]
fn prelude_feels_small() {
    use qserde::prelude::*;

    let state = SessionState {
        id: 100,
        name: "delta".to_string(),
        tags: vec!["prelude".to_string()],
    };

    let bytes = dump(&state).expect("dump should succeed");
    let restored = SessionState::load(&bytes).expect("type load should succeed");

    assert_eq!(restored, state);
}
