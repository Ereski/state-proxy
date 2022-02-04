//! The endpoint protcol. This protocol is used to communicate with endpoints over a base
//! protocol requested by the endpoint. This base protocol can range from raw TCP to SSH and
//! anything in between.
//!
//! The endpoint protocol defines a set of JSON-encoded messages that allow not only
//! communication with the proxied client, but also storage of state in the proxy and connection
//! multiplexing.

use crate::protocol::{
    Connection, Message, Protocol, ProtocolCapabilities, ProtocolServer,
};
use anyhow::{anyhow, Context};
use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use lazy_static::lazy_static;
use maplit::hashmap;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io,
    net::SocketAddr,
    num::NonZeroU32,
    pin::Pin,
    result,
    sync::{Arc, Weak},
};
use strum::IntoEnumIterator;
use strum_macros::{EnumDiscriminants, EnumIter, EnumString, IntoStaticStr};
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    sync::Mutex,
};
use tracing::{debug, instrument};

const ENDPOINT_PROTOCOL_VERSION: u32 = 0;
const MAX_MESSAGE_BINARY_SIZE: usize = 1024;
const MESSAGE_BUFFER_SIZE: usize = MAX_MESSAGE_BINARY_SIZE + 1;

lazy_static! {
    static ref SERIALIZATION_FORMATS: Arc<Vec<String>> = Arc::new(
        SerializationFormat::iter()
            .map(|x| <&'static str>::from(x).to_owned())
            .collect()
    );
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("base connection error: {0}")]
    ConnectionError(#[from] anyhow::Error),

    #[error("message is incomplete after reading {read} bytes")]
    IncompleteMessage { read: usize },

    #[error("I/O error: {0}")]
    IoError(#[from] io::Error),

    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("message too large to deserialize")]
    MessageTooLarge,

    #[error(
        "can only create a single client or server from an `EndpointProtocol`"
    )]
    MoreThanOneClient,

    #[error("server created a new channel with ID {channel_id}")]
    ServerCreatedChannel { channel_id: u32 },

    #[error("received an unknown channel ID {channel_id} for message: {message_name}")]
    UnknownChannelId {
        channel_id: u32,
        message_name: &'static str,
    },

    #[error("server selected an unknown serialization format: {received}. Expected one of: {received}")]
    UnknownSerializationFormat {
        expected: Arc<Vec<String>>,
        received: String,
    },

    #[error("received a {received} message but expected one of: {expected:?}")]
    WrongMessage {
        expected: Vec<&'static str>,
        received: &'static str,
    },

    #[error("expected a `Hello` message with version {expected}, received version {received}")]
    WrongVersion { expected: u32, received: u32 },
}

pub type Result<T> = result::Result<T, Error>;

pub struct EndpointProtocol {
    name: Arc<String>,
    multiplexing: NonZeroU32,
    sockets: bool,

    base_connection: Mutex<Option<Box<dyn Connection>>>,

    this: Weak<dyn Protocol>,
}

impl EndpointProtocol {
    pub fn new(base_connection: Box<dyn Connection>) -> Arc<Self> {
        let base_capabilities = base_connection.protocol().capabilities();

        let mut res = Arc::new(Self {
            name: Arc::new(format!(
                "endpoint protocol over {}",
                base_connection.protocol().name()
            )),
            multiplexing: if base_capabilities.multiplexing.get() == 1 {
                unsafe { NonZeroU32::new_unchecked(u32::MAX) }
            } else {
                base_capabilities.multiplexing
            },
            sockets: false,

            base_connection: Mutex::new(Some(base_connection)),

            this: Weak::<Self>::new(),
        });
        // TODO: this whole thing can be made simpler and safer with `new_cyclic`:
        // https://github.com/rust-lang/rust/issues/75861
        let this = Arc::downgrade(&res);
        Arc::get_mut(&mut res).unwrap().this = this;

        res
    }
}

#[async_trait]
impl Protocol for EndpointProtocol {
    fn name(&self) -> &Arc<String> {
        &self.name
    }

    fn capabilities(&self) -> ProtocolCapabilities {
        ProtocolCapabilities {
            client: true,
            server: true,
            // Always false because it doesn't make sense to disasm the endpoint protocol
            disasm: false,
            message_boundaries: true,
            multiplexing: self.multiplexing,
            sockets: self.sockets,
        }
    }

