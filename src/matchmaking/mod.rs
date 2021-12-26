use crate::{
    backend::{PortEvent, ServiceManager},
    matchmaking::{
        client::ProtocolClient,
        server::{NewConnectionRequest, ProtocolServer},
    },
};
use anyhow::{anyhow, Context, Result};
use std::{
    collections::{hash_map::Entry, HashMap},
    sync::Arc,
};
use tokio::sync::{
    mpsc::{self, Sender},
    RwLock,
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

    protocol_clients:
        RwLock<HashMap<String, Arc<dyn ProtocolClient + Send + Sync>>>,
    protocol_servers:
        RwLock<HashMap<String, Arc<dyn ProtocolServer + Send + Sync>>>,
    open_ports: RwLock<HashMap<u16, Arc<dyn ProtocolServer + Send + Sync>>>,
}

impl Matchmaker {
    pub fn new(service_manager: Arc<ServiceManager>) -> Arc<Self> {
        Arc::new(Self {
            service_manager,

            protocol_clients: RwLock::new(HashMap::new()),
            protocol_servers: RwLock::new(HashMap::new()),
            open_ports: RwLock::new(HashMap::new()),
        })
    }

    pub async fn register_client<C>(&self, client: C) -> Result<()>
    where
        C: ProtocolClient + Send + Sync + 'static,
    {
        match self
            .protocol_clients
            .write()
            .await
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

    pub async fn register_server<S>(&self, server: S) -> Result<()>
    where
        S: ProtocolServer + Send + Sync + 'static,
    {
        match self
            .protocol_servers
            .write()
            .await
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

    pub async fn run(self: Arc<Self>) -> Result<()> {
        let (port_event_sender, mut port_event_receiver) =
            mpsc::channel(PORT_NOTIFICATION_BUFFER_SIZE);
        self.service_manager.send_port_events_to(port_event_sender);

        let this = Arc::new(self);
        while let Some(event) = port_event_receiver.recv().await {
            this.handle_port_event(event)
                .await
                .context("while trying to process a port event")
                .unwrap();
        }

        panic!("service manager closed the channel")
    }

    async fn handle_port_event(
        self: &Arc<Self>,
        event: PortEvent,
    ) -> Result<()> {
        match event {
            PortEvent::Open { port, protocol } => {
                if let Some(protocol_server) =
                    self.protocol_servers.read().await.get(&protocol)
                {
                    assert!(self.open_ports.read().await.get(&port).is_none());

                    // TODO: get the bind ip from an argument
                    protocol_server.listen(
                        ([0, 0, 0, 0], port).into(),
                        self.clone().new_connection_sender(port),
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
                let mut open_ports = self.open_ports.write().await;
                let open_ports_entry = open_ports.entry(port);
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

    fn new_connection_sender(
        self: Arc<Self>,
        port: u16,
    ) -> Sender<NewConnectionRequest> {
        let (new_connection_sender, mut new_connection_receiver) =
            mpsc::channel(NEW_CONNECTION_BUFFER_SIZE);
        tokio::spawn(async move {
            let mut i = 0_usize;
            while let Some(new_connection_request) =
                new_connection_receiver.recv().await
            {
                let this = self.clone();
                i = i.overflowing_add(1).0;
                tokio::spawn(async move {
                    // TODO: error handling....
                    let (address, protocol) = this
                        .service_manager
                        .get_endpoint_for(port, i)
                        .await
                        .unwrap();
                    let message_channel = this
                        .protocol_clients
                        .read()
                        .await
                        .get(&protocol)
                        .unwrap()
                        .connect(address);
                    unimplemented!()
                });
            }
        });

        new_connection_sender
    }
}
