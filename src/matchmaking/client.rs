use crate::{matchmaking::MessageChannel, protocol::Protocol};
use anyhow::Result;
use async_trait::async_trait;
use std::net::SocketAddr;

/// Trait for clients that can talk to servers using a given protocol.
///
/// Use [`macro@async_trait`] to implement this trait.
#[async_trait]
pub trait ProtocolClient {
    /// Get the protocol this client can communicate in.
    fn protocol(&self) -> &dyn Protocol;

    /// Try to connect to the given address.
    async fn connect(
        &self,
        addr: SocketAddr,
    ) -> Result<Box<dyn MessageChannel + Send>>;
}