    async fn new_client(
        &self,
        _address: &str,
    ) -> anyhow::Result<Box<dyn Connection>> {
        Ok(Box::new(
            EndpointProtocolClient::new(
                self.base_connection
                    .lock()
                    .await
                    .take()
                    .ok_or(Error::MoreThanOneClient)?,
                self.this.upgrade().ok_or_else(|| {
                    anyhow!(
                    "Arc<EndpointProtocol> dropped. Cannot create a new client"
                )
                })?,
            )
            .await
            .with_context(|| {
                format!("while initializing a client connection for the endpoint protocol")
            })?,
        ))
    }

    async fn new_server(
        &self,
        address: SocketAddr,
    ) -> anyhow::Result<Box<dyn ProtocolServer>> {
        unimplemented!()
    }
}

pub struct EndpointProtocolClient {
    states: HashMap<u32, ChannelState>,

    base_connection: Box<dyn Connection>,
    endpoint_protocol: Arc<dyn Protocol>,

    base_capabilities: ProtocolCapabilities,
    next_channel_id: u32,
    read_buffer_map: HashMap<Option<u32>, BytesMut>,
    stream_left: Option<(Pin<Box<dyn AsyncRead + Send>>, u32)>,
    serialization_format: SerializationFormat,
}

impl EndpointProtocolClient {
    async fn new(
        base_connection: Box<dyn Connection>,
        endpoint_protocol: Arc<dyn Protocol>,
    ) -> Result<Self> {
        let base_capabilities = base_connection.protocol().capabilities();
        let mut this = Self {
            states: HashMap::new(),

            base_connection,
            endpoint_protocol,

            base_capabilities,
            next_channel_id: 0,
            read_buffer_map: hashmap! {
                None => BytesMut::with_capacity(MESSAGE_BUFFER_SIZE),
            },
            stream_left: None,
            serialization_format: SerializationFormat::Json,
        };
        this.init().await?;

        Ok(this)
    }

    async fn init(&mut self) -> Result<()> {
        // Perform initial negotiation with JSON-encoded `Hello` messages
        self.send_client_message(ClientMessageV0::Hello {
            version: ENDPOINT_PROTOCOL_VERSION,
            serialization_formats: SERIALIZATION_FORMATS.clone(),
        })
        .await?;
        let reply = self.receive_server_message().await?;

        match reply.get() {
            ServerMessageV0::Hello {
                version,
                serialization_format,
            } => {
                if *version != ENDPOINT_PROTOCOL_VERSION {
                    Err(Error::WrongVersion {
                        expected: ENDPOINT_PROTOCOL_VERSION,
                        received: *version,
                    })
                } else if let Ok(serialization_format) =
                    SerializationFormat::try_from(*serialization_format)
                {
                    self.serialization_format = serialization_format;

                    Ok(())
                } else {
                    Err(Error::UnknownSerializationFormat {
                        expected: SERIALIZATION_FORMATS.clone(),
                        received: (*serialization_format).to_owned(),
                    })
                }
            }
            x => Err(Error::WrongMessage {
                expected: vec![ServerMessageV0Discriminants::Hello.into()],
                received: x.name(),
            }),
        }
    }

    async fn send_client_message(
        &mut self,
        mut message: ClientMessageV0<'_>,
    ) -> Result<()> {
        let mut channel_id = match message.take_channel_id() {
            TakeChannelIdResult::Id(channel_id) => Some(channel_id),
            TakeChannelIdResult::Uneeded => None,
            TakeChannelIdResult::Missing => {
                panic!("BUG: missing channel ID in: {:#?}", message);
            }
        };
        if self.base_capabilities.multiplexing.get() == 1
            && channel_id.is_some()
        {
            message.set_channel_id(channel_id.unwrap());
            channel_id = None;
        }

        let payload = self.serialization_format.serialize(&message).unwrap();
        let mut message = Message::with_data_from_buffer(payload);
        message.channel_id = channel_id;
        self.base_connection.send(message).await?;

        Ok(())
    }

