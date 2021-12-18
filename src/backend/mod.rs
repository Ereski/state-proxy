use crate::matchmaking::Matchmaker;
use anyhow::{anyhow, Result};
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
// Events are pretty small so we can se this high to deal with spikes
const DISCOVERY_EVENT_BUFFER_SIZE: usize = 1024;

pub struct ServiceManager {
    matchmaker: Matchmaker,

    registered_discovery_services: HashSet<Arc<String>>,
    services: Arc<Mutex<HashMap<u16, Service>>>,
}

impl ServiceManager {
    pub fn new(matchmaker: Matchmaker) -> Self {
        Self {
            matchmaker,

            registered_discovery_services: HashSet::new(),
            services: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn register_discovery_service<D>(
        &mut self,
        discovery_service: D,
    ) -> Result<()>
    where
        D: DiscoveryService + Send + Sync + 'static,
    {
        let name = discovery_service.name();

        if self.registered_discovery_services.contains(&name) {
            Err(anyhow!(
                "the discovery service {} is already registered",
                name
            ))
        } else {
            let (send, recv) = mpsc::channel(DISCOVERY_EVENT_BUFFER_SIZE);
            discovery_service.run_with_sender(send);
            self.listen_on(name.clone(), recv);
            info!("New discovery service registered: {}", name);
            self.registered_discovery_services.insert(name);

            Ok(())
        }
    }

    fn listen_on(&self, name: Arc<String>, mut recv: Receiver<DiscoveryEvent>) {
        let services = self.services.clone();
        tokio::spawn(async move {
            while let Some(event) = recv.recv().await {
                unimplemented!();
            }
        });
    }

    pub async fn run(self) -> Result<()> {
        unimplemented!()
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
    pub endpoints: Vec<(Arc<EndpointRef>, Endpoint)>,
}

impl Service {
    pub fn new<P>(
        protocol: P,
        port: u16,
        endpoints: Vec<(Arc<EndpointRef>, Endpoint)>,
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
    pub protocol: String,
    pub address: SocketAddr,
}

impl Endpoint {
    pub fn new<P, A>(protocol: P, address: A) -> Self
    where
        P: ToString,
        A: Into<SocketAddr>,
    {
        Self {
            protocol: protocol.to_string(),
            address: address.into(),
        }
    }
}
