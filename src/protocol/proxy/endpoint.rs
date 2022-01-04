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
use anyhow::anyhow;
use async_trait::async_trait;
use lazy_static::lazy_static;
use serde_derive::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    net::SocketAddr,
    num::NonZeroU32,
    result,
    sync::{Arc, Weak},
};
use strum_macros::IntoStaticStr;
use thiserror::Error;
use tokio::sync::Mutex;

const ENDPOINT_PROTOCOL_VERSION: u32 = 0;

lazy_static! {
    static ref SERIALIZATION_FORMATS: Arc<Vec<String>> =
        Arc::new(vec!["json".to_owned()]);
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("base connection error: {0}")]
    ConnectionError(#[from] anyhow::Error),

    #[error(
        "can only create a single client or server from an `EndpointProtocol`"
    )]
    MoreThanOneClient,

    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("received an unknown channel ID {channel_id} for message: {message_name}")]
    UnknownChannelId {
        channel_id: u32,
        message_name: &'static str,
    },

    #[error("received a {received} message but expected one of: {expected:?}")]
    WrongMessage {
        expected: Vec<&'static str>,
        received: &'static str,
    },

    #[error("expected a Hello message with version {expected}, received version {received}")]
    WrongVersion { expected: u32, received: u32 },
}

pub type Result<T> = result::Result<T, Error>;

struct EndpointProtocol {
    name: Arc<String>,
    multiplexing: NonZeroU32,
    sockets: bool,

    base_connection: Mutex<Option<Box<dyn Connection>>>,

    this: Weak<dyn Protocol>,
}

impl EndpointProtocol {
    fn new(base_connection: Box<dyn Connection>) -> Arc<Self> {
        let base_capabilities = base_connection.protocol().capabilities();

        let res = Arc::new(Self {
            name: Arc::new(format!(
                "endpoint protocol over {}",
                base_connection.protocol().name()
            )),
            multiplexing: if base_capabilities.multiplexing.get() == 1 {
                unsafe { NonZeroU32::new_unchecked(u32::MAX) }
            } else {
                base_capabilities.multiplexing
            },
            sockets: base_capabilities.sockets,

            base_connection: Mutex::new(Some(base_connection)),

            this: Weak::<Self>::new(),
        });
        // TODO: this whole thing can be made simpler and safer with `new_cyclic`:
        // https://github.com/rust-lang/rust/issues/75861
        let this = Arc::downgrade(&res);
        (unsafe { &mut *(&*res as *const Self as *mut Self) }).this = this;

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
        address: &str,
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
            .await?,
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
    next_channel_id: u32,
    states: HashMap<u32, ChannelState>,

    endpoint_protocol: Arc<dyn Protocol>,
    base_connection: Box<dyn Connection>,
}

impl EndpointProtocolClient {
    pub async fn new(
        mut base_connection: Box<dyn Connection>,
        endpoint_protocol: Arc<dyn Protocol>,
    ) -> Result<Self> {
        // Perform initial negotiation with JSON-encoded `Hello` messages
        base_connection
            .send(Message::with_data_from_buffer(
                serde_json::to_vec(&ClientMessageV0::Hello {
                    version: ENDPOINT_PROTOCOL_VERSION,
                    serialization_formats: SERIALIZATION_FORMATS.clone(),
                })
                .unwrap(),
            ))
            .await?;
        let reply = base_connection.receive().await?;
        unimplemented!();

        Ok(Self {
            next_channel_id: 0,
            states: HashMap::new(),

            endpoint_protocol,
            base_connection,
        })
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

#[derive(IntoStaticStr, Deserialize, Serialize)]
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
        socket_ids: Option<&'a [u8]>,
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
        socket_id: Option<u32>,
    },
    SocketNotOpened {
        channel_id: Option<u32>,
        socket_id: Option<u32>,
        error: &'a str,
    },
    SocketError {
        channel_id: Option<u32>,
        socket_id: Option<u32>,
        error: &'a str,
    },
    SocketDataStart {
        channel_id: Option<u32>,
        socket_id: u32,
        size: Option<u64>,
    },
    SocketClosed {
        channel_id: Option<u32>,
        socket_id: Option<u32>,
    },
}

impl<'a> ClientMessageV0<'a> {
    fn name(&self) -> &'static str {
        self.into()
    }
}

#[derive(IntoStaticStr, Deserialize, Serialize)]
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
        size: Option<u64>,
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
        socket_id: Option<u32>,
        protocol: &'a str,
        address: &'a str,
    },
    SocketDataStart {
        channel_id: Option<u32>,
        socket_id: Option<u32>,
        size: Option<u64>,
    },
    CloseSocket {
        channel_id: Option<u32>,
        socket_id: Option<u32>,
    },
}

impl<'a> ServerMessageV0<'a> {
    fn name(&self) -> &'static str {
        self.into()
    }
}