    async fn receive_server_message(&mut self) -> Result<OwnedServerMessageV0> {
        loop {
            let (mut data, channel_id) =
                if let Some((data, channel_id)) = self.stream_left.take() {
                    (data, Some(channel_id))
                } else {
                    self.receive_message_with_data().await?
                };

            let read_buffer = self
                .read_buffer_map
                .get_mut(&channel_id)
                .ok_or(Error::ServerCreatedChannel {channel_id: channel_id.expect("BUG: the default channel is not present in the read buffer map")})?;
            let data_is_exhausted =
                fill_buffer_from(read_buffer, &mut data).await?;
            if read_buffer.len() > MAX_MESSAGE_BINARY_SIZE
                && self.base_capabilities.message_boundaries
            {
                read_buffer.clear();

                return Err(Error::MessageTooLarge);
            }
        }

        unimplemented!();

        /*let (channel_id, message) = if self.base_capabilities.message_boundaries
        {
            let message = self.base_connection.receive().await?;
            let mut data =
                message.data.ok_or(Error::IncompleteMessage { read: 0 })?;
            let channel_id = if self.base_capabilities.multiplexing.get() == 1 {
                None
            } else {
                message.channel_id
            };

            let mut size = 0;
            self.read_buffer.resize(MAX_MESSAGE_BINARY_SIZE + 1, 0);
            {
                let mut buffer = self.read_buffer.as_mut();
                loop {
                    let read = data.read(buffer).await?;
                    if read == 0 {
                        break;
                    }

                    size += read;
                    if size > MAX_MESSAGE_BINARY_SIZE {
                        return Err(Error::MessageTooLarge);
                    }

                    buffer = &mut buffer[read..];
                }
            }

            (
                channel_id,
                OwnedServerMessageV0::new(&mut self.read_buffer, |x| {
                    self.serialization_format.deserialize(x)
                })?,
            )
        } else {
            unimplemented!()
        };

        unimplemented!()*/
    }

    #[instrument(skip_all)]
    async fn receive_message_with_data(
        &mut self,
    ) -> Result<(Pin<Box<dyn AsyncRead + Send>>, Option<u32>)> {
        loop {
            let message = self.base_connection.receive().await?;
            let data = if let Some(data) = message.data {
                data
            } else {
                debug!("received a message with no data");

                continue;
            };
            let channel_id = if self.base_capabilities.multiplexing.get() == 1 {
                None
            } else {
                message.channel_id
            };

            return Ok((data, channel_id));
        }
    }
}

#[async_trait]
impl Connection for EndpointProtocolClient {
    fn protocol(&self) -> &Arc<dyn Protocol> {
        &self.endpoint_protocol
    }

    async fn receive(&mut self) -> anyhow::Result<Message> {
        unimplemented!()
    }

    async fn send(&mut self, message: Message) -> anyhow::Result<()> {
        unimplemented!()
    }

    async fn new_channel(&mut self) -> anyhow::Result<u32> {
        unimplemented!()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, EnumIter, EnumString, IntoStaticStr)]
enum SerializationFormat {
    Json,
}

impl SerializationFormat {
    fn deserialize<'a, T>(self, buffer: &'a [u8]) -> Result<Option<(usize, T)>>
    where
        T: Deserialize<'a>,
    {
        match self {
            Self::Json => {
                let mut deserializer = serde_json::StreamDeserializer::new(
                    serde_json::de::SliceRead::new(buffer),
                );

                match deserializer.next() {
                    Some(Ok(message)) => {
                        Ok(Some((deserializer.byte_offset(), message)))
                    }
                    Some(Err(err)) if err.is_eof() => Ok(None),
                    Some(Err(err)) => Err(err.into()),
                    None => Ok(None),
                }
            }
        }
    }

    fn serialize<D>(self, data: D) -> Result<Vec<u8>>
    where
        D: Serialize,
    {
        match self {
            Self::Json => Ok(serde_json::to_vec(&data)?),
        }
    }
}

struct ChannelState {
    client: ChannelClientState,
    server: ChannelServerState,
}

impl ChannelState {
    fn new() -> Self {
        Self::default()
    }
}

impl Default for ChannelState {
    fn default() -> Self {
        Self {
            client: ChannelClientState::WaitingForAcceptance,
            server: ChannelServerState::Idle,
        }
    }
}

