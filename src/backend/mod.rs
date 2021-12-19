use crate::backend::discovery::{DiscoveryEvent, ServiceDiscovery};
use anyhow::{anyhow, Result};
use maplit::hashmap;
use metered::{measure, HitCount, Throughput};
use std::{
    collections::{hash_map::Entry, HashMap, HashSet},
    fmt::{self, Display, Formatter},
    net::SocketAddr,
    sync::Arc,
};
use tokio::sync::{
    mpsc::{self, Receiver},
    Mutex,
};
use tracing::{info, warn};

pub mod discovery;

#[cfg(test)]
mod test;

// How many `DiscoveryEvent`s can be buffered before a `ServiceDiscovery` will have to wait.
// Events are pretty small so we can set this high to deal with spikes
const DISCOVERY_EVENT_BUFFER_SIZE: usize = 1024;

// Use locktree here to ensure deadlock-freedom
pub struct ServiceManager {
    registered_service_discovery_names: Mutex<HashSet<Arc<String>>>,
    port_service_map: Mutex<HashMap<u16, Service>>,

    service_discovery_metrics:
        Mutex<HashMap<Arc<String>, Arc<ServiceDiscoveryMetrics>>>,
}

impl ServiceManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            registered_service_discovery_names: Mutex::new(HashSet::new()),
            port_service_map: Mutex::new(HashMap::new()),

            service_discovery_metrics: Mutex::new(HashMap::new()),
        })
    }

    pub async fn register_service_discovery<D>(
        self: &Arc<Self>,
        service_discovery: D,
    ) -> Result<()>
    where
        D: ServiceDiscovery + Send + Sync + 'static,
    {
        let name = service_discovery.name();
        let mut registered_service_discovery_names =
            self.registered_service_discovery_names.lock().await;
        let mut service_discovery_metrics =
            self.service_discovery_metrics.lock().await;

        if registered_service_discovery_names.contains(&name) {
            Err(anyhow!(
                "the discovery service {} is already registered",
                name
            ))
        } else {
            let metrics = Arc::new(ServiceDiscoveryMetrics::new());
            service_discovery_metrics.insert(name.clone(), metrics.clone());

            let (send, recv) = mpsc::channel(DISCOVERY_EVENT_BUFFER_SIZE);
            service_discovery.run_with_sender(send);
            tokio::spawn(self.clone().listen_for_discovery_events(
                name.clone(),
                metrics,
                recv,
            ));
            registered_service_discovery_names.insert(name.clone());

            info!("New discovery service registered: {}", name);

            Ok(())
        }
    }

    async fn listen_for_discovery_events(
        self: Arc<Self>,
        from: Arc<String>,
        metrics: Arc<ServiceDiscoveryMetrics>,
        mut recv: Receiver<DiscoveryEvent>,
    ) {
        while let Some(event) = recv.recv().await {
            measure!(
                &metrics.events_processed,
                measure!(
                    &metrics.event_throughput,
                    self.process_event(from.clone(), event).await
                )
            );
        }

        panic!(
            "TODO: discovery service {} disconnected. Need to think what to do in this case",
            from
        );
    }

    async fn process_event(&self, from: Arc<String>, event: DiscoveryEvent) {
        let mut port_service_map = self.port_service_map.lock().await;
        match event {
            DiscoveryEvent::Add {
                uid,
                is_available,
                external_port,
                external_protocol,
                backend_address,
                backend_protocol,
            } => {
                let endpoint_id = EndpointId::new(from, uid);
                let endpoint = Endpoint::new(
                    is_available,
                    backend_address,
                    backend_protocol,
                );
                match port_service_map.entry(external_port) {
                    Entry::Vacant(port_service_map_entry) => {
                        info!(
                            "New service requested on port {} ({}) from {}. Initially available endpoint: {:#?}",
                            external_port, external_protocol, endpoint_id.service_discovery_name,
                            endpoint
                        );
                        port_service_map_entry.insert(Service::new(
                            external_port,
                            external_protocol,
                            hashmap! {
                                endpoint_id => endpoint,
                            },
                        ));
                    }
                    Entry::Occupied(mut port_service_map_entry) => {
                        match port_service_map_entry
                            .get_mut()
                            .endpoints
                            .entry(endpoint_id)
                        {
                            Entry::Vacant(endpoint_entry) => {
                                info!(
                                    "New endpoint '{}' added from '{}' for service on port {} ({}): {:#?}", endpoint_entry.key().uid,
                                    endpoint_entry.key().service_discovery_name, external_port,
                                    external_protocol, endpoint
                                );
                                endpoint_entry.insert(endpoint);
                            }
                            Entry::Occupied(mut endpoint_entry) => {
                                info!(
                                    "Endpoint '{}' from '{}' updated as: {:#?}",
                                    endpoint_entry.key().uid,
                                    endpoint_entry.key().service_discovery_name,
                                    endpoint
                                );
                                *endpoint_entry.get_mut() = endpoint;
                            }
                        }
                    }
                }
            }
            DiscoveryEvent::Remove { uid } => {
                let endpoint_id = EndpointId::new(from, uid);
                let mut found = false;
                let mut services_to_remove = Vec::new();
                for (port, service) in &mut *port_service_map {
                    if service.endpoints.remove(&endpoint_id).is_some() {
                        found = true;
                        info!(
                            "Removed endpoint '{}' ({}) for service on port {} ({})",
                            endpoint_id.uid, endpoint_id.service_discovery_name, service.port,
                            service.protocol
                        );
                    }

                    if service.endpoints.is_empty() {
                        services_to_remove.push(*port);
                    }
                }

                for port in services_to_remove {
                    let old_service = port_service_map.remove(&port).unwrap();
                    info!(
                        "Stopped serving on port {} ({}): no endpoint",
                        port, old_service.protocol
                    );
                }

                if !found {
                    warn!(
                        "Received a DiscoveryEvent::Remove from '{}' for an unknown endpoint: {}",
                        endpoint_id.service_discovery_name, endpoint_id.uid
                    );
                }
            }
            DiscoveryEvent::Suspend { uid } => {
                let endpoint_id = EndpointId::new(from, uid);
                let mut found = false;
                for service in port_service_map.values_mut() {
                    if let Some(endpoint) =
                        service.endpoints.get_mut(&endpoint_id)
                    {
                        found = true;
                        if endpoint.is_available {
                            endpoint.is_available = false;
                            info!(
                                "Suspended endpoint '{}' ({}) for service on port {} ({})",
                                endpoint_id.uid, endpoint_id.service_discovery_name, service.port,
                                service.protocol
                            );
                        }
                    }
                }

                if !found {
                    warn!(
                        "Received a DiscoveryEvent::Suspend from '{}' for an unknown endpoint: {}",
                        endpoint_id.service_discovery_name, endpoint_id.uid
                    );
                }
            }
            DiscoveryEvent::Resume { uid } => {
                let endpoint_id = EndpointId::new(from, uid);
                let mut found = false;
                for service in port_service_map.values_mut() {
                    if let Some(endpoint) =
                        service.endpoints.get_mut(&endpoint_id)
                    {
                        found = true;
                        if !endpoint.is_available {
                            endpoint.is_available = true;
                            info!(
                                "Resumed endpoint '{}' ({}) for service on port {} ({})",
                                endpoint_id.uid, endpoint_id.service_discovery_name, service.port,
                                service.protocol
                            );
                        }
                    }
                }

                if !found {
                    warn!(
                        "Received a DiscoveryEvent::Resume from '{}' for an unknown endpoint: {}",
                        endpoint_id.service_discovery_name, endpoint_id.uid
                    );
                }
            }
        }
    }
}

