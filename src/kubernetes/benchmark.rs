use super::KubernetesDiscoveryRuntime;
use crate::backend::discovery::DiscoveryEvent;
use anyhow::Result;
use k8s_openapi::api::core::v1::Pod;
use kube::runtime::watcher::Event;
use std::sync::Arc;
use tokio::sync::mpsc::{self, Receiver, Sender};

pub struct BenchKubernetesDiscoveryRuntime<'a> {
    kubernetes_runtime: KubernetesDiscoveryRuntime<'a>,
}

impl<'a> BenchKubernetesDiscoveryRuntime<'a> {
    pub fn new(
        name: &'a Arc<String>,
        channel_capacity: usize,
    ) -> (Self, Receiver<DiscoveryEvent>) {
        let (sender, receiver) = mpsc::channel(channel_capacity);

        (
            Self {
                kubernetes_runtime: KubernetesDiscoveryRuntime::new(
                    name, sender,
                ),
            },
            receiver,
        )
    }

    pub async fn handle_pod_event(&mut self, event: Event<Pod>) -> Result<()> {
        self.kubernetes_runtime.handle_pod_event(event).await
    }
}
