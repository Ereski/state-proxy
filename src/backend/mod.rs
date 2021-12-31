//! Management of proxied services.
//!
//! This module two main items:
//!
//! - The [`ServiceManager`] struct, which manages service endpoint tasks and organizes and
//!   collates that information into services that we should proxy.
//! - The [`EndpointDiscovery`] trait, which are implemented by tasks that add, remove, and update
//!   the list of services that have asked to be proxied by us.

use crate::backend::endpoint::{
    Endpoint, EndpointDiscovery, EndpointEvent, EndpointId,
};
use anyhow::{anyhow, Result};
use maplit::hashmap;
use metered::{measure, HitCount, Throughput};
use std::{
    collections::{hash_map::Entry, HashMap, HashSet},
    net::SocketAddr,
    sync::{Arc, Weak},
};
use tokio::sync::{
    mpsc::{self, Receiver, Sender},
    Mutex, RwLock,
};
use tracing::{info, warn};

pub mod endpoint;

#[cfg(feature = "benchmark")]
#[doc(hidden)]
pub mod benchmark;

#[cfg(test)]
mod test;

// How many `EndpointEvent`s can be buffered before that `EndpointDiscovery` will have to wait.
// Events are pretty small so we can set this high to deal with spikes
const ENDPOINT_EVENT_BUFFER_SIZE: usize = 1024;

/// Manages service discovery tasks and information about proxied services. [`EndpointDiscovery`]
/// tasks are registered through the [`ServiceManager::register_endpoint_discovery`] method.
///
/// [`ServiceManager::send_port_events_to`] registers a listener for [`PortEvent`]s as the
/// ports and protocols the proxy exposes will depend on which are requested by proxied services.
/// [`ServiceManager::get_endpoint_for`] will, in turn, select an available proxied endpoint that
/// can process an external request.
// TODO: Use locktree here to ensure deadlock-freedom
pub struct ServiceManager {
    registered_endpoint_discovery_names: Mutex<HashSet<Arc<String>>>,
    port_service_map: RwLock<HashMap<u16, Service>>,

    port_event_sender: RwLock<Option<Sender<PortEvent>>>,

    endpoint_discovery_metrics:
        Mutex<HashMap<Arc<String>, Arc<EndpointDiscoveryMetrics>>>,
}

impl ServiceManager {
    /// Create an empty [`ServiceManager`]. This function returns an `Arc<ServiceManager>` as most
    /// methods have that as receiver.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            registered_endpoint_discovery_names: Mutex::new(HashSet::new()),
            port_service_map: RwLock::new(HashMap::new()),

            port_event_sender: RwLock::new(None),

