use lazy_static::lazy_static;
use std::{collections::HashMap, net::SocketAddr};
use tokio::{
    net::{TcpStream, UdpSocket},
    sync::Mutex,
};

#[cfg(feature = "http")]
use crate::http::HttpHandle;
#[cfg(feature = "ssh")]
use crate::ssh::SshHandle;
#[cfg(feature = "websockets")]
use crate::websockets::WebsocketHandle;

lazy_static! {
    // TODO: investigate ways to increase parallelism
    static ref G_CONNECTIONS: Mutex<ConnectionMap> = Mutex::new(ConnectionMap::new());
}

pub type ConnectionId = u64;

struct ConnectionMap {
    next_id: ConnectionId,
    connections: HashMap<ConnectionId, Connection>,
}

impl ConnectionMap {
    fn new() -> Self {
        Self {
            next_id: 0,
            connections: HashMap::new(),
        }
    }

    fn register(&mut self, external_connection: ProtocolHandle) -> ConnectionId {
        let id = self.next_id;
        // TODO: handle ConnectionId overflow
        self.next_id = id.checked_add(1).unwrap();
        self.connections
            .insert(id, Connection::new(external_connection));

        id
    }
}

/// Structure holding an external socket and all the state uniquely associated with it.
struct Connection {
    external_connection: ProtocolHandle,
    application_state: Vec<u8>,
    application_sockets: HashMap<String, ProtocolHandle>,
}

impl Connection {
    fn new(external_connection: ProtocolHandle) -> Self {
        Self {
            external_connection,
            application_state: Vec::new(),
            application_sockets: HashMap::new(),
        }
    }
}

#[non_exhaustive]
enum ProtocolHandle {
    RawTcp(TcpStream, SocketAddr),
    RawUdp(UdpSocket),

    #[cfg(feature = "http")]
    Http(HttpHandle, SocketAddr),
    #[cfg(feature = "ssh")]
    Ssh(SshHandle, SocketAddr),
    #[cfg(feature = "websockets")]
    Websocket(WebsocketHandle, SocketAddr),
}

impl From<(TcpStream, SocketAddr)> for ProtocolHandle {
    fn from(x: (TcpStream, SocketAddr)) -> Self {
        Self::RawTcp(x.0, x.1)
    }
}

impl From<UdpSocket> for ProtocolHandle {
    fn from(x: UdpSocket) -> Self {
        Self::RawUdp(x)
    }
}

#[cfg(feature = "http")]
impl From<(HttpHandle, SocketAddr)> for ProtocolHandle {
    fn from(x: (HttpHandle, SocketAddr)) -> Self {
        Self::Http(x.0, x.1)
    }
}

#[cfg(feature = "ssh")]
impl From<(SshHandle, SocketAddr)> for ProtocolHandle {
    fn from(x: (SshHandle, SocketAddr)) -> Self {
        Self::Ssh(x.0, x.1)
    }
}

#[cfg(feature = "websockets")]
impl From<(WebsocketHandle, SocketAddr)> for ProtocolHandle {
    fn from(x: (WebsocketHandle, SocketAddr)) -> Self {
        Self::Websocket(x.0, x.1)
    }
}

pub async fn register_connection<H>(handle: H, remote_addr: SocketAddr) -> ConnectionId
where
    (H, SocketAddr): Into<ProtocolHandle>,
{
    register((handle, remote_addr).into()).await
}

pub async fn register_connectionless<H>(handle: H) -> ConnectionId
where
    H: Into<ProtocolHandle>,
{
    register(handle.into()).await
}

// TODO: consider spawning a new task in case of lock contention
async fn register(handle: ProtocolHandle) -> ConnectionId {
    G_CONNECTIONS.lock().await.register(handle)
}
