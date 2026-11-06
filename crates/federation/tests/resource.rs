//! Leases bound who may move bytes, credit bounds how fast, chunks bound memory.

use capability::{CapabilityId, NodeId};
use federation::{
    LeaseId, LeaseStore, ResourceError, ResourceLease, ResourceStreamMessage, StreamReceiver,
    StreamSender,
};

fn node(value: &str) -> NodeId {
    NodeId::new(value).expect("a valid node id")
}

fn capability(name: &str) -> CapabilityId {
    CapabilityId::new("resource", name).expect("a valid id")
}

fn lease(id: &str, expires_at_ms: Option<u64>, max_bytes: Option<u64>) -> ResourceLease {
    ResourceLease {
        id: LeaseId::new(id),
        capability: capability("fs.read"),
        holder: node("node:reader"),
        granted_at_ms: 100,
        expires_at_ms,
        max_bytes,
        used_bytes: 0,
    }
}

#[test]
fn a_lease_is_bound_to_its_holder_and_its_deadline() {
    let mut store = LeaseStore::new();
    store
        .grant(lease("l-1", Some(200), Some(8)))
        .expect("granted");

    let id = LeaseId::new("l-1");
    store
        .authorize(&id, &node("node:reader"), 150, 8)
        .expect("valid");
    let wrong = store
        .authorize(&id, &node("node:other"), 150, 1)
        .expect_err("not the holder");
    assert!(
        matches!(wrong, ResourceError::WrongHolder { .. }),
        "{wrong:?}"
    );
    let expired = store
        .authorize(&id, &node("node:reader"), 200, 1)
        .expect_err("the deadline passed");
    assert!(
        matches!(expired, ResourceError::Expired { .. }),
        "{expired:?}"
    );
}

#[test]
fn a_revoked_lease_is_unknown() {
    let mut store = LeaseStore::new();
    store.grant(lease("l-2", None, None)).expect("granted");
    assert!(store.revoke(&LeaseId::new("l-2")));

    let error = store
        .authorize(&LeaseId::new("l-2"), &node("node:reader"), 150, 1)
        .expect_err("revoked");

    assert!(matches!(error, ResourceError::UnknownLease { .. }));
}

#[test]
fn a_lease_quota_is_charged_and_enforced() {
    let mut store = LeaseStore::new();
    store.grant(lease("l-3", None, Some(8))).expect("granted");
    let id = LeaseId::new("l-3");
    let holder = node("node:reader");

    store.charge(&id, &holder, 150, 5).expect("within quota");
    assert_eq!(store.get(&id).expect("a lease").used_bytes, 5);
    store
        .charge(&id, &holder, 150, 3)
        .expect("exactly the quota");
    let error = store
        .charge(&id, &holder, 150, 1)
        .expect_err("the quota is spent");

    assert!(
        matches!(error, ResourceError::QuotaExceeded { remaining: 0, .. }),
        "{error:?}"
    );
}

#[test]
fn a_sender_waits_for_credit_and_the_receiver_opens_the_window() {
    let mut sender = StreamSender::new(LeaseId::new("l-4"), 4);
    let mut receiver = StreamReceiver::new(LeaseId::new("l-4"), "file:///demo", 0, 4);

    assert!(sender.next_chunk(b"abcdefgh").is_none(), "no credit yet");
    let open = receiver.open();
    sender.grant(&open).expect("the open grants credit");
    let chunk = sender
        .next_chunk(b"abcdefgh")
        .expect("credit allows one chunk");
    receiver.accept(&chunk).expect("in order and within credit");

    assert_eq!(receiver.data(), b"abcd");
    assert!(sender.next_chunk(b"efgh").is_none(), "credit is spent");
}

#[test]
fn a_receiver_refuses_an_out_of_order_chunk() {
    let mut receiver = StreamReceiver::new(LeaseId::new("l-5"), "file:///demo", 0, 8);
    let _ = receiver.open();
    let chunk = ResourceStreamMessage::Chunk {
        lease: LeaseId::new("l-5"),
        seq: 1,
        bytes: b"abcd".to_vec(),
    };

    let error = receiver.accept(&chunk).expect_err("seq 0 has not arrived");

    assert!(matches!(
        error,
        ResourceError::OutOfOrder {
            expected: 0,
            found: 1,
            ..
        }
    ));
}

#[test]
fn a_chunk_beyond_the_granted_credit_is_refused() {
    let mut receiver = StreamReceiver::new(LeaseId::new("l-6"), "file:///demo", 0, 4);
    let _ = receiver.open();
    let chunk = ResourceStreamMessage::Chunk {
        lease: LeaseId::new("l-6"),
        seq: 0,
        bytes: b"abcdef".to_vec(),
    };

    let error = receiver.accept(&chunk).expect_err("more bytes than credit");

    assert!(
        matches!(error, ResourceError::CreditExhausted { .. }),
        "{error:?}"
    );
}

#[test]
fn replenishing_credit_resumes_the_stream_and_close_verifies_it() {
    let data = b"abcdefgh";
    let mut sender = StreamSender::new(LeaseId::new("l-7"), 4);
    let mut receiver = StreamReceiver::new(LeaseId::new("l-7"), "file:///demo", 0, 4);
    let open = receiver.open();
    sender.grant(&open).expect("opened");

    let first = sender.next_chunk(data).expect("first chunk");
    receiver.accept(&first).expect("accepted");
    let credit = receiver.replenish().expect("half the window is consumed");
    sender.grant(&credit).expect("credit applied");
    let second = sender.next_chunk(&data[4..]).expect("second chunk");
    receiver.accept(&second).expect("accepted");

    assert_eq!(receiver.data(), data);
    assert_eq!(sender.sent(), 8);
    let close = sender.close(data);
    receiver.accept(&close).expect("checksum and length match");
}

#[test]
fn a_close_with_the_wrong_checksum_is_refused() {
    let data = b"abcd";
    let mut sender = StreamSender::new(LeaseId::new("l-8"), 4);
    let mut receiver = StreamReceiver::new(LeaseId::new("l-8"), "file:///demo", 0, 4);
    let open = receiver.open();
    sender.grant(&open).unwrap();
    let chunk = sender.next_chunk(data).unwrap();
    receiver.accept(&chunk).unwrap();

    let tampered = ResourceStreamMessage::Close {
        lease: LeaseId::new("l-8"),
        total: 4,
        checksum: federation::checksum(b"not the same bytes"),
    };
    let error = receiver
        .accept(&tampered)
        .expect_err("the bytes do not match");

    assert!(
        matches!(error, ResourceError::ChecksumMismatch { .. }),
        "{error:?}"
    );
}

#[test]
fn a_resumed_stream_starts_at_its_offset() {
    let mut receiver = StreamReceiver::new(LeaseId::new("l-9"), "file:///demo", 4, 4);
    let open = receiver.open();

    match &open {
        ResourceStreamMessage::Open { offset, credit, .. } => {
            assert_eq!(*offset, 4);
            assert_eq!(*credit, 4);
        }
        other => panic!("expected an open, got {other:?}"),
    }
    let chunk = ResourceStreamMessage::Chunk {
        lease: LeaseId::new("l-9"),
        seq: 0,
        bytes: b"efgh".to_vec(),
    };
    receiver.accept(&chunk).expect("resumes at four");
    assert_eq!(receiver.resume_offset(), 8);
}
