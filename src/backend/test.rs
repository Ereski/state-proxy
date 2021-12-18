use super::{
    DiscoveryEvent, DiscoveryService, DiscoveryServiceMetrics, Endpoint,
    EndpointRef, Service, ServiceManager,
};
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

// How many iterations to run a spinlock for
const SPINLOCK_TURNS: usize = 100;

static TEST_DISCOVERY_SERVICE_NAME: &str = "test";

struct TestDiscoveryService {
    events: Vec<DiscoveryEvent>,
    metrics_recv: OneshotReceiver<Arc<DiscoveryServiceMetrics>>,
    status_send: OneshotSender<bool>,
}

impl TestDiscoveryService {
    fn new(
        events: Vec<DiscoveryEvent>,
    ) -> (
        Self,
        OneshotSender<Arc<DiscoveryServiceMetrics>>,
        OneshotReceiver<bool>,
    ) {
        let (metrics_send, metrics_recv) = oneshot::channel();
        let (status_send, status_recv) = oneshot::channel();

        (
            Self {
                events,
                metrics_recv,
                status_send,
            },
            metrics_send,
            status_recv,
        )
    }
}

impl DiscoveryService for TestDiscoveryService {
    fn name(&self) -> Arc<String> {
        Arc::new(TEST_DISCOVERY_SERVICE_NAME.to_owned())
    }

    fn run_with_sender(self, send: Sender<DiscoveryEvent>) {
        tokio::spawn(async move {
            let n_events = u64::try_from(self.events.len()).unwrap();
            for event in self.events {
                if let Err(_) = send.send(event).await {
                    let _ = self.status_send.send(false);

                    return;
                }
            }

            // Spinlock until the metrics show everything was processed
            let mut turns = SPINLOCK_TURNS;
            let metrics = self.metrics_recv.await.ok().unwrap();
            while metrics.events_processed.get() != n_events {
                if turns == 0 {
                    let _ = self.status_send.send(false);

                    return;
                }
                turns -= 1;

                task::yield_now().await;
            }

            let _ = self.status_send.send(true);
        });
    }
}

async fn init(events: Vec<DiscoveryEvent>) -> Arc<ServiceManager> {
    let (discovery_service, metrics_send, status_recv) =
        TestDiscoveryService::new(events);
    let manager = ServiceManager::new();
    manager
        .register_discovery_service(discovery_service)
        .await
        .unwrap();
    metrics_send
        .send(
            manager
                .discovery_service_metrics
                .lock()
                .await
                .get(&TEST_DISCOVERY_SERVICE_NAME.to_owned())
                .unwrap()
                .clone(),
        )
        .ok()
        .unwrap();
    assert_eq!(status_recv.await, Ok(true));

    manager
}

#[tokio::test]
async fn can_register_a_discovery_service() {
    let manager = init(Vec::new()).await;

    assert_eq!(
        *manager.registered_discovery_services.lock().await,
        hashset! { Arc::new(TEST_DISCOVERY_SERVICE_NAME.to_owned()) }
    );
}

#[tokio::test]
async fn can_add_a_service() {
    let endpoint_ref = Arc::new(EndpointRef::new(
        Arc::new(TEST_DISCOVERY_SERVICE_NAME.to_owned()),
        "0",
    ));
    let service = Service::new(
        "http",
        80,
        hashmap! {
            endpoint_ref.clone() => Endpoint::new(true, "http", ([10, 0, 0, 1], 80)),
        },
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

#[tokio::test]
async fn can_add_then_remove_a_service() {
    let endpoint_ref = Arc::new(EndpointRef::new(
        Arc::new(TEST_DISCOVERY_SERVICE_NAME.to_owned()),
        "0",
    ));
    let service = Service::new(
        "http",
        80,
        hashmap! {
            endpoint_ref.clone() => Endpoint::new(true, "http", ([10, 0, 0, 1], 80)),
        },
    );
    let manager = init(vec![
        DiscoveryEvent::add(endpoint_ref.clone(), vec![service.clone()]),
        DiscoveryEvent::remove(endpoint_ref),
    ])
    .await;

    assert!(manager.services.lock().await.is_empty());
}

#[tokio::test]
async fn can_add_then_suspend_a_service() {
    let endpoint_ref = Arc::new(EndpointRef::new(
        Arc::new(TEST_DISCOVERY_SERVICE_NAME.to_owned()),
        "0",
    ));
    let mut service = Service::new(
        "http",
        80,
        hashmap! {
            endpoint_ref.clone() => Endpoint::new(true, "http", ([10, 0, 0, 1], 80)),
        },
    );
    let manager = init(vec![
        DiscoveryEvent::add(endpoint_ref.clone(), vec![service.clone()]),
        DiscoveryEvent::suspend(endpoint_ref.clone()),
    ])
    .await;

    service
        .endpoints
        .get_mut(&endpoint_ref)
        .unwrap()
        .is_available = false;
    assert_eq!(
        *manager.services.lock().await,
        hashmap! {
            80 => service,
        }
    );
}

#[tokio::test]
async fn can_add_then_resume_an_unavailable_service() {
    let endpoint_ref = Arc::new(EndpointRef::new(
        Arc::new(TEST_DISCOVERY_SERVICE_NAME.to_owned()),
        "0",
    ));
    let mut service = Service::new(
        "http",
        80,
        hashmap! {
            endpoint_ref.clone() => Endpoint::new(false, "http", ([10, 0, 0, 1], 80)),
        },
    );
    let manager = init(vec![
        DiscoveryEvent::add(endpoint_ref.clone(), vec![service.clone()]),
        DiscoveryEvent::suspend(endpoint_ref.clone()),
    ])
    .await;

    service
        .endpoints
        .get_mut(&endpoint_ref)
        .unwrap()
        .is_available = true;
    assert_eq!(
        *manager.services.lock().await,
        hashmap! {
            80 => service,
        }
    );
}
