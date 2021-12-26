use crate::protocol::MessageChannel;
use serde_derive::{Deserialize, Serialize};
use std::{net::SocketAddr, result, sync::Arc};
use strum_macros::IntoStaticStr;
use thiserror::Error;

const PROXY_PROTOCOL_VERSION: u64 = 0;

#[derive(Debug, Error)]
pub enum Error {
    #[error("cannot deserialize message: {0}")]
    BadMessage(#[from] serde_json::Error),

    #[error("received an unknown channel ID {channel_id} for message: {message_name}")]
    UnknownChannelId {
        channel_id: u64,
        message_name: &'static str,
    },

    #[error("received a {received} message but expected one of: {expected:?}")]
    WrongMessage {
        expected: Vec<&'static str>,
        received: &'static str,
    },

    #[error("expected a Hello message with version {expected}, received version {received}")]
    WrongVersion { expected: u64, received: u64 },
}

pub type Result<T> = result::Result<T, Error>;

pub struct ProxyProtocolServer {
    message_channel: MessageChannel,
}

impl ProxyProtocolServer {
    pub fn new(message_channel: MessageChannel) -> Self {
        Self { message_channel }
    }

    pub async fn run(self) -> Result<()> {
        unimplemented!()
    }
}

#[derive(IntoStaticStr, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
enum ServerMessageV0 {
    Hello {
        version: u64,
        client_protocols: Arc<Vec<String>>,
    },
    NewChannel {
        channel_id: Option<u64>,
        from: SocketAddr,
    },
    CloseChannel {
        channel_id: Option<u64>,
    },
    DataStart {
        channel_id: Option<u64>,
        size: Option<u64>,
    },
    DataChunk {
        channel_id: Option<u64>,
        size: Option<u64>,
    },
    DataEnd {
        channel_id: Option<u64>,
    },
    SocketOpened {
        channel_id: Option<u64>,
        socket_id: Option<u64>,
    },
    SocketData {
        channel_id: Option<u64>,
        socket_id: Option<u64>,
    },
    SocketClosed {
        channel_id: Option<u64>,
        socket_id: Option<u64>,
    },
}

impl ServerMessageV0 {
    fn name(&self) -> &'static str {
        self.into()
    }
}

#[derive(IntoStaticStr, serde_derive::Deserialize, serde_derive::Serialize)]
#[serde(deny_unknown_fields)]
enum ClientMessageV0<'a> {
    Hello {
        version: u64,
    },
    DataStart {
        channel_id: Option<u64>,
        size: Option<u64>,
        state: Option<&'a [u8]>,
    },
    DataChunk {
        channel_id: Option<u64>,
        size: Option<u64>,
        state: Option<&'a [u8]>,
    },
    DataEnd {
        channel_id: Option<u64>,
        state: Option<&'a [u8]>,
    },
    SetState {
        channel_id: Option<u64>,
        state: Option<&'a [u8]>,
    },
    OpenSocket {
        channel_id: Option<u64>,
        socket_id: Option<u64>,
        protocol: &'a str,
        address: &'a str,
    },
    SocketData {
        channel_id: Option<u64>,
        socket_id: Option<u64>,
    },
    CloseSocket {
        channel_id: Option<u64>,
        socket_id: Option<u64>,
    },
}

impl<'a> ClientMessageV0<'a> {
    fn name(&self) -> &'static str {
        self.into()
    }
}