            endpoint_discovery_metrics: Mutex::new(HashMap::new()),
        })
    }

    /// Set a channel to send [`PortEvent`]s to. When this method is called, all current open
    /// ports events sent first.
    // TODO: probably should make this async instead of spawning a task. Handle deadlocks in the
    // caller
    pub fn send_port_events_to(
        self: &Arc<Self>,
        port_event_sender: Sender<PortEvent>,
    ) {
        let this = self.clone();
        tokio::spawn(async move {
            let port_service_map = this.port_service_map.read().await;
            for service in port_service_map.values() {
                let res = port_event_sender
                    .send(PortEvent::Open {
                        port: service.port,
                        protocol: service.protocol.clone(),
                    })
                    .await;
                if res.is_err() {
                    return;
                }
            }
            *this.port_event_sender.write().await = Some(port_event_sender);
        });
    }

    /// Select an endpoint that can serve a connection from the given external port. The selection
    /// is semi-random and based on the `index` parameter.
    ///
    /// If there is an endpoint available, return its address and which protocol should be used to
    /// communicate with it.
    pub async fn get_endpoint_for(
        &self,
        port: u16,
        mut index: usize,
    ) -> Option<(SocketAddr, String)> {
        let port_service_map = self.port_service_map.read().await;
        if let Some(service) = port_service_map.get(&port) {
            let n_endpoints = service.endpoints.len();
            index %= n_endpoints;
            for candidate in service.endpoints.values().skip(index) {
                if candidate.is_available {
                    return Some((
                        candidate.address,
                        candidate.protocol.clone(),
                    ));
                }
            }
            for candidate in
                service.endpoints.values().take(n_endpoints - index)
            {
                if candidate.is_available {
                    return Some((
                        candidate.address,
                        candidate.protocol.clone(),
                    ));
                }
            }
        }

        None
    }

    /// Register an [`EndpointDiscovery`] task.
    pub async fn register_endpoint_discovery<D>(
        self: &Arc<Self>,
        endpoint_discovery: D,
    ) -> Result<()>
    where
        D: EndpointDiscovery + Send + Sync + 'static,
    {
        let name = endpoint_discovery.name();
        let mut registered_endpoint_discovery_names =
            self.registered_endpoint_discovery_names.lock().await;
        let mut endpoint_discovery_metrics =
            self.endpoint_discovery_metrics.lock().await;

        if registered_endpoint_discovery_names.contains(&name) {
            Err(anyhow!(
                "the discovery service {} is already registered",
                name
            ))
        } else {
            let metrics = Arc::new(EndpointDiscoveryMetrics::new());
            endpoint_discovery_metrics.insert(name.clone(), metrics.clone());

            let (sender, receiver) = mpsc::channel(ENDPOINT_EVENT_BUFFER_SIZE);
            endpoint_discovery.run_with_sender(sender);
            tokio::spawn(Self::listen_for_discovery_events(
                Arc::downgrade(self),
                name.clone(),
                metrics,
                receiver,
            ));
            registered_endpoint_discovery_names.insert(name.clone());

            info!("New discovery service registered: {}", name);

            Ok(())
        }
    }

    async fn listen_for_discovery_events(
        this: Weak<Self>,
        from: Arc<String>,
        metrics: Arc<EndpointDiscoveryMetrics>,
        mut receiver: Receiver<EndpointEvent>,
    ) {
        while let Some(event) = receiver.recv().await {
            let this = if let Some(this) = this.upgrade() {
                this
            } else {
                return;
            };
            measure!(
                &metrics.events_processed,
                measure!(
                    &metrics.event_throughput,
                    this.process_event(from.clone(), event).await
                )
            );
        }

        panic!(
            "TODO: discovery service {} disconnected. Need to think what to do in this case",
            from
        );
    }

    async fn process_event(&self, from: Arc<String>, event: EndpointEvent) {
        let mut port_service_map = self.port_service_map.write().await;
        match event {
            EndpointEvent::Add {
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
                            external_port, external_protocol, endpoint_id.endpoint_discovery_name,
                            endpoint
                        );
                        port_service_map_entry.insert(Service::new(
                            external_port,
                            external_protocol.clone(),
                            hashmap! {
                                endpoint_id => endpoint,
                            },
                        ));
                        Self::send_event(
                            &*self.port_event_sender.read().await,
                            PortEvent::Open {
                                port: external_port,
                                protocol: external_protocol,
                            },
                        )
                        .await;
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
                                    endpoint_entry.key().endpoint_discovery_name, external_port,
                                    external_protocol, endpoint
                                );
                                endpoint_entry.insert(endpoint);
                            }
                            Entry::Occupied(mut endpoint_entry) => {
                                info!(
                                    "Endpoint '{}' from '{}' updated as: {:#?}",
                                    endpoint_entry.key().uid,
                                    endpoint_entry
                                        .key()
                                        .endpoint_discovery_name,
                                    endpoint
                                );
                                *endpoint_entry.get_mut() = endpoint;
                            }
                        }
                    }
                }
            }
            EndpointEvent::Delete { uid } => {
                let endpoint_id = EndpointId::new(from, uid);
                let mut found = false;
                let mut services_to_delete = Vec::new();
                for (port, service) in &mut *port_service_map {
                    if service.endpoints.remove(&endpoint_id).is_some() {
                        found = true;
                        info!(
                            "Deleted endpoint '{}' ({}) for service on port {} ({})",
                            endpoint_id.uid, endpoint_id.endpoint_discovery_name, service.port,
                            service.protocol
                        );
                    }

                    if service.endpoints.is_empty() {
                        services_to_delete.push(*port);
                    }
                }

                let port_event_sender = self.port_event_sender.read().await;
                for port in services_to_delete {
                    let old_service = port_service_map.remove(&port).unwrap();
                    Self::send_event(
                        &port_event_sender,
                        PortEvent::Close { port },
                    )
                    .await;
                    info!(
                        "Stopped serving on port {} ({}): no endpoint",
                        port, old_service.protocol
                    );
                }

                if !found {
                    warn!(
                        "Received a EndpointEvent::Delete from '{}' for an unknown endpoint: {}",
                        endpoint_id.endpoint_discovery_name, endpoint_id.uid
                    );
                }
            }
            EndpointEvent::Suspend { uid } => {
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
                                endpoint_id.uid, endpoint_id.endpoint_discovery_name, service.port,
                                service.protocol
                            );
                        }
                    }
                }

                if !found {
                    warn!(
                        "Received a EndpointEvent::Suspend from '{}' for an unknown endpoint: {}",
                        endpoint_id.endpoint_discovery_name, endpoint_id.uid
                    );
                }
            }
            EndpointEvent::Resume { uid } => {
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
                                endpoint_id.uid, endpoint_id.endpoint_discovery_name, service.port,
                                service.protocol
                            );
                        }
                    }
                }

                if !found {
                    warn!(
                        "Received a EndpointEvent::Resume from '{}' for an unknown endpoint: {}",
                        endpoint_id.endpoint_discovery_name, endpoint_id.uid
                    );
                }
            }
        }
    }

    async fn send_event(sender: &Option<Sender<PortEvent>>, event: PortEvent) {
        if let Some(sender) = sender {
            if sender.send(event).await.is_err() {
                panic!("TODO: can't send port event");
            }
        }
    }
}

/// A notification that the proxy listen on or close a specified port.
#[derive(Debug, PartialEq, Eq)]
pub enum PortEvent {
    /// Listen on the given port and with a server of the given protocol.
    Open { port: u16, protocol: String },

    /// Close a port.
    Close { port: u16 },
}

#[derive(Debug, Default)]
struct EndpointDiscoveryMetrics {
    event_throughput: Throughput,
    events_processed: HitCount,
}

impl EndpointDiscoveryMetrics {
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
