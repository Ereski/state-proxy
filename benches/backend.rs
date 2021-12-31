use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use state_proxy::backend::{
    benchmark::BenchServiceManager, endpoint::EndpointEvent,
};
use std::sync::Arc;
use tokio::runtime::Runtime;

pub fn process_event(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();
    let runtime = &runtime;
    let service_manager = BenchServiceManager::new(runtime);
    let from = Arc::new("bench".to_owned());

    c.bench_function("ServiceManager process add event", |b| {
        let mut i = 0;
        b.to_async(runtime).iter_batched(
            || {
                i += 1;

                (
                    from.clone(),
                    EndpointEvent::Add {
                        uid: i.to_string(),
                        is_available: true,
                        external_port: 80,
                        external_protocol: "http".to_owned(),
                        backend_address: ([127, 0, 0, 1], 8080).into(),
                        backend_protocol: "http".to_owned(),
                    },
                )
            },
            |input| service_manager.process_event(input.0, input.1),
            BatchSize::SmallInput,
        )
    });

    c.bench_function("ServiceManager process suspend event", |b| {
        let mut i = 0;
        b.to_async(runtime).iter_batched(
            || {
                i += 1;

                (from.clone(), EndpointEvent::Suspend { uid: i.to_string() })
            },
            |input| service_manager.process_event(input.0, input.1),
            BatchSize::SmallInput,
        )
    });

    c.bench_function("ServiceManager process resume event", |b| {
        let mut i = 0;
        b.to_async(runtime).iter_batched(
            || {
                i += 1;

                (from.clone(), EndpointEvent::Resume { uid: i.to_string() })
            },
            |input| service_manager.process_event(input.0, input.1),
            BatchSize::SmallInput,
        )
    });

    c.bench_function("ServiceManager process delete event", |b| {
        let mut i = 0;
        b.to_async(runtime).iter_batched(
            || {
                i += 1;

                (from.clone(), EndpointEvent::Delete { uid: i.to_string() })
            },
            |input| service_manager.process_event(input.0, input.1),
            BatchSize::SmallInput,
        )
    });
}

criterion_group!(benches, process_event);
criterion_main!(benches);
