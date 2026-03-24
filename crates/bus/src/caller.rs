use std::sync::Arc;
use rkyv::{Archive, Serialize};
use rkyv::rancor::{Error, Strategy};
use rkyv::ser::allocator::ArenaHandle;
use rkyv::ser::Serializer;
use rkyv::ser::sharing::Share;
use rkyv::util::AlignedVec;
use zenoh::Session;
use crate::ZenohError;

pub struct Caller {
    name: String,
    session: Option<Arc<Session>>,
}

impl Caller {
    pub fn new(name: String, session: Option<Arc<Session>>) -> Self {
        Self{
            name,
            session,
        }
    }
    pub async fn call<Q, R>(&self, payload: &Q) -> Result<R, ZenohError> where
        Q: Archive,
        for<'a> Q: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,
        R: Archive,
        R::Archived: rkyv::Deserialize<R, rkyv::api::high::HighDeserializer<Error>>,{
        let session = self.session.clone();
        if let Some(session) = session {
            let client = crate::query::Query::new(self.name.clone())
                .with_session(session).await.unwrap();

            client.query(payload).await
        }
        else {
            Err(crate::error::ZenohError::NotConnected)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::caller::Arc;
    use crate::{Bus, BusConfig};
    use crate::callable::Callable;
    use crate::caller::Caller;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_caller() {
        let config = BusConfig::default();
        let bus = Bus::from(config).await;

        let callable = Callable::<String, String>::new("hello",bus.clone().into());
        let mut callable = callable.with_handler(|q| async move {
            println!("IN {:?}", q);
            Ok(q.to_uppercase())
        });

       callable.start().await.unwrap();

        // Wait for handler to be ready
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let session: zenoh::Session = bus.clone().into();
        let  handle1 = tokio::spawn(async move {
            let caller = Caller::new("hello".into(), Some(Arc::new(session)));
            let result = caller.call::<String,String>(&"hello call".to_string()).await.unwrap();
            println!("{}", result);
        });

        let session: zenoh::Session = bus.clone().into();
        let handle2 = tokio::spawn(async move {
            let caller = Caller::new("hello".into(), Some(Arc::new(session)));
            let result = caller.call::<String,String>(&"hello call 001".to_string()).await.unwrap();
            println!("{}", result);
        });

        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        handle2.abort();
        handle1.abort();
    }
}