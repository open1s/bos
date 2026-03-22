use brainos_common::{setup_bus, setup_logging};
use tokio::signal;

#[derive(Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Message {
    pub id: String,
    pub content: String,
    pub timestamp: u64,
}

impl Message {
    pub fn new(content: &str) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            content: content.to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        }
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct QueryRequest {
    pub question: String,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct QueryResponse {
    pub answer: String,
    pub timestamp: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    setup_logging()?;

    println!("╔══════════════════════════════════════╗");
    println!("║  Pub/Sub & Query Demo - Phase 1 Plan 03      ║");
    println!("╚══════════════════════════════════════╝\n");

    let session = setup_bus(None).await?;

    println!("Test 1: Publish/Subscribe Pattern");
    println!("{}", "-".repeat(40));

    let publish_topic = "demo/pubsub/events";
    let publisher = bus::Publisher::new(publish_topic);
    println!("✓ Created publisher on topic: {}", publish_topic);

    let mut subscriber = bus::Subscriber::<Message>::new(publish_topic);
    subscriber.init(session.clone()).await?;
    println!("✓ Created subscriber on topic: {}", publish_topic);

    let mut subscriber_receiver = subscriber.clone();
    tokio::spawn(async move {
        let mut count = 0;
        while let Some(data) = subscriber_receiver.recv().await {
            println!("  [Subscriber] Received message {}: {:?}", count, data);
            if count >= 2 {
                break;
            }
            count += 1;
        }
        println!("  [Subscriber] Finished receiving");
    });

    for i in 1..=3 {
        let msg = Message::new(&format!("Message {}", i));
        publisher.publish(&session, &msg).await?;
        println!("  [Publisher] Published message {} to {}", i, publish_topic);
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    println!();

    println!("Test 2: Query/Response Pattern");
    println!("{}", "-".repeat(40));

    let query_topic = "demo/query/answer";
    let mut queryable = bus::QueryableWrapper::<QueryRequest, QueryResponse>::new(query_topic)
        .with_handler(|req| async move {
            let answer = match req.question.as_str() {
                "what is 1+1?" => QueryResponse {
                    answer: "2".to_string(),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                },
                "what is 2*3?" => QueryResponse {
                    answer: "6".to_string(),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                },
                _ => {
                    return Ok(QueryResponse {
                        answer: "unknown".to_string(),
                        timestamp: 0,
                    })
                }
            };
            Ok(answer)
        });

    queryable.init(&session).await?;
    println!("✓ Created queryable on topic: {}", query_topic);

    let query_handle = queryable.clone().into_task()?;
    println!("✓ Started queryable task");

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let mut query_wrapper = bus::QueryWrapper::new(query_topic);
    query_wrapper.init(session.clone()).await?;

    let req1 = QueryRequest {
        question: "what is 1+1?".to_string(),
    };
    let result1 = query_wrapper.query_bytes(&bus::Codec.encode(&req1)?).await?;
    if let Some(response_bytes) = result1.first() {
        let ans1: QueryResponse = bus::Codec.decode(response_bytes)?;
        println!("  Query: {} → Answer: {}", req1.question, ans1.answer);
    } else {
        println!("  Query: {} → No response", req1.question);
    }

    let req2 = QueryRequest {
        question: "what is 2*3?".to_string(),
    };
    let result2 = query_wrapper.query_bytes(&bus::Codec.encode(&req2)?).await?;
    if let Some(response_bytes) = result2.first() {
        let ans2: QueryResponse = bus::Codec.decode(response_bytes)?;
        println!("  Query: {} → Answer: {}", req2.question, ans2.answer);
    } else {
        println!("  Query: {} → No response", req2.question);
    }

    let req3 = QueryRequest {
        question: "what is 9*9?".to_string(),
    };
    let result3 = query_wrapper
        .query_bytes_with_timeout(&bus::Codec.encode(&req3)?, tokio::time::Duration::from_millis(100))
        .await;
    match result3 {
        Ok(results) => {
            if let Some(response_bytes) = results.first() {
                let ans3: QueryResponse = bus::Codec.decode(response_bytes)?;
                if ans3.answer == "unknown" {
                    println!("  Query: {} → No answer (service doesn't support that)", req3.question);
                } else {
                    println!("  Query: {} → Answer: {}", req3.question, ans3.answer);
                }
            } else {
                println!("  Query: {} → No response", req3.question);
            }
        }
        Err(_) => {
            println!("  Query: {} → Timeout or No response", req3.question);
        }
    }

    println!();
    println!("{}", "=".repeat(50));
    println!("Pub/Sub & Query Test Complete");
    println!("{}", "=".repeat(50));
    println!();

    query_handle.abort();
    println!("Press Ctrl+C to exit...");
    signal::ctrl_c().await?;
    println!("\nGoodbye!");

    Ok(())
}
