use crate::{
    matchmaking::{
        client::ProtocolClient,
        server::{NewConnectionRequest, ProtocolServer, ServerConnectionState},
    },
    protocol::{
        AncillaryData, Message, MessageChannel, Protocol, SocketStream,
    },
};
use anyhow::Result;
use hyper::{
    client::HttpConnector, service, Body, Client, Request, Response, Server,
};
use hyper_tls::HttpsConnector;
use lazy_static::lazy_static;
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::mpsc::{Receiver, Sender};

lazy_static! {
    static ref HTTP_PROTOCOL: Arc<dyn Protocol + Send + Sync> =
        Arc::new(HttpProtocol);
}

struct HttpProtocol;

impl Protocol for HttpProtocol {
    fn name(&self) -> &str {
        "http"
    }

    fn translate(&self, message: Message) -> Result<Message> {
        unimplemented!()
    }
}

struct HttpAncillaryData {}

impl AncillaryData for HttpAncillaryData {}

pub struct HttpClient {
    hyper_client: HyperClient,
}

impl HttpClient {
    pub fn new() -> Self {
        Self {
            hyper_client: Client::builder().build(HttpsConnector::new()),
        }
    }
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ProtocolClient for HttpClient {
    fn protocol(&self) -> &Arc<dyn Protocol + Send + Sync> {
        &HTTP_PROTOCOL
    }

    fn connect(&self, addr: SocketAddr) -> Result<MessageChannel> {
        let (message_channel, message_channel2) =
            MessageChannel::create(HTTP_PROTOCOL.clone());
        let hyper_client = self.hyper_client.clone();
        tokio::spawn(async move {
            while let Some(message) = message_channel.recv().await {
                unimplemented!()
            }
        });

        Ok(message_channel2)
    }
}

type HyperClient = Client<HttpsConnector<HttpConnector>, Body>;

#[derive(Default)]
pub struct HttpServer;

impl HttpServer {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ProtocolServer for HttpServer {
    fn protocol(&self) -> &Arc<dyn Protocol + Send + Sync> {
        &HTTP_PROTOCOL
    }

    fn listen(
        &self,
        bind_to: SocketAddr,
        new_connection_send: Sender<NewConnectionRequest>,
    ) {
        tokio::spawn(Server::bind(&bind_to).serve(service::make_service_fn(
            |_| async move {
                Ok::<_, hyper::Error>(service::service_fn(
                    |request| async move {
                        Ok::<_, hyper::Error>(Response::new(Body::from(
                            "UNIMPLEMENTED",
                        )))
                    },
                ))
            },
        )));
    }

    fn mute(&self, port: u16) {
        unimplemented!()
    }

    fn insert_state(&self, state: ServerConnectionState) -> Result<()> {
        unimplemented!()
    }

    fn extract_states(self) -> Receiver<ServerConnectionState> {
        unimplemented!()
    }
}
