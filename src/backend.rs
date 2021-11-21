use std::{collections::HashMap, net::SocketAddr, sync::Arc};

#[derive(Default)]
pub struct Backend {
    services: HashMap<u16, Service>,
}

impl Backend {
    pub fn new() -> Self {
        Self::default()
    }
}

pub struct Service {
    protocol_name: String,
    endpoints: HashMap<Arc<EndpointId>, Endpoint>,
}

#[derive(PartialEq, Eq, Hash)]
pub struct EndpointId {
    pub discovery_service: &'static str,
    pub id: String,
}

impl EndpointId {
    pub fn new<I>(discovery_service: &'static str, id: I) -> Self
    where
        I: ToString,
    {
        Self {
            discovery_service,
            id: id.to_string(),
        }
    }
}

pub struct Endpoint {
    protocol_name: String,
    address: SocketAddr,
}