enum ChannelServerState {
    Idle,
    StartReceivingData {
        size: Option<u64>,
    },
    ReceivingChunk {
        total_size: Option<u64>,
        chunk_size: Option<u64>,
        progress: u64,
    },
    StartReceivingSocketData {
        socket_id: u32,
        size: Option<u64>,
    },
    ReceivingSocketChunk {
        socket_id: u32,
        total_size: Option<u64>,
        chunk_size: Option<u64>,
        progress: u64,
    },
}

enum ChannelClientState {
    WaitingForAcceptance,
    Idle,
    StartReceivingData {
        size: Option<u64>,
    },
    ReceivingChunk {
        total_size: Option<u64>,
        chunk_size: Option<u64>,
        progress: u64,
    },
    StartReceivingSocketData {
        socket_id: u32,
        size: Option<u64>,
    },
    ReceivingSocketChunk {
        socket_id: u32,
        total_size: Option<u64>,
        chunk_size: Option<u64>,
        progress: u64,
    },
}

#[derive(
    Debug,
    EnumDiscriminants,
    IntoStaticStr,
    serde_derive::Deserialize,
    serde_derive::Serialize,
)]
#[strum_discriminants(derive(IntoStaticStr))]
#[serde(deny_unknown_fields)]
enum ServerMessageV0<'a> {
    Hello {
        version: u32,
        serialization_format: &'a str,
    },
    Accept {
        channel_id: Option<u32>,
    },
    Deny {
        channel_id: Option<u32>,
        error: &'a str,
    },
    DataStart {
        channel_id: Option<u32>,
        size: Option<u64>,
        state: Option<&'a [u8]>,
    },
    DataChunk {
        channel_id: Option<u32>,
        size: u64,
        state: Option<&'a [u8]>,
    },
    DataEnd {
        channel_id: Option<u32>,
        state: Option<&'a [u8]>,
    },
    SetState {
        channel_id: Option<u32>,
        state: Option<&'a [u8]>,
    },
    OpenSocket {
        channel_id: Option<u32>,
        socket_id: u32,
        protocol: &'a str,
        address: &'a str,
    },
    SocketDataStart {
        channel_id: Option<u32>,
        socket_id: u32,
        size: Option<u64>,
    },
    CloseSocket {
        channel_id: Option<u32>,
        socket_id: u32,
    },
}

impl<'a> ServerMessageV0<'a> {
    fn name(&self) -> &'static str {
        self.into()
    }

    fn take_channel_id(&mut self) -> TakeChannelIdResult {
        match self {
            Self::Hello { .. } => TakeChannelIdResult::Uneeded,
            Self::Accept { channel_id }
            | Self::Deny { channel_id, .. }
            | Self::DataStart { channel_id, .. }
            | Self::DataChunk { channel_id, .. }
            | Self::DataEnd { channel_id, .. }
            | Self::SetState { channel_id, .. }
            | Self::OpenSocket { channel_id, .. }
            | Self::SocketDataStart { channel_id, .. }
            | Self::CloseSocket { channel_id, .. } => {
                if let Some(channel_id) = channel_id.take() {
                    TakeChannelIdResult::Id(channel_id)
                } else {
                    TakeChannelIdResult::Missing
                }
            }
        }
    }

    fn set_channel_id(&mut self, new_channel_id: u32) {
        match self {
            Self::Hello { .. } => (),
            Self::Accept { channel_id }
            | Self::Deny { channel_id, .. }
            | Self::DataStart { channel_id, .. }
            | Self::DataChunk { channel_id, .. }
            | Self::DataEnd { channel_id, .. }
            | Self::SetState { channel_id, .. }
            | Self::OpenSocket { channel_id, .. }
            | Self::SocketDataStart { channel_id, .. }
            | Self::CloseSocket { channel_id, .. } => {
                *channel_id = Some(new_channel_id);
            }
        }
    }
}

struct OwnedServerMessageV0 {
    message: ServerMessageV0<'static>,
    buffer: Bytes,
}

impl OwnedServerMessageV0 {
    fn new<D>(buffer: &mut BytesMut, deserialize: D) -> Result<Self>
    where
        D: for<'a> FnOnce(
            &'a [u8],
        )
            -> Result<Option<(usize, ServerMessageV0<'a>)>>,
    {
        unimplemented!()
    }

    fn get(&self) -> &ServerMessageV0 {
        unimplemented!()
    }
}

