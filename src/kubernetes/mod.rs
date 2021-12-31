use crate::backend::endpoint::{EndpointDiscovery, EndpointEvent};
use anyhow::{anyhow, Context, Result};
use futures::TryStreamExt;
use http::Uri;
use k8s_openapi::api::core::v1::Pod;
use kube::{
    api::ListParams, config::KubeConfigOptions, runtime,
    runtime::watcher::Event, Api, Client, Config,
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

#[cfg(feature = "benchmark")]
#[doc(hidden)]
pub mod benchmark;

/// The label pods should use to declare their interest in being proxied, and the format version
/// of their `state-proxy.io` annotations.
pub static FORMAT_VERSION_LABEL: &str = "state-proxy.io/use";

/// The current annotation format version.
pub static FORMAT_VERSION: &str = "v0";

/// The annotation used to define which services should be proxied for this pod.
pub static SERVICES_ANNOTATION: &str = "state-proxy.io/services";

/// Kubernetes configuration.
pub enum KubernetesConfig {
    /// Use kubeconfig with the given context, or `default`.
    KubeConfig { context: Option<String> },

    /// Connect to a kubernetes cluster at the given URL.
    Explicit { url: Uri },
}

/// A [`EndpointDiscovery`] for Kubernetes.
pub struct KubernetesEndpointDiscovery {
    name: Arc<String>,

    config: Config,
    namespace: String,
}

impl KubernetesEndpointDiscovery {
    pub async fn new(
        config: KubernetesConfig,
        namespace: Option<String>,
    ) -> Result<Self> {
        let namespace = namespace.unwrap_or_else(|| "default".to_owned());
        let config = match config {
            KubernetesConfig::KubeConfig { context } => {
                Config::from_kubeconfig(&KubeConfigOptions {
                    context,
                    ..Default::default()
                })
                .await
                .map_err(|x| x.into())
            },
            KubernetesConfig::Explicit { url } => {
                // URI must have a scheme otherwise the client initialization will panic instead of
                // erroring out. Check that here to avoid user-unfriendly errors
                if url.scheme().is_some() {
                    Ok(Config::new(url))
                } else {
                    Err(anyhow!(
                        "Kubernetes cluster URL must have a scheme component"
                    ))
                }
            }
        }.context("while trying to get the connection parameters to the Kubernetes cluster")?;
        let name = format!("kubernetes@{}/{}", config.cluster_url, namespace);

        Ok(Self {
            name: Arc::new(name),

            config,
            namespace,
        })
    }

    async fn run(self, sender: Sender<EndpointEvent>) -> Result<()> {
        info!(
            "Connecting to the Kubernetes cluster at {}",
            self.config.cluster_url
        );
        let client = Client::try_from(self.config).with_context(|| {
            format!(
                "while trying to connect to the Kubernetes cluster: {}",
                self.name
            )
        })?;

        info!("{}: Watching pods", self.name);
        // TODO: retry on error
        let pods = runtime::watcher(
            Api::<Pod>::namespaced(client, &self.namespace),
            ListParams {
                // We are only interested in pods that have explicitly asked to be proxied with us
                label_selector: Some(format!(
                    "{}={}",
                    FORMAT_VERSION_LABEL, FORMAT_VERSION
                )),
                ..ListParams::default()
            },
        );
        pin!(pods);
        let mut runtime = KubernetesDiscoveryRuntime::new(&self.name, sender);
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

impl EndpointDiscovery for KubernetesEndpointDiscovery {
    fn name(&self) -> Arc<String> {
        self.name.clone()
    }

    fn run_with_sender(self, sender: Sender<EndpointEvent>) {
        tokio::spawn(async move {
            if let Err(err) = self.run(sender).await {
                error!("{:?}", err);
            }
        });
    }
}

struct KubernetesDiscoveryRuntime<'a> {
    name: &'a Arc<String>,

    sender: Sender<EndpointEvent>,
    known_pod_uids: HashSet<String>,
}

impl<'a> KubernetesDiscoveryRuntime<'a> {
    fn new(name: &'a Arc<String>, sender: Sender<EndpointEvent>) -> Self {
        Self {
            name,

            sender,
            known_pod_uids: HashSet::new(),
        }
    }

    async fn handle_pod_event(&mut self, event: Event<Pod>) -> Result<()> {
        match event {
            Event::Applied(pod) => {
                if pod.metadata.uid.is_some() {
                    self.apply_pod(pod).await?;
                }
            }
            Event::Deleted(pod) => {
                if let Some(uid) = pod.metadata.uid {
                    self.known_pod_uids.remove(&uid);
                    self.send(EndpointEvent::delete(uid)).await?;
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
                        current_pod_uids.insert(uid.to_owned());
                        self.apply_pod(pod).await?;
                    }
                }

                let old_pod_uids =
                    mem::replace(&mut self.known_pod_uids, current_pod_uids);
                for uid in old_pod_uids {
                    if !self.known_pod_uids.contains(&uid) {
                        self.send(EndpointEvent::delete(uid)).await?;
                    }
                }
            }
        }

        Ok(())
    }

    async fn apply_pod(&mut self, pod: Pod) -> Result<()> {
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

                if self.known_pod_uids.insert(uid.to_owned()) {
                    self.discover_services(
                        annotations,
                        uid.clone(),
                        &name,
                        address,
                    )
                    .await?;
                }

                match phase.as_ref() {
                    "Running" => {
                        self.send(EndpointEvent::resume(uid)).await?;
                    }
                    "Succeeded" => {
                        self.known_pod_uids.remove(&uid);
                        self.send(EndpointEvent::delete(uid)).await?;
                    }
                    "Failed" => {
                        self.send(EndpointEvent::suspend(uid)).await?;
                    }
                    "Unknown" => {
                        warn!(
                            "{}: pod {} ({}) is in an unknown state. Suspending pod until the situation resolves",
                            self.name, name, uid
                        );
                        self.send(EndpointEvent::suspend(uid)).await?;
                    }
                    // If the pod is only pending, we don't care
                    "Pending" => (),
                    unknown => {
                        warn!(
                            "{}: received an unknown pod status from Kubernetes for pod {} ({}): {}. Suspending pod until the situation resolves",
                            self.name, name, uid, unknown
                        );
                        self.send(EndpointEvent::suspend(uid)).await?;
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
        let mut has_service = false;
        if let Some(services_string) = annotations.get(SERVICES_ANNOTATION) {
            for service_string in services_string.split(',') {
                has_service = true;

                self.send_service_from_string(
                    service_string,
                    uid.clone(),
                    pod_name,
                    pod_address,
                ).await.with_context(|| {
                    format!(
                        "while handling service string '{}' for pod {} ({}) from Kubernetes cluster: {}", service_string, pod_name, uid, self.name
                    )
                })?;
            }
        }

        if !has_service {
            warn!("pod {} ({}) does not define any services", pod_name, uid);
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
                self.send(EndpointEvent::add(
                    uid,
                    false,
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

    async fn send(&self, event: EndpointEvent) -> Result<()> {
        match self.sender.send(event).await {
            Ok(_) => Ok(()),
            Err(_) => Err(anyhow!("channel closed")),
        }
    }
}

fn parse_port(port: &str) -> Result<u16> {
    port.parse()
        .with_context(|| format!("'{}' is not a port number", port))
}
