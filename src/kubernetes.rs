use anyhow::{anyhow, Context, Result};
use futures::TryStreamExt;
use k8s_openapi::api::core::v1::Pod;
use kube::{api::ListParams, runtime, runtime::watcher::Event, Api, Client, Config};
use std::{collections::BTreeMap, net::IpAddr};
use tokio::pin;
use tracing::{error, info, warn};
use tuple_transpose::TupleTranspose;

static API_VERSION_LABEL: &str = "state-proxy.io/use";
static API_VERSION: &str = "v0";
static SERVICES_ANNOTATION: &str = "state-proxy.io/services";

pub enum K8sConfig {
    Infer,
    Explicit(Config),
}

impl ProxiedPodMap {
    fn new() -> Self {
        Self::default()
    }

    fn clear(&mut self) {
        self.map.clear();
    }

    fn register(&mut self, id: ProxiedPodId, proxied: ProxiedPod) {
        if self.map.insert(id.clone(), proxied).is_some() {
            info!("Updated pod {} in the list of proxied pods", id);
        } else {
            info!("Added pod {} to the list of proxied pods", id);
        }
    }

    fn delete(&mut self, id: ProxiedPodId) {
        if self.map.remove(&id).is_some() {
            info!("Removed pod {} from the list of proxied pods", id);
        }
    }
}

enum ProxiedPodMapAction {
    Register {
        id: ProxiedPodId,
        proxied: ProxiedPod,
    },
    Delete {
        id: ProxiedPodId,
    },
}

impl ProxiedPodMapAction {
    fn register(id: ProxiedPodId, proxied: ProxiedPod) -> Self {
        Self::Register { id, proxied }
    }

    fn delete(id: ProxiedPodId) -> Self {
        Self::Delete { id }
    }

    fn execute(self, proxied_map: &mut ProxiedPodMap) {
        match self {
            Self::Register { id, proxied } => proxied_map.register(id, proxied),
            Self::Delete { id } => proxied_map.delete(id),
        }
    }
}

struct ProxiedPod {
    name: String,
    addr: IpAddr,
}

impl ProxiedPod {
    fn new(name: String, addr: IpAddr, services: Vec<ProxiedService>) -> Self {
        unimplemented!()
    }
}

struct ProxiedService {}

pub async fn run_communication(config: K8sConfig) {
    if let Err(err) = do_run(config).await {
        error!("{:?}", err);
    }
}

async fn do_run(config: K8sConfig) -> Result<()> {
    // URI must have a scheme otherwise client initialization will panic instead of erroring out.
    // Check that here to avoid user-unfriendly errors
    if let K8sConfig::Explicit(Config { cluster_url, .. }) = &config {
        if cluster_url.scheme().is_none() {
            return Err(anyhow!(
                "Kubernetes cluster URL must have a scheme component"
            ));
        }
    }

    let client = if let K8sConfig::Explicit(config) = config {
        info!(
            "Connecting to the Kubernetes cluster at {}",
            config.cluster_url
        );

        Client::try_from(config)
    } else {
        info!("Connecting to the Kubernetes cluster specified by the environment");

        Client::try_default().await
    }
    .context("while trying to connect to the Kubernetes cluster")?;

    info!("Watching pods from the Kubernetes cluster");
    // TODO: implement retry on error
    let pods = runtime::watcher(
        Api::<Pod>::default_namespaced(client),
        ListParams {
            // We are only interested in pods that have explicitly asked to be proxied with us
            label_selector: Some(format!("{}={}", API_VERSION_LABEL, API_VERSION)),
            ..ListParams::default()
        },
    );
    pin!(pods);
    while let Some(event) = pods.try_next().await.context("while watching pods")? {
        handle_pod_event(event).await?;
    }

    Err(anyhow!("pod watch stream closed unexpectedly"))
}

async fn handle_pod_event(event: Event<Pod>) -> Result<()> {
    match event {
        Event::Applied(pod) => {
            if let Some(action) = parse_useful_pod(pod)? {
                action.execute(&mut *G_PROXIED_PODS.lock().await);
            }
        }
        Event::Deleted(pod) => {
            if let Some(uid) = pod.metadata.uid {
                G_PROXIED_PODS.lock().await.delete(uid);
            } else {
                warn!("Received a pod deletion event without the pod UID");
            }
        }
        Event::Restarted(all_pods) => {
            let mut proxied_map = G_PROXIED_PODS.lock().await;
            proxied_map.clear();
            for pod in all_pods {
                if let Some(action) = parse_useful_pod(pod)? {
                    action.execute(&mut proxied_map);
                }
            }
        }
    }

    Ok(())
}

// A "useful" pod is one that is fully defined and is either in the `Succeeded` or `Failed`
// phases. We don't care about the other pods
fn parse_useful_pod(pod: Pod) -> Result<Option<ProxiedPodMapAction>> {
    if let Some(status) = pod.status {
        // Take everything we care about out of those endless `Option`s
        let meta = pod.metadata;
        if let Some((uid, name, annotations, addr, phase)) = (
            meta.uid,
            meta.name,
            meta.annotations,
            status.pod_ip,
            status.phase,
        )
            .transpose()
        {
            let addr = addr.parse().with_context(|| {
                format!(
                    "while parsing the address {} of pod {} ({})",
                    addr, name, uid
                )
            })?;
            match phase.as_ref() {
                "Running" => {
                    return Ok(Some(ProxiedPodMapAction::register(
                        uid.clone(),
                        ProxiedPod::new(
                            name.clone(),
                            addr,
                            services_from_annotations(annotations).with_context(|| {
                                format!("while parsing services for pod {} ({})", name, uid)
                            })?,
                        ),
                    )))
                }
                "Failed" => return Ok(Some(ProxiedPodMapAction::delete(uid))),
                _ => (),
            }
        }
    }

    Ok(None)
}

fn services_from_annotations(annotations: BTreeMap<String, String>) -> Result<Vec<ProxiedService>> {
    let mut services = Vec::new();
    if let Some(services_string) = annotations.get(SERVICES_ANNOTATION) {
        for service_string in services_string.split(',') {
            services.push(parse_service_string(service_string)?);
        }
    }

    if services.is_empty() {
        Err(anyhow!("no services were defined"))
    } else {
        Ok(services)
    }
}

// The format is very simple: `<external-port>:<external-protocol>:<server-port>:<server-protocol>`
// `<external-*>` refers to what the proxy accepts from the outside world, and `<server-*>`
// refer to how the proxy communicates with the pod
fn parse_service_string(service_string: &str) -> Result<ProxiedService> {
    let mut split = service_string.split(':');
    let parts = (split.next(), split.next(), split.next(), split.next()).transpose();

    match parts {
        Some((external_port, external_protocol, server_port, server_protocol))
            if split.next().is_none() =>
        {
            unimplemented!()
        }
        _ => Err(anyhow!("service string {} is invalid", service_string)),
    }
}
