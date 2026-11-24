#![cfg(test)]

//! End-to-end checks for the bus wrapper. These use a local Zenoh peer
//! session (no external router is required).

use std::sync::mpsc;
use std::time::Duration;

use bus::{Bus, BusConfig, Publisher, Query, QueryableWrapper, ZenohError};

fn unique_topic(suffix: &str) -> String {
    format!("bus/test/{}/{suffix}", std::process::id())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_a_bus_clone_keeps_subscriptions_alive() {
    let bus = Bus::from(BusConfig::default()).await;
    let topic = unique_topic("drop-clone");

    let (tx, rx) = mpsc::channel::<String>();
    bus.subscribe(&topic, move |message: String| {
        let _ = tx.send(message);
    })
    .await;

    // Let the spawned subscription task declare the Zenoh subscription.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Dropping a clone used to abort every task the bus owned, including the
    // subscription above.
    drop(bus.clone());
    let _session: std::sync::Arc<bus::Session> = bus.clone().into();

    let publisher = Publisher::new(topic.clone())
        .with_session(bus.session())
        .expect("with_session cannot fail");
    publisher
        .publish(&"still here".to_string())
        .await
        .expect("publish should succeed");

    let received = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("subscription should survive dropping a Bus clone");
    assert_eq!(received, "still here");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_malformed_payload_does_not_end_a_subscription() {
    let bus = Bus::from(BusConfig::default()).await;
    let topic = unique_topic("malformed-payload");
    let session = bus.session();

    let (tx, rx) = mpsc::channel::<String>();
    bus.subscribe(&topic, move |message: String| {
        let _ = tx.send(message);
    })
    .await;

    // Let the spawned subscription task declare the Zenoh subscription.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Anything on the shared key-space can write a payload that is not this
    // type; the first one used to end the delivery loop for good.
    session
        .put(&topic, vec![0xFFu8; 4])
        .await
        .expect("a raw payload can be published");
    tokio::time::sleep(Duration::from_millis(200)).await;

    session
        .put(
            &topic,
            bus::Codec.encode(&"hello".to_string()).expect("encode"),
        )
        .await
        .expect("publish should succeed");

    let received = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("a valid payload after a malformed one must still be delivered");
    assert_eq!(received, "hello");
    assert!(
        bus.subscription_failures().is_empty(),
        "a malformed payload is skipped, not reported as an end: {:?}",
        bus.subscription_failures()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_topic_that_never_decodes_reports_the_failure() {
    let bus = Bus::from(BusConfig::default()).await;
    let topic = unique_topic("undecodable-topic");
    let session = bus.session();

    bus.subscribe(&topic, |_message: String| {}).await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    for _ in 0..(bus::MAX_CONSECUTIVE_DECODE_FAILURES + 1) {
        session
            .put(&topic, vec![0xFFu8; 4])
            .await
            .expect("a raw payload can be published");
    }

    // Give the delivery task time to drain the burst and give up.
    tokio::time::sleep(Duration::from_secs(2)).await;

    let failures = bus.subscription_failures();
    assert_eq!(
        failures.len(),
        1,
        "the ended subscription reports exactly once: {failures:?}"
    );
    assert!(
        failures[0].contains("undecodable payloads"),
        "the reason names the cause: {failures:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn query_with_timeout_without_replies_returns_an_error() {
    let bus = Bus::from(BusConfig::default()).await;
    let topic = unique_topic("no-replies");
    let query = Query::new(topic)
        .with_session(bus.session())
        .await
        .expect("with_session cannot fail");

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        query.query_with_timeout::<String, String>(&"ping".to_string(), Duration::from_millis(100)),
    )
    .await
    .expect("query_with_timeout should respect its own timeout");

    assert!(
        matches!(result, Err(ZenohError::Query(_))),
        "expected a Query error when nothing answers, got {result:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stream_handler_delivers_more_items_than_the_channel_capacity() {
    let bus = Bus::from(BusConfig::default()).await;
    let topic = unique_topic("stream");
    let session = bus.session();

    let mut queryable = QueryableWrapper::<String, String>::new(topic.clone());
    queryable
        .set_stream_handler(|_request: String, tx| async move {
            for i in 0..100u32 {
                if tx.send(Ok(i.to_string())).await.is_err() {
                    break;
                }
            }
        })
        .expect("handler can be set before start");
    queryable.init(&session).await.expect("queryable init");
    queryable.run().expect("queryable run");

    tokio::time::sleep(Duration::from_millis(300)).await;

    let query = Query::new(topic)
        .with_session(session)
        .await
        .expect("with_session cannot fail");

    let results = tokio::time::timeout(
        Duration::from_secs(5),
        query.stream_with_handler::<String, String>(&"go".to_string(), Ok),
    )
    .await
    .expect("stream handler must not deadlock")
    .expect("stream should succeed");

    let expected: Vec<String> = (0..100u32).map(|i| i.to_string()).collect();
    assert_eq!(
        results, expected,
        "every produced item should be delivered, in order"
    );

    // A handler that refuses a response surfaces as a query error, not as a
    // serialization error.
    let error = query
        .stream_with_handler::<String, String>(&"go".to_string(), |_response| {
            Err(anyhow::anyhow!("handler refused"))
        })
        .await;
    assert!(
        matches!(error, Err(ZenohError::Query(_))),
        "handler failures are query errors, got {error:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn query_surfaces_handler_errors_and_answers_payload_less_queries() {
    let bus = Bus::from(BusConfig::default()).await;
    let topic = unique_topic("handler-error");
    let session = bus.session();

    let mut queryable = QueryableWrapper::<String, String>::new(topic.clone())
        .with_handler(|_request: String| async { Err(ZenohError::Query("refused".to_string())) });
    queryable.init(&session).await.expect("queryable init");
    queryable.run().expect("queryable run");

    tokio::time::sleep(Duration::from_millis(300)).await;

    let query = Query::new(topic.clone())
        .with_session(session.clone())
        .await
        .expect("with_session cannot fail");
    let result: Result<String, ZenohError> = query.query(&"hi".to_string()).await;
    match result {
        Err(ZenohError::Query(message)) => assert!(
            message.contains("refused"),
            "the handler cause should reach the caller: {message}"
        ),
        other => panic!("expected the handler failure to surface, got {other:?}"),
    }

    // A query without a payload is answered with an error instead of being
    // dropped silently.
    let replies = session.get(topic).await.expect("query is sent");
    let reply = replies.recv_async().await.expect("an error reply");
    assert!(
        reply.result().is_err(),
        "payload-less queries receive an error reply"
    );
}
