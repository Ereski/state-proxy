use crate::matchmaking::{
    client::ProtocolClient,
    server::{NewConnectionRequest, ProtocolServer, ServerConnectionState},
};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use futures::{FutureExt, TryFutureExt};
use std::{
    collections::{hash_map::Entry, HashMap},
    error,
    future::Future,
    net::SocketAddr,
    pin::Pin,
    result,
    sync::Arc,
};
use tokio::{
    io::AsyncRead,
    sync::mpsc::{Receiver, Sender},
};

pub mod client;
pub mod server;

#[derive(Default)]
// Struct responsible for matching external clients with internal services.
pub struct Matchmaker {
    clients: HashMap<&'static str, ErasedProtocolClient>,
    servers: HashMap<&'static str, ErasedProtocolServer>,
}

impl Matchmaker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_client<C, P>(&mut self, client: C) -> Result<()>
    where
        C: ProtocolClient<P> + Send + Sync + 'static,
        P: Protocol,
    {
        match self.clients.entry(P::NAME) {
            Entry::Vacant(entry) => {
                entry.insert(ErasedProtocolClient::new(client));

                Ok(())
            }
            Entry::Occupied(_) => Err(anyhow!(
                "a client for the {} protocol is already registered",
                P::NAME
            )),
        }
    }

    pub fn register_server<S, P>(&mut self, server: S) -> Result<()>
    where
        S: ProtocolServer<P> + Send + Sync + 'static,
        P: Protocol,
    {
        match self.servers.entry(P::NAME) {
            Entry::Vacant(entry) => {
                entry.insert(ErasedProtocolServer::new(server));

                Ok(())
            }
            Entry::Occupied(_) => Err(anyhow!(
                "a server for the {} protocol is already registered",
                P::NAME
            )),
        }
    }

    pub async fn run(self) -> Result<()> {
        unimplemented!()
    }
}

struct ErasedProtocolClient {
    connect: Box<
        dyn Fn(SocketAddr) -> Pin<Box<dyn Future<Output = Result<ErasedDuplexMessageChannel>>>>,
    >,
}

impl ErasedProtocolClient {
    fn new<C, P>(client: C) -> Self
    where
        C: ProtocolClient<P> + Send + Sync + 'static,
        P: Protocol,
    {
        let client = Arc::new(client);

        Self {
            connect: Box::new(move |addr| {
                // TODO: this is a cheap clone, but still a clone to go around lifetime
                // shenanigans. Also see `ErasedDuplexMessageChannel`
                let client = client.clone();

                Box::pin(async move {
                    match client.connect(addr).await {
                        Ok(channel) => Ok(ErasedDuplexMessageChannel::new(channel)),
                        Err(err) => Err(err.into()),
                    }
                })
            }),
        }
    }

    async fn connect(&self, addr: SocketAddr) -> Result<ErasedDuplexMessageChannel> {
        Ok((self.connect)(addr).await?)
    }
}

struct ErasedProtocolServer {
    listen: Box<dyn Fn(SocketAddr, Sender<NewConnectionRequest>)>,
    insert_state: Box<dyn Fn(ServerConnectionState) -> Result<()>>,
    extract_states: Box<dyn FnOnce() -> Receiver<ServerConnectionState>>,
}

impl ErasedProtocolServer {
    fn new<S, P>(server: S) -> Self
    where
        S: ProtocolServer<P> + Send + Sync + 'static,
        P: Protocol,
    {
        let server = Arc::new(server);
        let server_listen = server.clone();
        let server_insert_state = server.clone();

        Self {
            listen: Box::new(move |addr, new_connection_send| {
                server_listen.listen(addr, new_connection_send)
            }),
            insert_state: Box::new(move |state| {
                server_insert_state
                    .insert_state(state)
                    .map_err(|err| err.into())
            }),
            extract_states: Box::new(|| match Arc::try_unwrap(server) {
                Ok(server) => server.extract_states(),
                Err(_) => panic!("BUG: inner ProtocolServer has more than one reference"),
            }),
        }
    }

    fn listen(&self, addr: SocketAddr, new_connection_send: Sender<NewConnectionRequest>) {
        (self.listen)(addr, new_connection_send)
    }

    fn insert_state(&self, state: ServerConnectionState) -> Result<()> {
        (self.insert_state)(state)
    }

    fn extract_states(self) -> Receiver<ServerConnectionState> {
        let Self { extract_states, .. } = self;

        extract_states()
    }
}

struct ErasedDuplexMessageChannel {
    send: Box<dyn Fn(Message) -> Pin<Box<dyn Future<Output = Result<()>>>>>,
    recv: Box<dyn Fn() -> Pin<Box<dyn Future<Output = Result<Message>>>>>,
}

impl ErasedDuplexMessageChannel {
    fn new<C>(channel: C) -> Self
    where
        C: DuplexChannel<Message> + 'static,
    {
        let channel_send = Arc::new(channel);
        let channel_recv = channel_send.clone();

        Self {
            send: Box::new(move |msg| {
                let channel = channel_send.clone();

                Box::pin(async move { Ok(channel.send(msg).await?) })
            }),
            recv: Box::new(move || {
                let channel = channel_recv.clone();

                Box::pin(async move { Ok(channel.recv().await?) })
            }),
        }
    }

    async fn send(&self, msg: Message) -> Result<()> {
        (self.send)(msg).await
    }

    async fn recv(&self) -> Result<Message> {
        (self.recv)().await
    }
}

// Description of a single network protocol.
pub trait Protocol {
    // Lowercase name of the protocol. A version number must be included if there are multiple
    // incompatible versions (e.g. SSH1 vs SSH2).
    const NAME: &'static str;
}

// Trait for channels that permit bidirectional communication.
//
// Use [`async_trait`] when implementing this trait.
#[async_trait]
pub trait DuplexChannel<M> {
    // Error type for this channel;
    type Error: error::Error + Send + Sync + 'static;

    // Try to send a message through the channel.
    async fn send(&self, msg: M) -> result::Result<(), Self::Error>;

    // Try to receive a message through the channel.
    async fn recv(&self) -> result::Result<M, Self::Error>;
}

pub struct Message {
    data: Box<dyn AsyncRead + Send>,
    // TODO: ancillary
}
