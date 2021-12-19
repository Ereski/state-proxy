use super::{
    DiscoveryEvent, Endpoint, EndpointId, Service, ServiceDiscovery,
    ServiceDiscoveryMetrics, ServiceManager,
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

static TEST_SERVICE_DISCOVERY_NAME: &str = "test";

struct TestServiceDiscovery {
    events: Vec<DiscoveryEvent>,
    metrics_recv: OneshotReceiver<Arc<ServiceDiscoveryMetrics>>,
    status_send: OneshotSender<bool>,
}

impl TestServiceDiscovery {
    fn new(
        events: Vec<DiscoveryEvent>,
    ) -> (
        Self,
        OneshotSender<Arc<ServiceDiscoveryMetrics>>,
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

impl ServiceDiscovery for TestServiceDiscovery {
    fn name(&self) -> Arc<String> {
        Arc::new(TEST_SERVICE_DISCOVERY_NAME.to_owned())
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
    let (service_discovery, metrics_send, status_recv) =
        TestServiceDiscovery::new(events);
    let manager = ServiceManager::new();
    manager
        .register_service_discovery(service_discovery)
        .await
        .unwrap();
    metrics_send
        .send(
            manager
                .service_discovery_metrics
                .lock()
                .await
                .get(&TEST_SERVICE_DISCOVERY_NAME.to_owned())
                .unwrap()
                .clone(),
        )
        .ok()
        .unwrap();
    assert_eq!(status_recv.await, Ok(true));

    manager
}

#[tokio::test]
async fn can_register_a_service_discovery() {
    let manager = init(Vec::new()).await;

    assert_eq!(
        *manager.registered_service_discovery_names.lock().await,
        hashset! { Arc::new(TEST_SERVICE_DISCOVERY_NAME.to_owned()) }
    );
}

#[tokio::test]
async fn can_add_a_service() {
    let manager = init(vec![DiscoveryEvent::add(
        "0",
        true,
        80,
        "http",
        ([10, 0, 0, 1], 80),
        "http",
    )])
    .await;

    assert_eq!(
        *manager.port_service_map.lock().await,
        hashmap! {
            80 => Service::new(
                80,
                "http",
                hashmap! {
                    EndpointId::new(Arc::new(TEST_SERVICE_DISCOVERY_NAME.to_owned()), "0") =>
                        Endpoint::new(true, ([10, 0, 0, 1], 80), "http"),
                },
            ),
        }
    );
}

#[tokio::test]
async fn can_add_then_delete_a_service() {
    let manager = init(vec![
        DiscoveryEvent::add("0", true, 80, "http", ([10, 0, 0, 1], 80), "http"),
        DiscoveryEvent::delete("0"),
    ])
    .await;

    assert!(manager.port_service_map.lock().await.is_empty());
}

#[tokio::test]
async fn can_add_then_suspend_a_service() {
    let manager = init(vec![
        DiscoveryEvent::add("0", true, 80, "http", ([10, 0, 0, 1], 80), "http"),
        DiscoveryEvent::suspend("0"),
    ])
    .await;

    assert_eq!(
        *manager.port_service_map.lock().await,
        hashmap! {
            80 => Service::new(
                80,
                "http",
                hashmap! {
                    EndpointId::new(Arc::new(TEST_SERVICE_DISCOVERY_NAME.to_owned()), "0") =>
                        Endpoint::new(false, ([10, 0, 0, 1], 80), "http"),
                },
            ),
        }
    );
}

#[tokio::test]
async fn can_add_then_resume_an_unavailable_service() {
    let manager = init(vec![
        DiscoveryEvent::add(
            "0",
            false,
            80,
            "http",
            ([10, 0, 0, 1], 80),
            "http",
        ),
        DiscoveryEvent::resume("0"),
    ])
    .await;

    assert_eq!(
        *manager.port_service_map.lock().await,
        hashmap! {
            80 => Service::new(
                80,
                "http",
                hashmap! {
                    EndpointId::new(Arc::new(TEST_SERVICE_DISCOVERY_NAME.to_owned()), "0") =>
                        Endpoint::new(true, ([10, 0, 0, 1], 80), "http"),
                },
            ),
        }
    );
}
