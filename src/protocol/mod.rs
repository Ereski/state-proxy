use anyhow::Result;
use bytes::Bytes;
use std::{
    net::{TcpStream, UdpSocket},
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

    /// Translate ancillary data from another protocol.
    fn translate_ancillary(
        &self,
        ancillary: Box<dyn AncillaryData>,
    ) -> Result<Box<dyn AncillaryData>>;

    /// Get the capabilities for this protocol.
    fn capabilities(&self) -> ProtocolCapabilities {
        ProtocolCapabilities::default()
    }
}

/// Protocol capabilities besides transmitting data.
#[derive(Default)]
pub struct ProtocolCapabilities {
    multiplexing: bool,
    sockets: bool,
}

pub trait AncillaryData {
    /// The protocol that defines this [`AncillaryData`].
    fn protocol(&self) -> &dyn Protocol;

    /// The ID of the channel if the protocol supports multiplexing.
    fn channel_id(&self) -> Option<u64> {
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

/// A channel that permit bidirectional, message-based communication.
///
/// [`MessageChannel`]s can be freely cloned.
#[derive(Clone)]
pub struct MessageChannel {
    sender: Sender<Message>,
    receiver: Arc<Mutex<Receiver<Message>>>,
}

impl MessageChannel {
    /// Create a connected [`MessageChannel`] pair.
    pub fn create() -> (Self, Self) {
        let (sender_a, receiver_a) = mpsc::channel(MESSAGE_CHANNEL_BUFFER_SIZE);
        let (sender_b, receiver_b) = mpsc::channel(MESSAGE_CHANNEL_BUFFER_SIZE);

        (
            Self {
                sender: sender_a,
                receiver: Arc::new(Mutex::new(receiver_b)),
            },
            Self {
                sender: sender_b,
                receiver: Arc::new(Mutex::new(receiver_a)),
            },
        )
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
