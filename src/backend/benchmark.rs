use super::{discovery::DiscoveryEvent, PortEvent, ServiceManager};
use std::sync::Arc;
use tokio::{runtime::Runtime, sync::mpsc};

pub struct BenchServiceManager {
    service_manager: Arc<ServiceManager>,
}

impl BenchServiceManager {
    pub fn new(runtime: &Runtime) -> Self {
        let service_manager = ServiceManager::new();
        let service_manager2 = service_manager.clone();
        runtime.spawn(async move {
            let (sender, mut receiver) = mpsc::channel(1000000);
            service_manager2.send_ports_events_to(sender);
            while receiver.recv().await.is_some() {}
        });

        BenchServiceManager { service_manager }
    }

    pub async fn process_event(
        &self,
        from: Arc<String>,
        event: DiscoveryEvent,
    ) {
        self.service_manager.process_event(from, event).await;
    }
}
