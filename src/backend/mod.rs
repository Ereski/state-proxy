use anyhow::{anyhow, Result};
use metered::{measure, HitCount, Throughput};
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::Arc,
};
use tokio::sync::{
    mpsc::{self, Receiver, Sender},
    Mutex,
};
use tracing::info;

#[cfg(test)]
mod test;

// How many `DiscoveryEvent`s can be buffered before a `DiscoveryService` will have to wait.
// Events are pretty small so we can set this high to deal with spikes
const DISCOVERY_EVENT_BUFFER_SIZE: usize = 1024;

// Use locktree here to ensure deadlock-freedom
pub struct ServiceManager {
    registered_discovery_services: Mutex<HashSet<Arc<String>>>,
    services: Mutex<HashMap<u16, Service>>,

    discovery_service_metrics:
        Mutex<HashMap<Arc<String>, Arc<DiscoveryServiceMetrics>>>,
}

impl ServiceManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            registered_discovery_services: Mutex::new(HashSet::new()),
            services: Mutex::new(HashMap::new()),

            discovery_service_metrics: Mutex::new(HashMap::new()),
        })
    }

    pub async fn register_discovery_service<D>(
        self: &Arc<Self>,
        discovery_service: D,
    ) -> Result<()>
    where
        D: DiscoveryService + Send + Sync + 'static,
    {
        let name = discovery_service.name();
        let mut registered_discovery_services =
            self.registered_discovery_services.lock().await;
        let mut discovery_service_metrics =
            self.discovery_service_metrics.lock().await;

        if registered_discovery_services.contains(&name) {
            Err(anyhow!(
                "the discovery service {} is already registered",
                name
            ))
        } else {
            let metrics = Arc::new(DiscoveryServiceMetrics::new());
            discovery_service_metrics.insert(name.clone(), metrics.clone());

            let (send, recv) = mpsc::channel(DISCOVERY_EVENT_BUFFER_SIZE);
            discovery_service.run_with_sender(send);
            tokio::spawn(self.clone().listen_for_discovery_events(
                name.clone(),
                metrics,
                recv,
            ));
            registered_discovery_services.insert(name.clone());

            info!("New discovery service registered: {}", name);

            Ok(())
        }
    }

    async fn listen_for_discovery_events(
        self: Arc<Self>,
        name: Arc<String>,
        metrics: Arc<DiscoveryServiceMetrics>,
        mut recv: Receiver<DiscoveryEvent>,
    ) {
        while let Some(event) = recv.recv().await {
            measure!(
                &metrics.events_processed,
                measure!(
                    &metrics.event_throughput,
                    self.process_event(&name, event).await
                )
            );
        }
    }

    async fn process_event(&self, name: &str, event: DiscoveryEvent) {
        unimplemented!()
    }
}

#[derive(Debug, Default)]
struct DiscoveryServiceMetrics {
    event_throughput: Throughput,
    events_processed: HitCount,
}

impl DiscoveryServiceMetrics {
    fn new() -> Self {
        Self::default()
    }
}

pub trait DiscoveryService {
    fn name(&self) -> Arc<String>;

    fn run_with_sender(self, send: Sender<DiscoveryEvent>);
}

pub enum DiscoveryEvent {
    Add {
        id: Arc<EndpointRef>,
        services: Vec<Service>,
    },
    Remove {
        id: Arc<EndpointRef>,
    },
    Suspend {
        id: Arc<EndpointRef>,
    },
    Resume {
        id: Arc<EndpointRef>,
    },
}

impl DiscoveryEvent {
    pub fn add(id: Arc<EndpointRef>, services: Vec<Service>) -> Self {
        Self::Add { id, services }
    }

    pub fn remove(id: Arc<EndpointRef>) -> Self {
        Self::Remove { id }
    }

    pub fn suspend(id: Arc<EndpointRef>) -> Self {
        Self::Suspend { id }
    }

    pub fn resume(id: Arc<EndpointRef>) -> Self {
        Self::Resume { id }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Service {
    pub protocol: String,
    pub port: u16,
    pub endpoints: HashMap<Arc<EndpointRef>, Endpoint>,
}

impl Service {
    pub fn new<P>(
        protocol: P,
        port: u16,
        endpoints: HashMap<Arc<EndpointRef>, Endpoint>,
    ) -> Self
    where
        P: ToString,
    {
        Self {
            protocol: protocol.to_string(),
            port,
            endpoints,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EndpointRef {
    pub discovery_service: Arc<String>,
    pub uid: String,
}

impl EndpointRef {
    pub fn new<I>(discovery_service: Arc<String>, uid: I) -> Self
    where
        I: ToString,
    {
        Self {
            discovery_service,
            uid: uid.to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub is_available: bool,
    pub protocol: String,
    pub address: SocketAddr,
}

impl Endpoint {
    pub fn new<P, A>(is_available: bool, protocol: P, address: A) -> Self
    where
        P: ToString,
        A: Into<SocketAddr>,
    {
        Self {
            is_available,
            protocol: protocol.to_string(),
            address: address.into(),
        }
    }
}
