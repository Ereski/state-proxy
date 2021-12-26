use std::{net::SocketAddr, sync::Arc};
use tokio::sync::mpsc::Sender;

// TODO: actually this should be named `EndpointDiscovery`

/// Trait for tasks that add, remove, and update the list of services that are asking to be
/// proxied.
pub trait ServiceDiscovery {
    /// Get name of this task. This name must be unique enough to avoid collisions with other
    /// tasks. It should also be descriptive enough to appear in error messages.
    fn name(&self) -> Arc<String>;

    /// Run the task. This method should spawn an asynchronous task with [`tokio::spawn`] or
    /// [`tokio::task::spawn_blocking`], and [`DiscoveryEvent`]s should be sent through the given
    /// channel.
    fn run_with_sender(self, sender: Sender<DiscoveryEvent>);
}

/// An event sent from a service discovery task.
#[derive(Debug, PartialEq, Eq)]
pub enum DiscoveryEvent {
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

impl DiscoveryEvent {
    /// Create a [`DiscoveryEvent::Add`] event.
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

    /// Create a [`DiscoveryEvent::Delete`] event.
    pub fn delete<U>(uid: U) -> Self
    where
        U: Into<String>,
    {
        Self::Delete { uid: uid.into() }
    }

    /// Create a [`DiscoveryEvent::Suspend`] event.
    pub fn suspend<U>(uid: U) -> Self
    where
        U: Into<String>,
    {
        Self::Suspend { uid: uid.into() }
    }

    /// Create a [`DiscoveryEvent::Resume`] event.
    pub fn resume<U>(uid: U) -> Self
    where
        U: Into<String>,
    {
        Self::Resume { uid: uid.into() }
    }
}
