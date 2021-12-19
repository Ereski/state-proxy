use crate::backend::{
    discovery::{DiscoveryEvent, ServiceDiscovery},
    ServiceManager,
};
use anyhow::{anyhow, Context, Result};
use futures::TryStreamExt;
use http::Uri;
use k8s_openapi::api::core::v1::Pod;
use kube::{
    api::ListParams, runtime, runtime::watcher::Event, Api, Client, Config,
};
use std::{
    collections::{BTreeMap, HashSet},
    mem,
    net::IpAddr,
    sync::Arc,
};
use tokio::{pin, sync::mpsc::Sender};
use tracing::{error, info, warn};
use tuple_transpose::TupleTranspose;

static API_VERSION_LABEL: &str = "state-proxy.io/use";
static API_VERSION: &str = "v0";
static SERVICES_ANNOTATION: &str = "state-proxy.io/services";

pub enum KubernetesConfig {
    Infer,
    Explicit { url: Uri },
}

struct KubernetesServiceDiscovery {
    name: Arc<String>,

    config: KubernetesConfig,
    namespace: String,
}

impl KubernetesServiceDiscovery {
    fn new(config: KubernetesConfig, namespace: Option<String>) -> Self {
        let namespace = namespace.unwrap_or_else(|| "default".to_owned());
        let name = match &config {
            KubernetesConfig::Infer => format!("kubernetes/{}", namespace),
            KubernetesConfig::Explicit { url } => {
                format!("kubernetes/{}/{}", url, namespace)
            }
        };

        Self {
            name: Arc::new(name),

            config,
            namespace,
        }
    }

    async fn run(self, send: Sender<DiscoveryEvent>) -> Result<()> {
        let client = if let KubernetesConfig::Explicit { url } = self.config {
            // URI must have a scheme otherwise the client initialization will panic instead of
            // erroring out. Check that here to avoid user-unfriendly errors
            if url.scheme().is_none() {
                return Err(anyhow!(
                    "Kubernetes cluster URL must have a scheme component"
                ));
            }

            info!(
                "Connecting to the Kubernetes cluster at {}",
                url
            );

            Client::try_from(Config::new(url))
        } else {
            info!("Connecting to the Kubernetes cluster specified by the environment");

            Client::try_default().await
        }
        .with_context(|| {
            format!("while trying to connect to the Kubernetes cluster: {}", self.name)
        })?;

        info!("{}: Watching pods", self.name);
        // TODO: retry on error
        let pods = runtime::watcher(
            Api::<Pod>::namespaced(client, &self.namespace),
            ListParams {
                // We are only interested in pods that have explicitly asked to be proxied with us
                label_selector: Some(format!(
                    "{}={}",
                    API_VERSION_LABEL, API_VERSION
                )),
                ..ListParams::default()
            },
        );
        pin!(pods);
        let mut runtime = KubernetesDiscoveryRuntime::new(&self.name, send);
        while let Some(event) = pods.try_next().await.with_context(|| {
            format!(
                "while watching pods for the Kubernetes cluster: {}",
                self.name
            )
        })? {
            runtime.handle_pod_event(event).await?;
        }

        Err(anyhow!(
            "pod watch stream closed unexpectedly for Kubernetes cluster: {}",
            self.name
        ))
    }
}

impl ServiceDiscovery for KubernetesServiceDiscovery {
    fn name(&self) -> Arc<String> {
        self.name.clone()
    }

    fn run_with_sender(self, send: Sender<DiscoveryEvent>) {
        tokio::spawn(async move {
            if let Err(err) = self.run(send).await {
                error!("{:?}", err);
            }
        });
    }
}

struct KubernetesDiscoveryRuntime<'a> {
    name: &'a Arc<String>,

    send: Sender<DiscoveryEvent>,
    known_pod_uids: HashSet<String>,
}

impl<'a> KubernetesDiscoveryRuntime<'a> {
    fn new(name: &'a Arc<String>, send: Sender<DiscoveryEvent>) -> Self {
        Self {
            name,

            send,
            known_pod_uids: HashSet::new(),
        }
    }

    async fn handle_pod_event(&mut self, event: Event<Pod>) -> Result<()> {
        match event {
            Event::Applied(pod) => {
                if let Some(uid) = &pod.metadata.uid {
                    let is_new = self.known_pod_uids.insert(uid.to_owned());
                    self.apply_pod(pod, is_new).await?;
                }
            }
            Event::Deleted(pod) => {
                if let Some(uid) = pod.metadata.uid {
                    self.known_pod_uids.remove(&uid);
                    self.send(DiscoveryEvent::remove(uid)).await?;
                } else {
                    warn!(
                        "{}: Received a pod deletion event without the pod UID",
                        self.name
                    );
                }
            }
            Event::Restarted(pods) => {
                let mut current_pod_uids = HashSet::new();
                for pod in pods {
                    if let Some(uid) = &pod.metadata.uid {
                        let is_new = self.known_pod_uids.contains(uid);
                        current_pod_uids.insert(uid.to_owned());
                        self.apply_pod(pod, is_new).await?;
                    }
                }

                let old_pod_uids =
                    mem::replace(&mut self.known_pod_uids, current_pod_uids);
                for uid in old_pod_uids {
                    if !self.known_pod_uids.contains(&uid) {
                        self.send(DiscoveryEvent::remove(uid)).await?;
                    }
                }
            }
        }

        Ok(())
    }

