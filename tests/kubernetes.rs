use crate::test_utils::panic_on_timeout;
use futures::join;
use pretty_assertions::assert_eq;
use serde_json::Value;
use state_proxy::{
    backend::discovery::{DiscoveryEvent, ServiceDiscovery},
    kubernetes::{
        KubernetesConfig, KubernetesServiceDiscovery, API_VERSION,
        API_VERSION_LABEL, SERVICES_ANNOTATION,
    },
};
use tokio::{process::Command, sync::mpsc};

mod test_utils;

static MINIKUBE_COMMAND: &str = "minikube";
static MINIKUBE_PROFILE: &str = "state-proxy-kubernetes-test";

// This test is disabled by default as it can be expensive and requires `minikube` to be installed
// in the host. This test is also racy and can cause data loss if `minikube` is being used
// elsewhere concurrently
#[tokio::test]
#[ignore]
async fn kubernetes_minikube() {
    let minikube_version = minikube(&["version", "--output=json"])
        .output()
        .await
        .unwrap();
    if !minikube_version.status.success() {
        panic!("`minikube version` failed");
    }
    println!(
        "Using minikube {}",
        serde_json::from_slice::<Value>(&minikube_version.stdout).unwrap()
            ["minikubeVersion"]
    );

    let minikube_start = minikube(&[
        "start",
        "-p",
        MINIKUBE_PROFILE,
        "--keep-context=true",
        "--interactive=false",
        "--wait=all",
    ])
    .status()
    .await
    .unwrap();
    if !minikube_start.success() {
        panic!("Failed to start minikube");
    }
    println!("Test minikube started under profile {}", MINIKUBE_PROFILE);

    let minikube_profile = minikube(&["profile", MINIKUBE_PROFILE])
        .status()
        .await
        .unwrap();
    if !minikube_profile.success() {
        panic!("Failed to set the minikube profile to {}", MINIKUBE_PROFILE);
    }

    let (sender, mut receiver) = mpsc::channel(10);
    KubernetesServiceDiscovery::new(
        KubernetesConfig::KubeConfig {
            context: Some(MINIKUBE_PROFILE.to_owned()),
        },
        None,
    )
    .await
    .unwrap()
    .run_with_sender(sender);

    // Add four pods:
    //
    // - One has no labels or annotations
    // - One has the correct label but it is set to an unknown version
    // - One has the correct label but doesn't define any services
    // - The last one has the correct label and defines a single service
    //
    // After all four are added, only one `Add` event followed by a `Resume` event should be sent
    // (for the last pod)
    let (clear_pod, bad_version_pod, no_service_pod, valid_pod) = join!(
        kubectl(&["run", "0", "--image=alpine"]).status(),
        kubectl(&[
            "run",
            "1",
            "--image=alpine",
            &format!("--labels={}=garbage", API_VERSION_LABEL)
        ])
        .status(),
        kubectl(&[
            "run",
            "2",
            "--image=alpine",
            &format!("--labels={}={}", API_VERSION_LABEL, API_VERSION)
        ])
        .status(),
        kubectl(&[
            "run",
            "3",
            "--image=alpine",
            &format!("--labels={}={}", API_VERSION_LABEL, API_VERSION),
            &format!("--annotations={}=80:http:80:http", SERVICES_ANNOTATION),
            "--output=json"
        ])
        .output(),
    );
    let valid_pod = valid_pod.unwrap();
    if !(clear_pod.unwrap().success()
        && bad_version_pod.unwrap().success()
        && no_service_pod.unwrap().success()
        && valid_pod.status.success())
    {
        panic!("Failed to run test pods");
    }
    let valid_pod_uid = serde_json::from_slice::<Value>(&valid_pod.stdout)
        .unwrap()["metadata"]["uid"]
        .as_str()
        .unwrap()
        .to_owned();
    let event = panic_on_timeout(receiver.recv()).await.unwrap();
    let backend_address = match &event {
        DiscoveryEvent::Add {
            backend_address, ..
        } => backend_address.ip(),
        _ => panic!("Expected an `Add` event, received: {:#?}", event),
    };
    assert_eq!(
        event,
        DiscoveryEvent::add(
            &valid_pod_uid,
            false,
            80,
            "http".to_owned(),
            (backend_address, 80),
            "http".to_owned(),
        )
    );
    assert_eq!(
        panic_on_timeout(receiver.recv()).await.unwrap(),
        DiscoveryEvent::resume(valid_pod_uid.clone())
    );

    // Bring down the four pods created previously. Should only receive one delete event for the
    // valid pod
    let (clear_pod, bad_version_pod, no_service_pod, valid_pod) = join!(
        kubectl(&["delete", "pod/0"]).status(),
        kubectl(&["delete", "pod/1"]).status(),
        kubectl(&["delete", "pod/2"]).status(),
        kubectl(&["delete", "pod/3"]).status(),
    );
    if !(clear_pod.unwrap().success()
        && bad_version_pod.unwrap().success()
        && no_service_pod.unwrap().success()
        && valid_pod.unwrap().success())
    {
        panic!("Failed to remove test pods");
    }
    assert_eq!(
        panic_on_timeout(receiver.recv()).await.unwrap(),
        DiscoveryEvent::delete(valid_pod_uid)
    );

    let minikube_delete = minikube(&["delete"]).status().await.unwrap();
    if !minikube_delete.success() {
        eprintln!("Failed to delete minikube resources");
    }
    println!("Cleaned profile {}", MINIKUBE_PROFILE);
}

fn minikube(extra_args: &[&str]) -> Command {
    let mut command = Command::new(MINIKUBE_COMMAND);
    command.kill_on_drop(true).args(extra_args);

    command
}

fn kubectl(extra_args: &[&str]) -> Command {
    let mut command = Command::new(MINIKUBE_COMMAND);
    let mut args = vec!["kubectl", "--ssh=true", "--"];
    args.extend_from_slice(extra_args);
    command.kill_on_drop(true).args(args);

    command
}
