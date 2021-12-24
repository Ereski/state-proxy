use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use k8s_openapi::{
    api::core::v1::{Pod, PodStatus},
    apimachinery::pkg::apis::meta::v1::ObjectMeta,
};
use kube::runtime::watcher::Event;
use state_proxy::kubernetes::benchmark::BenchKubernetesDiscoveryRuntime;
use std::{cell::RefCell, collections::BTreeMap, sync::Arc};
use tokio::runtime::Runtime;

pub fn handle_pod_event_applied(c: &mut Criterion) {
    c.bench_function(
        "KubernetesDiscoveryRuntime handle applied pod event",
        |b| {
            let runtime = Runtime::new().unwrap();
            let name = Arc::new("bench".to_string());
            let (kubernetes_runtime, mut receiver) =
                BenchKubernetesDiscoveryRuntime::new(&name, 1000000);
            let kubernetes_runtime = RefCell::new(kubernetes_runtime);
            let kubernetes_runtime = &kubernetes_runtime;
            runtime
                .spawn(async move { while receiver.recv().await.is_some() {} });

            let mut i = 0;
            b.to_async(runtime).iter_batched(
                || {
                    i += 1;

                    Event::Applied(Pod {
                        metadata: ObjectMeta {
                            uid: Some(i.to_string()),
                            name: Some(i.to_string()),
                            annotations: Some(BTreeMap::new()),
                            ..ObjectMeta::default()
                        },
                        spec: None,
                        status: Some(PodStatus {
                            pod_ip: Some("127.0.0.1".to_owned()),
                            phase: Some("Running".to_owned()),
                            ..PodStatus::default()
                        }),
                    })
                },
                |event| async {
                    kubernetes_runtime
                        .borrow_mut()
                        .handle_pod_event(event)
                        .await
                        .unwrap();
                },
                BatchSize::SmallInput,
            );
        },
    );
}

pub fn handle_pod_event_deleted(c: &mut Criterion) {
    c.bench_function(
        "KubernetesDiscoveryRuntime handle deleted pod event",
        |b| {
            let runtime = Runtime::new().unwrap();
            let name = Arc::new("bench".to_string());
            let (kubernetes_runtime, mut receiver) =
                BenchKubernetesDiscoveryRuntime::new(&name, 1000000);
            let kubernetes_runtime = RefCell::new(kubernetes_runtime);
            let kubernetes_runtime = &kubernetes_runtime;
            runtime
                .spawn(async move { while receiver.recv().await.is_some() {} });

            let mut i = 0;
            b.to_async(runtime).iter_batched(
                || {
                    i += 1;

                    Event::Deleted(Pod {
                        metadata: ObjectMeta {
                            uid: Some(i.to_string()),
                            ..ObjectMeta::default()
                        },
                        spec: None,
                        status: None,
                    })
                },
                |event| async {
                    kubernetes_runtime
                        .borrow_mut()
                        .handle_pod_event(event)
                        .await
                        .unwrap();
                },
                BatchSize::SmallInput,
            );
        },
    );
}

criterion_group!(benches, handle_pod_event_applied, handle_pod_event_deleted);
criterion_main!(benches);
