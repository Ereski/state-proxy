use crate::protocol::{MessageChannel, Protocol, SocketStream};
use anyhow::Result;
use std::net::SocketAddr;
use tokio::sync::mpsc::{Receiver, Sender};

/// Trait for servers that can accept external connections and move those connections between
/// different instances seamlessly.
pub trait ProtocolServer {
    /// Get the protocol this server can communicate in.
    fn protocol(&self) -> &dyn Protocol;

    /// Start listening on the given `bind_to` address. For every new connection, a request must
    /// be sent into the `new_connection_sender` which will be routed to a backend server. The
    /// backend server will then decide how to reply to that connection request.
    ///
    /// The implementation should run the actual server through [`tokio::spawn`] or
    /// [`tokio::task::spawn_blocking`] so as to be able to communicate using async channels.
    fn listen(
        &self,
        bind_to: SocketAddr,
        new_connection_sender: Sender<NewConnectionRequest>,
    );

    /// Stop listening on the given port and release all resources associated with it,
    /// including open connections.
    fn mute(&self, port: u16);

    /// Insert the given connection and its state into the server. If successful, the server
    /// should continue communicating through this socket stream as if it was created by it.
    fn insert_state(&self, state: ServerConnectionState) -> Result<()>;

    /// Start extracting all external connections and their internal state from the server.
    ///
    /// As soon as this method is called, the server should stop accepting new connections but it
    /// should still process existing connections as normal. And then for each existing
    /// connection, it must:
    ///
    /// 1. Pause processing and flush the output buffer for that connection. The socket stream
    ///    must not be closed.
    /// 2. Move the socket stream and its associated state into a `ServerConnectionState` struct.
    /// 3. Send that struct through the channel and cleanup any remaining resources associated
    ///    with that connection.
    fn extract_states(self) -> Receiver<ServerConnectionState>;
}

pub struct NewConnectionRequest {
    from: SocketAddr,
    channel: MessageChannel,
}

/// Frozen server state for a single connection.
pub struct ServerConnectionState {
    /// The socket stream through which the client and the server were communicating.
    stream: SocketStream,

    /// Remaining data in the read buffer.
    read_buf: Vec<u8>,

    /// Any other internal data that is required for another instance to continue processing the
    /// connection seamlessly. This data should be as backwards- and forwards-compatible as
    /// possible to reduce the chance that an upgrade or downgrade will require a reconnection.
    /// Note that this does not apply to changes that impact security.
    internal: Vec<u8>,
}