    async fn apply_pod(&self, pod: Pod, is_new: bool) -> Result<()> {
        if let Some(status) = pod.status {
            // Take everything we care about out of those endless `Option`s
            let meta = pod.metadata;
            if let Some((uid, name, annotations, address, phase)) = (
                meta.uid,
                meta.name,
                meta.annotations,
                status.pod_ip,
                status.phase,
            )
                .transpose()
            {
                let address = address.parse().with_context(|| {
                    format!(
                        "while parsing the address {} of pod {} ({}) from the Kubernetes cluster: {}",
                        address, name, uid, self.name
                    )
                })?;

                match phase.as_ref() {
                    "Running" => {
                        if is_new {
                            self.discover_services(
                                annotations,
                                uid,
                                &name,
                                address,
                            )
                            .await?;
                        } else {
                            self.send(DiscoveryEvent::resume(uid)).await?;
                        }
                    }
                    "Succeeded" => {
                        self.send(DiscoveryEvent::remove(uid)).await?;
                    }
                    "Failed" => {
                        self.send(DiscoveryEvent::suspend(uid)).await?;
                    }
                    "Unknown" => {
                        warn!(
                            "{}: pod {} ({}) is in an unknown state. Suspending pod until the situation resolves",
                            self.name, name, uid
                        );
                        self.send(DiscoveryEvent::suspend(uid)).await?;
                    }
                    // If the pod is only pending, we don't care
                    "Pending" => (),
                    unknown => {
                        warn!(
                            "{}: received an unknown pod status from Kubernetes for pod {} ({}): {}. Suspending pod until the situation resolves",
                            self.name, name, uid, unknown
                        );
                        self.send(DiscoveryEvent::suspend(uid)).await?;
                    }
                }
            }
        }

        Ok(())
    }

    async fn discover_services(
        &self,
        annotations: BTreeMap<String, String>,
        uid: String,
        pod_name: &str,
        pod_address: IpAddr,
    ) -> Result<()> {
        if let Some(services_string) = annotations.get(SERVICES_ANNOTATION) {
            for service_string in services_string.split(',') {
                self.send_service_from_string(
                    service_string,
                    uid.clone(),
                    pod_name,
                    pod_address,
                ).await.with_context(|| format!("while handling service string '{}' for pod {} ({}) from Kubernetes cluster: {}", service_string, pod_name, uid, self.name))?;
            }
        }

        Ok(())
    }

    // The format is very simple:
    //
    // <external-protocol>:<external-port>:<backend-protocol>:<backend-port>
    //
    // `<external-*>` refers to what the proxy accepts from the outside world, and `<backend-*>`
    // refer to how the proxy communicates with the pod
    async fn send_service_from_string(
        &self,
        service_string: &str,
        uid: String,
        pod_name: &str,
        pod_address: IpAddr,
    ) -> Result<()> {
        if service_string.split(':').count() > 4 {
            warn!(
                "{}: service definition '{}' for pod {} ({}) has garbage at the end",
                self.name, pod_name, uid, service_string
            );
        }

        let mut parts = service_string.split(':');
        let parts = (parts.next(), parts.next(), parts.next(), parts.next())
            .transpose();

        match parts {
            Some((
                external_port,
                external_protocol,
                backend_port,
                backend_protocol,
            )) => {
                self.send(DiscoveryEvent::add(
                    uid,
                    true,
                    parse_port(external_port)?,
                    external_protocol,
                    (pod_address, parse_port(backend_port)?),
                    backend_protocol,
                ))
                .await?;

                Ok(())
            }
            _ => Err(anyhow!("invalid service string")),
        }
    }

    async fn send(&self, event: DiscoveryEvent) -> Result<()> {
        match self.send.send(event).await {
            Ok(_) => Ok(()),
            Err(_) => Err(anyhow!("channel closed")),
        }
    }
}

pub async fn register(
    manager: &Arc<ServiceManager>,
    config: KubernetesConfig,
    namespace: Option<String>,
) -> Result<()> {
    manager
        .register_service_discovery(KubernetesServiceDiscovery::new(
            config, namespace,
        ))
        .await
}

fn parse_port(port: &str) -> Result<u16> {
    port.parse()
        .with_context(|| format!("'{}' is not a port number", port))
}
