use crate::{
    matchmaking::{
        client::ProtocolClient,
        server::{
            NewConnectionReply, NewConnectionRequest, ProtocolServer,
            ServerConnectionState, SocketStream,
        },
        Matchmaker, Message, MessageChannel,
    },
    protocol::Protocol,
};
use anyhow::Result;
use async_trait::async_trait;
use hyper::{
    client::HttpConnector, service, Body, Client, Request, Response, Server,
};
use hyper_tls::HttpsConnector;
use std::net::SocketAddr;
use thiserror::Error;
use tokio::sync::{
    mpsc::{self, Receiver, Sender},
    Mutex,
};

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

#[async_trait]
impl ProtocolClient for HttpClient {
    fn protocol(&self) -> &dyn Protocol {
        &HttpProtocol
    }

    async fn connect(
        &self,
        addr: SocketAddr,
    ) -> Result<Box<dyn MessageChannel + Send>> {
        Ok(Box::new(HttpMessageChannel::new(
            self.hyper_client.clone(),
            addr,
        )))
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
    fn protocol(&self) -> &dyn Protocol {
        &HttpProtocol
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

struct HttpMessageChannel {
    sender: Sender<Message>,
    // This needs to be in a `Mutex` because `Receiver::recv` takes a mut receiver, and the
    // `DuplexChannel` trait defines an immutable receiver
    receiver: Mutex<Receiver<hyper::Result<Message>>>,
}

impl HttpMessageChannel {
    fn new(hyper_client: HyperClient, addr: SocketAddr) -> Self {
        let (request_sender, mut request_receiver) = mpsc::channel(1);
        let (response_sender, response_receiver) = mpsc::channel(1);
        tokio::spawn(async move {
            while let Some(msg) = request_receiver.recv().await {
                // TODO: Lots unimplemented here
                let response =
                    hyper_client.request(unimplemented!()).await.map(|x| {
                        let (header, body) = x.into_parts();
                        unimplemented!()
                    });
                if response_sender.send(response).await.is_err() {
                    return;
                }
            }
        });

        HttpMessageChannel {
            sender: request_sender,
            receiver: Mutex::new(response_receiver),
        }
    }
}

#[async_trait]
impl MessageChannel for HttpMessageChannel {
    async fn send(&self, msg: Message) -> Result<()> {
        match self.sender.send(msg).await {
            Ok(_) => Ok(()),
            Err(_) => Err(HttpMessageChannelError::Closed.into()),
        }
    }

    async fn recv(&self) -> Result<Message> {
        match self.receiver.lock().await.recv().await {
            Some(res) => res.map_err(|err| err.into()),
            None => Err(HttpMessageChannelError::Closed.into()),
        }
    }
}

#[derive(Debug, Error)]
enum HttpMessageChannelError {
    #[error("hyper error: {0}")]
    Hyper(#[from] hyper::Error),

    #[error("HTTP channel closed")]
    Closed,
}

struct HttpProtocol;

impl Protocol for HttpProtocol {
    fn name(&self) -> &str {
        "http"
    }
}