#[derive(Debug, Default)]
struct ServiceDiscoveryMetrics {
    event_throughput: Throughput,
    events_processed: HitCount,
}

impl ServiceDiscoveryMetrics {
    fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Service {
    port: u16,
    protocol: String,
    endpoints: HashMap<EndpointId, Endpoint>,
}

impl Service {
    fn new<P>(
        port: u16,
        protocol: P,
        endpoints: HashMap<EndpointId, Endpoint>,
    ) -> Self
    where
        P: Into<String>,
    {
        Self {
            protocol: protocol.into(),
            endpoints,
            port,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Hash)]
struct EndpointId {
    service_discovery_name: Arc<String>,
    uid: String,
}

impl EndpointId {
    fn new<I>(service_discovery_name: Arc<String>, uid: I) -> Self
    where
        I: Into<String>,
    {
        Self {
            service_discovery_name,
            uid: uid.into(),
        }
    }
}

impl Display for EndpointId {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "({}, {})", self.service_discovery_name, self.uid)
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Endpoint {
    is_available: bool,
    address: SocketAddr,
    protocol: String,
}

impl Endpoint {
    fn new<A, P>(is_available: bool, address: A, protocol: P) -> Self
    where
        A: Into<SocketAddr>,
        P: Into<String>,
    {
        Self {
            is_available,
            address: address.into(),
            protocol: protocol.into(),
        }
    }
}
