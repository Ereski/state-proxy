use super::{
    DiscoveryEvent, DiscoveryService, Endpoint, EndpointRef, Service,
    ServiceManager,
};
use crate::matchmaking::Matchmaker;
use maplit::{hashmap, hashset};
use pretty_assertions::assert_eq;
use std::sync::Arc;
use tokio::{
    sync::{
        mpsc::Sender,
        oneshot::{self, Receiver as OneshotReceiver, Sender as OneshotSender},
    },
    task,
};

struct TestDiscoveryService {
    events: Vec<DiscoveryEvent>,
    status_send: OneshotSender<bool>,
}

impl TestDiscoveryService {
    fn new(events: Vec<DiscoveryEvent>) -> (Self, OneshotReceiver<bool>) {
        let (status_send, status_recv) = oneshot::channel();

        (
            Self {
                events,
                status_send,
            },
            status_recv,
        )
    }
}

impl DiscoveryService for TestDiscoveryService {
    fn name(&self) -> Arc<String> {
        Arc::new("test".to_owned())
    }

    fn run_with_sender(self, send: Sender<DiscoveryEvent>) {
        tokio::spawn(async move {
            let full_capacity = send.capacity();
            for event in self.events {
                match send.send(event).await {
                    Ok(_) => (),
                    Err(_) => {
                        let _ = self.status_send.send(false);

                        return;
                    }
                }
            }

            // Wait until all events are consumed
            while send.capacity() != full_capacity {
                task::yield_now().await;
            }

            let _ = self.status_send.send(true);
        });
    }
}

async fn init(events: Vec<DiscoveryEvent>) -> ServiceManager {
    let (discovery_service, status_recv) = TestDiscoveryService::new(events);
    let mut manager = ServiceManager::new(Matchmaker::new());
    manager
        .register_discovery_service(discovery_service)
        .unwrap();
    assert_eq!(status_recv.await, Ok(true));

    manager
}

#[tokio::test]
async fn can_register_a_discovery_service() {
    let manager = init(Vec::new()).await;

    assert_eq!(
        manager.registered_discovery_services,
        hashset! { Arc::new("test".to_owned()) }
    );
}

#[tokio::test]
async fn can_process_an_add_event() {
    let endpoint_ref =
        Arc::new(EndpointRef::new(Arc::new("test".to_owned()), "0"));
    let service = Service::new(
        "http",
        80,
        vec![(
            endpoint_ref.clone(),
            Endpoint::new("http", ([10, 0, 0, 1], 80)),
        )],
    );
    let manager = init(vec![DiscoveryEvent::add(
        endpoint_ref,
        vec![service.clone()],
    )])
    .await;

    assert_eq!(
        *manager.services.lock().await,
        hashmap! {
            80 => service,
        }
    );
}
