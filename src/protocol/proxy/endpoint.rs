//! The endpoint protcol. This protocol is used to communicate with endpoints over a base
//! protocol requested by the endpoint. This base protocol can range from raw TCP to SSH and
//! anything in between.
//!
//! The endpoint protocol defines a set of JSON-encoded messages that allow not only
//! communication with the proxied client, but also storage of state in the proxy and connection
//! multiplexing.

use crate::protocol::{
    AncillaryData, Message, MessageChannel, Protocol, ProtocolCapabilities,
};
use serde_derive::{Deserialize, Serialize};
use std::{net::SocketAddr, num::NonZeroU32, result, sync::Arc};
use strum_macros::IntoStaticStr;
use thiserror::Error;

const PROXY_PROTOCOL_VERSION: u32 = 0;

#[derive(Debug, Error)]
pub enum Error {
    #[error("cannot deserialize message: {0}")]
    BadMessage(#[from] serde_json::Error),

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
    name: String,

    multiplexing: NonZeroU32,
    sockets: bool,
    base_protocol: Arc<dyn Protocol + Send + Sync>,
}

impl EndpointProtocol {
    fn new(base_protocol: Arc<dyn Protocol + Send + Sync>) -> Self {
        let base_capabilities = base_protocol.capabilities();

        Self {
            name: format!("endpoint protocol over {}", base_protocol.name()),

            multiplexing: if base_capabilities.multiplexing.get() == 1 {
                unsafe { NonZeroU32::new_unchecked(u32::MAX) }
            } else {
                base_capabilities.multiplexing
            },
            sockets: base_capabilities.sockets,
            base_protocol,
        }
    }
}

impl Protocol for EndpointProtocol {
    fn name(&self) -> &str {
        &self.name
    }

    fn translate(&self, message: Message) -> anyhow::Result<Message> {
        unimplemented!()
    }

    fn capabilities(&self) -> ProtocolCapabilities {
        ProtocolCapabilities {
            multiplexing: self.multiplexing,
            sockets: self.sockets,
        }
    }
}

pub struct EndpointProtocolServer {
    next_channel_id: u32,
    endpoint_channel: MessageChannel,
}

impl EndpointProtocolServer {
    pub fn new(endpoint_channel: MessageChannel) -> Self {
        Self {
            next_channel_id: 0,
            endpoint_channel,
        }
    }

    pub fn new_channel(&mut self) -> MessageChannel {
        let multiplexed_channel = MultiplexedChannelServer::start(
            self.next_channel_id,
            self.endpoint_channel.clone(),
        );
        self.next_channel_id += 1;

        multiplexed_channel
    }
}

struct MultiplexedChannelServer {
    id: u32,
    client_state: MultiplexedChannelClientState,
    server_state: MultiplexedChannelServerState,

    endpoint_protocol: Arc<EndpointProtocol>,
    endpoint_channel: MessageChannel,
    proxy_channel: MessageChannel,
}

impl MultiplexedChannelServer {
    fn start(id: u32, endpoint_channel: MessageChannel) -> MessageChannel {
        let endpoint_protocol = Arc::new(EndpointProtocol::new(
            endpoint_channel.protocol().clone(),
        ));
        let (proxy_channel, proxy_channel2) =
            MessageChannel::create(endpoint_protocol.clone());
        tokio::spawn(async move {
            Self {
                id,
                client_state: MultiplexedChannelClientState::Idle,
                server_state:
                    MultiplexedChannelServerState::WaitingForAcceptance,

                endpoint_protocol,
                endpoint_channel,
                proxy_channel,
            }
            .run()
            .await
        });

        proxy_channel2
    }

    async fn run(self) {
        unimplemented!()
    }
}

enum MultiplexedChannelClientState {
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

enum MultiplexedChannelServerState {
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

#[derive(IntoStaticStr, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
enum ServerMessageV0<'a> {
    Hello {
        version: u32,
        client_protocols: Arc<Vec<String>>,
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

impl<'a> ServerMessageV0<'a> {
    fn name(&self) -> &'static str {
        self.into()
    }
}

#[derive(IntoStaticStr, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
enum ClientMessageV0<'a> {
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

impl<'a> ClientMessageV0<'a> {
    fn name(&self) -> &'static str {
        self.into()
    }
}
