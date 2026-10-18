//! The attribute macro must merge user rkyv arguments with its own crate path.
//! Before that, any user rkyv attribute suppressed the injected path and forced
//! downstream crates to depend on rkyv directly.

#[qserde::Archive]
#[rkyv(derive(Debug))]
#[derive(Debug, PartialEq)]
struct Attributed {
    id: u64,
    #[rkyv(with = qserde::rkyv::with::Skip)]
    note: Option<String>,
}

#[test]
fn user_rkyv_attributes_keep_the_qserde_crate_path() {
    let value = Attributed { id: 7, note: None };
    let bytes = qserde::to_bytes(&value).expect("serialize");
    let restored: Attributed = qserde::from_bytes(&bytes).expect("deserialize");
    assert_eq!(restored, value);

    // derive(Debug) from the user attribute reached the archived type.
    let archived = qserde::rkyv::access::<ArchivedAttributed, qserde::rkyv::rancor::Error>(&bytes)
        .expect("valid archive");
    assert!(format!("{archived:?}").contains("Attributed"));
}
