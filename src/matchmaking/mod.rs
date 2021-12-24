use crate::{
    backend::{PortEvent, ServiceManager},
    matchmaking::{
        client::ProtocolClient,
        server::{NewConnectionRequest, ProtocolServer},
    },
};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use std::{
    collections::{hash_map::Entry, HashMap},
    sync::Arc,
};
use tokio::{
    io::AsyncRead,
    sync::mpsc::{self, Sender},
};
use tracing::{info, warn};

pub mod client;
pub mod server;

// This value should be on the higher end to deal with sudden spikes without blocking the protocol
// server. This is per server
const NEW_CONNECTION_BUFFER_SIZE: usize = 4096;
// We don't expect many of these to pile up so it's ok to give this a lower value
const PORT_NOTIFICATION_BUFFER_SIZE: usize = 8;

/// Struct responsible for matching external clients with internal servers.
pub struct Matchmaker {
    service_manager: Arc<ServiceManager>,

    protocol_clients: HashMap<String, Arc<dyn ProtocolClient + Send + Sync>>,
    protocol_servers: HashMap<String, Arc<dyn ProtocolServer + Send + Sync>>,
    open_ports: HashMap<u16, Arc<dyn ProtocolServer + Send + Sync>>,
}

impl Matchmaker {
    pub fn new(service_manager: Arc<ServiceManager>) -> Self {
        Self {
            service_manager,

            protocol_clients: HashMap::new(),
            protocol_servers: HashMap::new(),
            open_ports: HashMap::new(),
        }
    }

    pub fn register_client<C>(&mut self, client: C) -> Result<()>
    where
        C: ProtocolClient + Send + Sync + 'static,
    {
        match self
            .protocol_clients
            .entry(client.protocol().name().to_owned())
        {
            Entry::Vacant(entry) => {
                info!("Client registered for {}", entry.key());
                entry.insert(Arc::new(client));

                Ok(())
            }
            Entry::Occupied(entry) => Err(anyhow!(
                "a client for the {} protocol is already registered",
                entry.key()
            )),
        }
    }

    pub fn register_server<S>(&mut self, server: S) -> Result<()>
    where
        S: ProtocolServer + Send + Sync + 'static,
    {
        match self
            .protocol_servers
            .entry(server.protocol().name().to_owned())
        {
            Entry::Vacant(entry) => {
                info!("Server registered for {}", entry.key());
                entry.insert(Arc::new(server));

                Ok(())
            }
            Entry::Occupied(entry) => Err(anyhow!(
                "a server for the {} protocol is already registered",
                entry.key()
            )),
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        let (port_event_sender, mut port_event_receiver) =
            mpsc::channel(PORT_NOTIFICATION_BUFFER_SIZE);
        self.service_manager.send_ports_events_to(port_event_sender);

        while let Some(event) = port_event_receiver.recv().await {
            self.handle_port_event(event)
                .await
                .context("while trying to process a port event")
                .unwrap();
        }

        panic!("service manager closed the channel")
    }

    async fn handle_port_event(&mut self, event: PortEvent) -> Result<()> {
        match event {
            PortEvent::Open { port, protocol } => {
                if let Some(protocol_server) =
                    self.protocol_servers.get(&protocol)
                {
                    assert!(self.open_ports.get(&port).is_none());

                    // TODO: get the bind ip from an argument
                    protocol_server.listen(
                        ([0, 0, 0, 0], port).into(),
                        self.new_connection_sender(),
                    );

                    info!(
                        "Listening port {} for protocol '{}'",
                        protocol, port
                    );
                } else {
                    return Err(anyhow!("unknown protocol: {}", protocol));
                }
            }
            PortEvent::Close { port } => {
                let open_ports_entry = self.open_ports.entry(port);
                if let Entry::Occupied(open_ports_entry) = open_ports_entry {
                    let (_, protocol_server) = open_ports_entry.remove_entry();
                    protocol_server.mute(port);
                } else {
                    warn!("Received a close port event for a port that is not open: {}", port);
                }
            }
        }

        Ok(())
    }

    fn new_connection_sender(&self) -> Sender<NewConnectionRequest> {
        let (new_connection_sender, mut new_connection_receiver) =
            mpsc::channel(NEW_CONNECTION_BUFFER_SIZE);
        tokio::spawn(async move {
            while let Some(new_connection_request) =
                new_connection_receiver.recv().await
            {
                unimplemented!()
            }
        });

        new_connection_sender
    }
}

/// Trait for channels that permit bidirectional, message-based communication.
///
/// Use [`macro@async_trait`] to implement this trait.
#[async_trait]
pub trait MessageChannel {
    /// Try to send a message through the channel.
    async fn send(&self, msg: Message) -> Result<()>;

    /// Try to receive a message through the channel.
    async fn recv(&self) -> Result<Message>;
}

pub struct Message {
    data: Box<dyn AsyncRead + Send>,
    // TODO: ancillary
}
