use crate::matchmaking::{DuplexChannel, Message, Protocol};
use async_trait::async_trait;
use std::{error, net::SocketAddr, result};

// Trait for clients that can talk to servers using a given protocol.
//
// Use [`async_trait`] when implementing this trait.
#[async_trait]
pub trait ProtocolClient<P>
where
    P: Protocol,
{
    // Error type for this protocol client.
    type Error: error::Error + Send + Sync + 'static;

    // Type of the channel this client creates on a successful [`connect`].
    type Channel: DuplexChannel<Message> + 'static;

    // Try to connect to the given address.
    async fn connect(&self, addr: SocketAddr) -> result::Result<Self::Channel, Self::Error>;
}
