use crate::protocol::{MessageChannel, Protocol};
use anyhow::Result;
use std::{net::SocketAddr, sync::Arc};

/// Trait for clients that can talk to servers using a given protocol.
pub trait ProtocolClient {
    /// Get the protocol this client can communicate in.
    fn protocol(&self) -> &Arc<dyn Protocol + Send + Sync>;

    /// Try to connect to the given address.
    fn connect(&self, addr: SocketAddr) -> Result<MessageChannel>;
}
