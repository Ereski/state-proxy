use anyhow::anyhow;
use async_trait::async_trait;
use std::{
    borrow::Cow,
    collections::{hash_map::Entry, HashMap},
    io::Cursor,
    net::{SocketAddr, TcpStream, UdpSocket},
    num::NonZeroU32,
    result,
    sync::Arc,
};
use thiserror::Error;
use tokio::io::AsyncRead;

#[cfg(unix)]
use std::os::unix::net::UnixStream;

pub mod proxy;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Protocol '{name}' is already registered")]
    DuplicateRegistration { name: Arc<String> },
}

pub type Result<T> = result::Result<T, Error>;

/// A simple registry for all protocols the proxy can use to communicate.
#[derive(Default)]
pub struct ProtocolRegistry {
    protocols: HashMap<Arc<String>, Arc<dyn Protocol>>,
}

impl ProtocolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, protocol: Arc<dyn Protocol>) -> Result<()> {
        match self.protocols.entry(protocol.name().clone()) {
            Entry::Vacant(entry) => {
                entry.insert(protocol);

                Ok(())
            }
            Entry::Occupied(entry) => Err(Error::DuplicateRegistration {
                name: entry.key().clone(),
            }),
        }
    }

    pub fn get(&self, name: &String) -> Option<&Arc<dyn Protocol>> {
        self.protocols.get(name)
    }
}

/// Description of a single network protocol.
#[async_trait]
pub trait Protocol: Send + Sync {
    /// Return the lowercase name of the protocol. A version number must be included if there are
    /// multiple incompatible versions (e.g. SSH1 vs SSH2).
    fn name(&self) -> &Arc<String>;

    /// Get the capabilities for this protocol.
    fn capabilities(&self) -> ProtocolCapabilities {
        ProtocolCapabilities::default()
    }

    async fn new_client(
        &self,
        address: &str,
    ) -> anyhow::Result<Box<dyn Connection>> {
        Err(anyhow!(
            "Protocol '{}' does not offer a client",
            self.name()
        ))
    }

    async fn assemble_client(
        &self,
        state: ConnectionState,
    ) -> anyhow::Result<Box<dyn Connection>> {
        Err(anyhow!(
            "Protocol '{}' cannot assemble clients",
            self.name()
        ))
    }

    async fn new_server(
        &self,
        address: SocketAddr,
    ) -> anyhow::Result<Box<dyn ProtocolServer>> {
        Err(anyhow!(
            "Protocol '{}' does not offer a server",
            self.name()
        ))
    }
}

#[async_trait]
pub trait ProtocolServer: Send {
    async fn next(&mut self) -> anyhow::Result<Box<dyn Connection>>;

    async fn assemble_connection(
        &mut self,
        state: ConnectionState,
    ) -> anyhow::Result<()> {
        Err(anyhow!("This server does not support connection assembly"))
    }
}

#[async_trait]
pub trait Connection: Send {
    fn protocol(&self) -> &Arc<dyn Protocol>;

    async fn receive(&mut self) -> anyhow::Result<Message>;

    async fn send(&mut self, message: Message) -> anyhow::Result<()>;

    async fn new_channel(&mut self) -> anyhow::Result<u32> {
        Err(anyhow!("This connection does not support multiplexing"))
    }

    async fn disassemble(self: Box<Self>) -> anyhow::Result<ConnectionState> {
        Err(anyhow!("This connection does not support disassembly"))
    }
}

/// Frozen state for a single connection.
pub struct ConnectionState {
    /// The socket stream for this connection. May be `None` if the [`Connection`] is stateless.
    stream: Option<SocketStream>,

    /// Any internal data that is required for another instance to continue processing the
    /// connection seamlessly. This data should be as backwards- and forwards-compatible as
    /// possible to reduce the chance that an proxy upgrade or downgrade will require a
    /// reconnection.
    internal: Vec<u8>,
}

/// Capabilities for a single [`Protocol`].
pub struct ProtocolCapabilities {
    /// Whether a client is available for this protocol.
    pub client: bool,

    /// Whether a server is available for this protocol.
    pub server: bool,

    /// Whether this protocol's implementation supports connection disassembly and reassembly.
    pub disasm: bool,

    /// Whether this protocol has meaningful message boundaries. If false, this protocol should
    /// never return or accept more than one [`Message`] per direction per connection.
    pub message_boundaries: bool,

    /// The maximum number of multiplexed channels in a single connection for this protocol.
    pub multiplexing: NonZeroU32,

    /// Whether this protocol is capable of sending and receiving sockets.
    pub sockets: bool,
}

impl Default for ProtocolCapabilities {
    fn default() -> Self {
        Self {
            client: false,
            server: false,
            disasm: false,
            message_boundaries: false,
            multiplexing: unsafe { NonZeroU32::new_unchecked(1) },
            sockets: false,
        }
    }
}

#[derive(Default)]
pub struct Message {
    /// Raw data, if any.
    pub data: Option<Box<dyn AsyncRead + Send>>,

    /// Length of of the [`Message::data`] field if available.
    pub size: Option<u64>,

    /// ID of the channel if the protocol supports multiplexing.
    pub channel_id: Option<u32>,

    /// Sockets associated with the message if the protocol supports the transmission of sockets.
    pub sockets: Vec<SocketStream>,

    /// Any extra protocol-specific metadata.
    pub meta: HashMap<Cow<'static, str>, Cow<'static, str>>,
}

impl Message {
    pub fn with_data<D>(data: D) -> Self
    where
        D: AsyncRead + Send + 'static,
    {
        Self {
            data: Some(Box::new(data)),
            ..Self::default()
        }
    }

    pub fn with_data_from_buffer<B>(buffer: B) -> Self
    where
        B: AsRef<[u8]> + Unpin + Send + 'static,
    {
        Self::with_data(Cursor::new(buffer))
    }
}

#[non_exhaustive]
pub enum SocketStream {
    Tcp(TcpStream),
    Udp(UdpSocket),

    #[cfg(unix)]
    Uds(UnixStream),
}
