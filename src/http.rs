use crate::protocol::{Connection, Message, Protocol, ProtocolServer};
use async_trait::async_trait;
use hyper::{client::HttpConnector, Body, Client};
use hyper_tls::HttpsConnector;
use lazy_static::lazy_static;
use std::{net::SocketAddr, sync::Arc};

lazy_static! {
    pub static ref HTTP_PROTOCOL: Arc<dyn Protocol> =
        Arc::new(HttpProtocol::new());
}

struct HttpProtocol {
    name: Arc<String>,
}

impl HttpProtocol {
    fn new() -> Self {
        Self {
            name: Arc::new("http".to_owned()),
        }
    }
}

#[async_trait]
impl Protocol for HttpProtocol {
    fn name(&self) -> &Arc<String> {
        &self.name
    }

    async fn new_client(
        &self,
        address: &str,
    ) -> anyhow::Result<Box<dyn Connection>> {
        unimplemented!()
    }

    async fn new_server(
        &self,
        address: SocketAddr,
    ) -> anyhow::Result<Box<dyn ProtocolServer>> {
        unimplemented!()
    }
}

struct HttpClient {
    protocol: Arc<dyn Protocol>,

    hyper_client: Client<HttpsConnector<HttpConnector>, Body>,
}

impl HttpClient {
    fn new(protocol: Arc<dyn Protocol>) -> Self {
        Self {
            protocol,

            hyper_client: Client::builder().build(HttpsConnector::new()),
        }
    }
}

#[async_trait]
impl Connection for HttpClient {
    fn protocol(&self) -> &Arc<dyn Protocol> {
        &self.protocol
    }

    async fn send(&mut self, message: Message) -> anyhow::Result<()> {
        unimplemented!()
    }

    async fn receive(&mut self) -> anyhow::Result<Message> {
        unimplemented!()
    }
}

#[derive(Default)]
struct HttpServer;

impl HttpServer {
    fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl ProtocolServer for HttpServer {
    async fn next(&mut self) -> anyhow::Result<Box<dyn Connection>> {
        unimplemented!()
    }
}
