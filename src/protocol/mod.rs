use anyhow::Result;
use bytes::Bytes;
use std::{
    net::{TcpStream, UdpSocket},
    num::NonZeroU32,
    sync::Arc,
};
use tokio::sync::{
    mpsc::{self, Receiver, Sender},
    Mutex,
};

#[cfg(unix)]
use std::os::unix::net::UnixStream;

pub mod proxy;

// TODO: `ProtocolManager`

// Two mpsc channels are created with this size per `MessageChannel` pair, and a `MessageChannel`
// pair is created per external and internal connection. So it can't be too large, and a large
// buffer is not very useful anyway
const MESSAGE_CHANNEL_BUFFER_SIZE: usize = 1;

/// Description of a single network protocol.
pub trait Protocol {
    /// Return the lowercase name of the protocol. A version number must be included if there are
    /// multiple incompatible versions (e.g. SSH1 vs SSH2).
    fn name(&self) -> &str;

    /// Translate a message from another protocol.
    fn translate(&self, message: Message) -> Result<Message>;

    /// Get the capabilities for this protocol.
    fn capabilities(&self) -> ProtocolCapabilities {
        ProtocolCapabilities::default()
    }

    /// Craft a [`Message`] asking for a new multiplexed channel to be opened.
    fn new_channel(&self) -> Message {
        panic!("Protocol '{}' does not support multiplexing", self.name())
    }
}

/// Protocol capabilities besides transmitting data.
pub struct ProtocolCapabilities {
    /// The maximum number of multiplexed channels in a single connection for this protocol.
    multiplexing: NonZeroU32,

    /// Whether this protocol is capable of sending and receiving sockets.
    sockets: bool,
}

impl Default for ProtocolCapabilities {
    fn default() -> Self {
        Self {
            multiplexing: unsafe { NonZeroU32::new_unchecked(1) },
            sockets: false,
        }
    }
}

pub trait AncillaryData {
    /// The ID of the channel if the protocol supports multiplexing.
    fn channel_id(&self) -> Option<u32> {
        None
    }

    /// Length of of the [`Message::data`] field this object is attached to.
    fn data_length(&self) -> Option<u64> {
        None
    }

    /// Sockets associated with this message, if the protocol supports the transmission of
    /// sockets.
    fn sockets(&self) -> &[SocketStream] {
        &[]
    }
}

/// A channel that permits bidirectional, message-based communication.
///
/// [`MessageChannel`]s can be freely cloned.
#[derive(Clone)]
pub struct MessageChannel {
    protocol: Arc<dyn Protocol + Send + Sync>,
    sender: Sender<Message>,
    receiver: Arc<Mutex<Receiver<Message>>>,
}

impl MessageChannel {
    /// Create a connected [`MessageChannel`] pair.
    pub fn create(protocol: Arc<dyn Protocol + Send + Sync>) -> (Self, Self) {
        let (sender_a, receiver_a) = mpsc::channel(MESSAGE_CHANNEL_BUFFER_SIZE);
        let (sender_b, receiver_b) = mpsc::channel(MESSAGE_CHANNEL_BUFFER_SIZE);

        (
            Self {
                protocol: protocol.clone(),
                sender: sender_a,
                receiver: Arc::new(Mutex::new(receiver_b)),
            },
            Self {
                protocol,
                sender: sender_b,
                receiver: Arc::new(Mutex::new(receiver_a)),
            },
        )
    }

    /// Get the underlying protocol that this [`MessageChannel`] represents.
    pub fn protocol(&self) -> &Arc<dyn Protocol + Send + Sync> {
        &self.protocol
    }

    /// Try to send a message through the channel.
    #[must_use]
    pub async fn send(&self, message: Message) -> Option<Message> {
        match self.sender.send(message).await {
            Ok(()) => None,
            Err(err) => Some(err.0),
        }
    }

    /// Try to receive a message through the channel.
    #[must_use]
    pub async fn recv(&self) -> Option<Message> {
        self.receiver.lock().await.recv().await
    }
}

#[derive(Default)]
pub struct Message {
    pub data: Option<Bytes>,
    pub ancillary: Option<Box<dyn AncillaryData + Send + Sync>>,
}

#[non_exhaustive]
pub enum SocketStream {
    Tcp(TcpStream),
    Udp(UdpSocket),

    #[cfg(unix)]
    Uds(UnixStream),
}
