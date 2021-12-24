use std::{net::SocketAddr, sync::Arc};
use tokio::sync::mpsc::Sender;

pub trait ServiceDiscovery {
    fn name(&self) -> Arc<String>;

    fn run_with_sender(self, sender: Sender<DiscoveryEvent>);
}

#[derive(Debug, PartialEq, Eq)]
pub enum DiscoveryEvent {
    Add {
        uid: String,
        is_available: bool,
        external_port: u16,
        external_protocol: String,
        backend_address: SocketAddr,
        backend_protocol: String,
    },
    Delete {
        uid: String,
    },
    Suspend {
        uid: String,
    },
    Resume {
        uid: String,
    },
}

impl DiscoveryEvent {
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

    pub fn delete<U>(uid: U) -> Self
    where
        U: Into<String>,
    {
        Self::Delete { uid: uid.into() }
    }

    pub fn suspend<U>(uid: U) -> Self
    where
        U: Into<String>,
    {
        Self::Suspend { uid: uid.into() }
    }

    pub fn resume<U>(uid: U) -> Self
    where
        U: Into<String>,
    {
        Self::Resume { uid: uid.into() }
    }
}
