use std::{
    fmt::{self, Display, Formatter},
    net::SocketAddr,
    sync::Arc,
};
use tokio::sync::mpsc::Sender;

/// Trait for tasks that add, remove, and update the list of endpoints that are asking to be
/// proxied.
pub trait EndpointDiscovery {
    /// Get name of this task. This name must be unique enough to avoid collisions with other
    /// tasks. It should also be descriptive enough to appear in error messages.
    fn name(&self) -> Arc<String>;

    /// Run the task. This method should spawn an asynchronous task with [`tokio::spawn`] or
    /// [`tokio::task::spawn_blocking`], and [`EndpointEvent`]s should be sent through the given
    /// channel.
    fn run_with_sender(self, sender: Sender<EndpointEvent>);
}

/// An event sent from a service discovery task.
#[derive(Debug, PartialEq, Eq)]
pub enum EndpointEvent {
    /// An endpoint has been added.
    Add {
        /// An unique ID representing this endpoint for the sending service discovery task. This
        /// field is freeform.
        uid: String,

        /// Whether this endpoint is already available to handle proxied connections.
        is_available: bool,

        /// The port that external clients connect to to reach this endpoint.
        external_port: u16,

        /// The protocol that external clients connect through to reach this endpoint.
        external_protocol: String,

        /// This endpoint's address.
        backend_address: SocketAddr,

        /// The protocol that the proxy should use to communicate with this endpoint.
        backend_protocol: String,
    },

    /// An endpoint has been deleted.
    Delete {
        /// The unique ID of the endpoint.
        uid: String,
    },

    /// An endpoint still exists but is not available anymore.
    Suspend {
        /// The unique ID of the endpoint.
        uid: String,
    },

    /// A previously unavailable endpoint is now available.
    Resume {
        /// The unique ID of the endpoint.
        uid: String,
    },
}

impl EndpointEvent {
    /// Create a [`EndpointEvent::Add`] event.
    pub fn add<I, EP, BA, BP>(
        uid: I,
        is_available: bool,
        external_port: u16,
        external_protocol: EP,
        backend_address: BA,
        backend_protocol: BP,
    ) -> Self
    where
        I: Into<String>,
        EP: Into<String>,
        BA: Into<SocketAddr>,
        BP: Into<String>,
    {
        Self::Add {
            uid: uid.into(),
            is_available,
            external_port,
            external_protocol: external_protocol.into(),
            backend_address: backend_address.into(),
            backend_protocol: backend_protocol.into(),
        }
    }

    /// Create a [`EndpointEvent::Delete`] event.
    pub fn delete<U>(uid: U) -> Self
    where
        U: Into<String>,
    {
        Self::Delete { uid: uid.into() }
    }

    /// Create a [`EndpointEvent::Suspend`] event.
    pub fn suspend<U>(uid: U) -> Self
    where
        U: Into<String>,
    {
        Self::Suspend { uid: uid.into() }
    }

    /// Create a [`EndpointEvent::Resume`] event.
    pub fn resume<U>(uid: U) -> Self
    where
        U: Into<String>,
    {
        Self::Resume { uid: uid.into() }
    }
}

#[derive(Debug, PartialEq, Eq, Hash)]
pub(crate) struct EndpointId {
    pub(crate) endpoint_discovery_name: Arc<String>,
    pub(crate) uid: String,
}

impl EndpointId {
    pub(crate) fn new<I>(endpoint_discovery_name: Arc<String>, uid: I) -> Self
    where
        I: Into<String>,
    {
        Self {
            endpoint_discovery_name,
            uid: uid.into(),
        }
    }
}

impl Display for EndpointId {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "({}, {})", self.endpoint_discovery_name, self.uid)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Endpoint {
    pub(crate) is_available: bool,
    pub(crate) address: SocketAddr,
    pub(crate) protocol: String,
}

impl Endpoint {
    pub(crate) fn new<A, P>(is_available: bool, address: A, protocol: P) -> Self
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
