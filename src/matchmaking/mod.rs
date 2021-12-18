use crate::matchmaking::{client::ProtocolClient, server::ProtocolServer};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::collections::{hash_map::Entry, HashMap};
use tokio::io::AsyncRead;
use tracing::info;

pub mod client;
pub mod server;

/// Struct responsible for matching external clients with internal servers.
pub struct Matchmaker {
    clients: HashMap<String, Box<dyn ProtocolClient + Send + Sync>>,
    servers: HashMap<String, Box<dyn ProtocolServer + Send + Sync>>,
}

impl Matchmaker {
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
            servers: HashMap::new(),
        }
    }

    pub fn register_client<C>(&mut self, client: C) -> Result<()>
    where
        C: ProtocolClient + Send + Sync + 'static,
    {
        match self.clients.entry(client.protocol().name().to_owned()) {
            Entry::Vacant(entry) => {
                info!("Client registered for {}", entry.key());
                entry.insert(Box::new(client));

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
        match self.servers.entry(server.protocol().name().to_owned()) {
            Entry::Vacant(entry) => {
                info!("Server registered for {}", entry.key());
                entry.insert(Box::new(server));

                Ok(())
            }
            Entry::Occupied(entry) => Err(anyhow!(
                "a server for the {} protocol is already registered",
                entry.key()
            )),
        }
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
