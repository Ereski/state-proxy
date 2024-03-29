use super::{
    Endpoint, EndpointDiscovery, EndpointDiscoveryMetrics, EndpointEvent,
    EndpointId, PortEvent, Service, ServiceManager,
};
use crate::test_utils::panic_on_timeout;
use maplit::{hashmap, hashset};
use pretty_assertions::assert_eq;
use std::sync::Arc;
use tokio::{
    sync::{
        mpsc::{self, Sender},
        oneshot::{self, Receiver as OneshotReceiver, Sender as OneshotSender},
    },
    task,
};

// How many iterations to run a spinlock for
const SPINLOCK_TURNS: usize = 100;

static TEST_ENDPOINT_DISCOVERY_NAME: &str = "test";

struct TestEndpointDiscovery {
    events: Vec<EndpointEvent>,
    metrics_receiver: OneshotReceiver<Arc<EndpointDiscoveryMetrics>>,
    status_sender: OneshotSender<bool>,
}

impl TestEndpointDiscovery {
    fn new(
        events: Vec<EndpointEvent>,
    ) -> (
        Self,
        OneshotSender<Arc<EndpointDiscoveryMetrics>>,
        OneshotReceiver<bool>,
    ) {
        let (metrics_sender, metrics_receiver) = oneshot::channel();
        let (status_sender, status_receiver) = oneshot::channel();

        (
            Self {
                events,
                metrics_receiver,
                status_sender,
            },
            metrics_sender,
            status_receiver,
        )
    }
}

impl EndpointDiscovery for TestEndpointDiscovery {
    fn name(&self) -> Arc<String> {
        Arc::new(TEST_ENDPOINT_DISCOVERY_NAME.to_owned())
    }

    fn run_with_sender(self, sender: Sender<EndpointEvent>) {
        tokio::spawn(async move {
            let n_events = u64::try_from(self.events.len()).unwrap();
            for event in self.events {
                if sender.send(event).await.is_err() {
                    let _ = self.status_sender.send(false);

                    return;
                }
            }

            // Spinlock until the metrics show everything was processed
            let mut turns = SPINLOCK_TURNS;
            let metrics = self.metrics_receiver.await.ok().unwrap();
            while metrics.events_processed.get() != n_events {
                if turns == 0 {
                    let _ = self.status_sender.send(false);

                    return;
                }
                turns -= 1;

                task::yield_now().await;
            }

            let _ = self.status_sender.send(true);
        });
    }
}

async fn init<D>(
    events: D,
    port_event_sender: Option<Sender<PortEvent>>,
) -> Arc<ServiceManager>
where
    D: IntoIterator<Item = EndpointEvent>,
{
    let manager = ServiceManager::new();

    if let Some(port_event_sender) = port_event_sender {
        manager.send_port_events_to(port_event_sender)
    }

    let (endpoint_discovery, metrics_sender, status_receiver) =
        TestEndpointDiscovery::new(Vec::from_iter(events));

    manager
        .register_endpoint_discovery(endpoint_discovery)
        .await
        .unwrap();

    metrics_sender
        .send(
            manager
                .endpoint_discovery_metrics
                .lock()
                .await
                .get(&TEST_ENDPOINT_DISCOVERY_NAME.to_owned())
                .unwrap()
                .clone(),
        )
        .ok()
        .unwrap();
    assert_eq!(status_receiver.await, Ok(true));

    manager
}

#[tokio::test]
async fn can_register_a_endpoint_discovery() {
    let manager = init(Vec::new(), None).await;

    assert_eq!(
        *manager.registered_endpoint_discovery_names.lock().await,
        hashset! { Arc::new(TEST_ENDPOINT_DISCOVERY_NAME.to_owned()) }
    );
}

#[tokio::test]
async fn can_add_a_service() {
    let manager = init(
        [EndpointEvent::add(
            "0",
            true,
            80,
            "http",
            ([10, 0, 0, 1], 80),
            "http",
        )],
        None,
    )
    .await;

    assert_eq!(
        *manager.port_service_map.read().await,
        hashmap! {
            80 => Service::new(
                80,
                "http",
                hashmap! {
                    EndpointId::new(Arc::new(TEST_ENDPOINT_DISCOVERY_NAME.to_owned()), "0") =>
                        Endpoint::new(true, ([10, 0, 0, 1], 80), "http"),
                },
            ),
        }
    );
}

#[tokio::test]
async fn can_add_then_delete_a_service() {
    let manager = init(
        [
            EndpointEvent::add(
                "0",
                true,
                80,
                "http",
                ([10, 0, 0, 1], 80),
                "http",
            ),
            EndpointEvent::delete("0"),
        ],
        None,
    )
    .await;

    assert!(manager.port_service_map.read().await.is_empty());
}

#[tokio::test]
async fn can_add_then_suspend_a_service() {
    let manager = init(
        [
            EndpointEvent::add(
                "0",
                true,
                80,
                "http",
                ([10, 0, 0, 1], 80),
                "http",
            ),
            EndpointEvent::suspend("0"),
        ],
        None,
    )
    .await;

    assert_eq!(
        *manager.port_service_map.read().await,
        hashmap! {
            80 => Service::new(
                80,
                "http",
                hashmap! {
                    EndpointId::new(Arc::new(TEST_ENDPOINT_DISCOVERY_NAME.to_owned()), "0") =>
                        Endpoint::new(false, ([10, 0, 0, 1], 80), "http"),
                },
            ),
        }
    );
}

#[tokio::test]
async fn can_add_then_resume_an_unavailable_service() {
    let manager = init(
        [
            EndpointEvent::add(
                "0",
                false,
                80,
                "http",
                ([10, 0, 0, 1], 80),
                "http",
            ),
            EndpointEvent::resume("0"),
        ],
        None,
    )
    .await;

    assert_eq!(
        *manager.port_service_map.read().await,
        hashmap! {
            80 => Service::new(
                80,
                "http",
                hashmap! {
                    EndpointId::new(Arc::new(TEST_ENDPOINT_DISCOVERY_NAME.to_owned()), "0") =>
                        Endpoint::new(true, ([10, 0, 0, 1], 80), "http"),
                },
            ),
        }
    );
}

#[tokio::test]
async fn sends_open_and_close_port_events() {
    let (event_sender, mut event_receiver) = mpsc::channel(1);
    let _manager = init(
        [
            EndpointEvent::add(
                "0",
                false,
                80,
                "http",
                ([10, 0, 0, 1], 80),
                "http",
            ),
            EndpointEvent::delete("0"),
        ],
        Some(event_sender),
    )
    .await;

    assert_eq!(
        panic_on_timeout(event_receiver.recv()).await,
        Some(PortEvent::Open {
            port: 80,
            protocol: "http".to_owned(),
        })
    );
    assert_eq!(
        panic_on_timeout(event_receiver.recv()).await,
        Some(PortEvent::Close { port: 80 })
    );
}

#[tokio::test]
async fn sends_missed_open_events_on_registering_a_sender() {
    let (event_sender, mut event_receiver) = mpsc::channel(1);
    let manager = init(
        [EndpointEvent::add(
            "0",
            false,
            80,
            "http",
            ([10, 0, 0, 1], 80),
            "http",
        )],
        None,
    )
    .await;
    manager.send_port_events_to(event_sender);

    assert_eq!(
        panic_on_timeout(event_receiver.recv()).await,
        Some(PortEvent::Open {
            port: 80,
            protocol: "http".to_owned(),
        })
    );
}