#[derive(
    Debug,
    EnumDiscriminants,
    IntoStaticStr,
    serde_derive::Deserialize,
    serde_derive::Serialize,
)]
#[strum_discriminants(derive(IntoStaticStr))]
#[serde(deny_unknown_fields)]
enum ClientMessageV0<'a> {
    Hello {
        version: u32,
        serialization_formats: Arc<Vec<String>>,
    },
    NewChannel {
        channel_id: Option<u32>,
        from: SocketAddr,
    },
    RecoverChannel {
        channel_id: Option<u32>,
        from: SocketAddr,
        state: &'a [u8],
    },
    CloseChannel {
        channel_id: Option<u32>,
        error: Option<&'a str>,
    },
    DataStart {
        channel_id: Option<u32>,
        size: Option<u64>,
    },
    DataChunk {
        channel_id: Option<u32>,
        size: Option<u64>,
    },
    DataEnd {
        channel_id: Option<u32>,
    },
    SocketOpened {
        channel_id: Option<u32>,
        socket_id: u32,
    },
    SocketNotOpened {
        channel_id: Option<u32>,
        socket_id: u32,
        error: &'a str,
    },
    SocketError {
        channel_id: Option<u32>,
        socket_id: u32,
        error: &'a str,
    },
    SocketDataStart {
        channel_id: Option<u32>,
        socket_id: u32,
        size: Option<u64>,
    },
    SocketClosed {
        channel_id: Option<u32>,
        socket_id: u32,
    },
}

impl<'a> ClientMessageV0<'a> {
    fn name(&self) -> &'static str {
        self.into()
    }

    fn take_channel_id(&mut self) -> TakeChannelIdResult {
        match self {
            Self::Hello { .. } => TakeChannelIdResult::Uneeded,
            Self::NewChannel { channel_id, .. }
            | Self::RecoverChannel { channel_id, .. }
            | Self::CloseChannel { channel_id, .. }
            | Self::DataStart { channel_id, .. }
            | Self::DataChunk { channel_id, .. }
            | Self::DataEnd { channel_id, .. }
            | Self::SocketOpened { channel_id, .. }
            | Self::SocketNotOpened { channel_id, .. }
            | Self::SocketError { channel_id, .. }
            | Self::SocketDataStart { channel_id, .. }
            | Self::SocketClosed { channel_id, .. } => {
                if let Some(channel_id) = channel_id.take() {
                    TakeChannelIdResult::Id(channel_id)
                } else {
                    TakeChannelIdResult::Missing
                }
            }
        }
    }

    fn set_channel_id(&mut self, new_channel_id: u32) {
        match self {
            Self::Hello { .. } => (),
            Self::NewChannel { channel_id, .. }
            | Self::RecoverChannel { channel_id, .. }
            | Self::CloseChannel { channel_id, .. }
            | Self::DataStart { channel_id, .. }
            | Self::DataChunk { channel_id, .. }
            | Self::DataEnd { channel_id, .. }
            | Self::SocketOpened { channel_id, .. }
            | Self::SocketNotOpened { channel_id, .. }
            | Self::SocketError { channel_id, .. }
            | Self::SocketDataStart { channel_id, .. }
            | Self::SocketClosed { channel_id, .. } => {
                *channel_id = Some(new_channel_id);
            }
        }
    }
}

enum TakeChannelIdResult {
    Id(u32),
    Uneeded,
    Missing,
}

async fn fill_buffer_from<D>(
    read_buffer: &mut BytesMut,
    data: &mut D,
) -> Result<bool>
where
    D: AsyncRead + Unpin,
{
    let mut in_buffer = read_buffer.len();
    assert!(in_buffer <= MESSAGE_BUFFER_SIZE);
    read_buffer.resize(MESSAGE_BUFFER_SIZE, 0);
    {
        let mut read_buffer_slice = &mut read_buffer[in_buffer..];
        loop {
            let read = data.read(read_buffer_slice).await?;
            in_buffer += read;
            if read == 0 || in_buffer > MAX_MESSAGE_BINARY_SIZE {
                break;
            }

            read_buffer_slice = &mut read_buffer_slice[in_buffer..];
        }
    }
    read_buffer.truncate(in_buffer);

    Ok(in_buffer < MESSAGE_BUFFER_SIZE)
}
