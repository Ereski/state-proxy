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

struct HttpProtocol;

impl Protocol for HttpProtocol {
    fn name(&self) -> &str {
        "http"
    }
}

struct HttpClient {
    hyper_client: HyperClient,
}

impl HttpClient {
    fn new() -> Self {
        Self {
            hyper_client: Client::builder().build(HttpsConnector::new()),
        }
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

struct HttpMessageChannel {
    send: Sender<Message>,
    // This needs to be in a `Mutex` because `Receiver::recv` takes a mut receiver, and the
    // `DuplexChannel` trait defines an immutable receiver
    recv: Mutex<Receiver<hyper::Result<Message>>>,
}

impl HttpMessageChannel {
    fn new(hyper_client: HyperClient, addr: SocketAddr) -> Self {
        let (request_send, mut request_recv) = mpsc::channel(1);
        let (response_send, response_recv) = mpsc::channel(1);
        tokio::spawn(async move {
            while let Some(msg) = request_recv.recv().await {
                // TODO: Lots unimplemented here
                let response =
                    hyper_client.request(unimplemented!()).await.map(|x| {
                        let (header, body) = x.into_parts();
                        unimplemented!()
                    });
                if response_send.send(response).await.is_err() {
                    return;
                }
            }
        });

        HttpMessageChannel {
            send: request_send,
            recv: Mutex::new(response_recv),
        }
    }
}

#[async_trait]
impl MessageChannel for HttpMessageChannel {
    async fn send(&self, msg: Message) -> Result<()> {
        match self.send.send(msg).await {
            Ok(_) => Ok(()),
            Err(_) => Err(HttpMessageChannelError::Closed.into()),
        }
    }

    async fn recv(&self) -> Result<Message> {
        match self.recv.lock().await.recv().await {
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

type HyperClient = Client<HttpsConnector<HttpConnector>, Body>;

struct HttpServer;

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

    fn insert_state(&self, state: ServerConnectionState) -> Result<()> {
        unimplemented!()
    }

    fn extract_states(self) -> Receiver<ServerConnectionState> {
        unimplemented!()
    }
}

pub fn register(matchmaker: &mut Matchmaker) -> Result<()> {
    matchmaker.register_client(HttpClient::new())?;
    matchmaker.register_server(HttpServer)?;

    Ok(())
}
