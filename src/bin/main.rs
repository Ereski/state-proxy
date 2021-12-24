use anyhow::{anyhow, Context, Result};
use clap::{crate_authors, crate_description, App, Arg, ArgMatches};
use state_proxy::{backend::ServiceManager, matchmaking::Matchmaker};
use std::{
    ffi::{OsStr, OsString},
    process,
    sync::Arc,
};
use tokio::runtime::{Builder, Runtime};
use tracing::{error, info, warn, Level};
use tracing_subscriber::fmt::time::Uptime;

#[cfg(feature = "handover")]
use state_proxy::handover;
#[cfg(feature = "protocol-http")]
use state_proxy::http::{HttpClient, HttpServer};
#[cfg(feature = "discovery-kubernetes")]
use state_proxy::kubernetes::{KubernetesConfig, KubernetesServiceDiscovery};

static PROGRAM_NAME: &str = "State Proxy";
static PROGRAM_VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    let args = parse_command_line();
    init_logging();
    if let Err(err) = run(args) {
        error!("CRITICAL: {:?}", err);
        process::exit(1);
    }
}

fn parse_command_line() -> ArgMatches<'static> {
    let app = App::new(PROGRAM_NAME)
        .author(crate_authors!(", "))
        .version(PROGRAM_VERSION)
        .about(crate_description!())
        .args(&[
            Arg::with_name("worker-threads")
                .long("worker-threads")
                .takes_value(true)
                .value_name("COUNT")
                .validator_os(arg_is_number)
                .help("Number of worker threads to spawn. Defaults to the number of CPU threads if not specified."),
        ]);
    #[cfg(feature = "discovery-kubernetes")]
    let app = app.args(&[
        Arg::with_name("k8s-namespace")
            .long("k8s-namespace")
            .takes_value(true)
            .value_name("NAMESPACE")
            .help("List Kubernetes pods from the given namespace."),
        Arg::with_name("k8s-url")
            .long("k8s-url")
            .takes_value(true)
            .value_name("URL")
            .help("Connect to the Kubernetes cluster given by the URL."),
        Arg::with_name("k8s-from-kubeconfig")
            .short("k")
            .long("k8s-from-kubeconfig")
            .conflicts_with("k8s-url")
            .help("Connect to the Kubernetes cluster defined by a configuration file pointed by the `KUBECONFIG` environment variable, or `~/.kube/config` is `KUBECONFIG` is not set."),
            Arg::with_name("k8s-context")
                .long("k8s-context")
                .takes_value(true)
                .value_name("CONTEXT")
                .requires("k8s-context")
                .help("Use the given context, When reading from a configuration file."),
    ]);
    #[cfg(feature = "protocol-http")]
    let app = app.args(&[Arg::with_name("accept-http")
        .long("accept-http")
        .help("Accept external HTTP connections.")]);
    #[cfg(feature = "protocol-ssh")]
    let app = app.args(&[Arg::with_name("accept-ssh")
        .long("accept-ssh")
        .help("Accept external SSH connections.")]);
    #[cfg(feature = "protocol-websockets")]
    let app = app.args(&[Arg::with_name("accept-websockets")
        .long("accept-websockets")
        .help("Accept external websockets connections.")]);
    #[cfg(feature = "handover")]
    let app = app.args(&[Arg::with_name("handover-socket")
        .long("handover-socket")
        .takes_value(true)
        .value_name("ADDRESS")
        .help("Address of the Unix Domain Socket used for handover.")]);

    app.get_matches()
}

fn arg_is_number(x: &OsStr) -> Result<(), OsString> {
    if x.to_string_lossy()
        .find(|x: char| !x.is_digit(10))
        .is_none()
    {
        Ok(())
    } else {
        Err(OsString::from("must be a number"))
    }
}

fn init_logging() {
    tracing_subscriber::fmt()
        .with_timer(Uptime::default())
        .with_thread_ids(true)
        .with_max_level(Level::DEBUG)
        .init();
}

fn run(args: ArgMatches) -> Result<()> {
    info!("{} {} starting...", PROGRAM_NAME, PROGRAM_VERSION);

    init_executor(&args)
        .with_context(|| {
            anyhow!("failed to initialize the asynchronous runtime")
        })?
        .block_on(async move {
            let service_manager = ServiceManager::new();
            register_kubernetes(&service_manager, &args).await?;

            let mut matchmaker = Matchmaker::new(service_manager);
            register_http(&mut matchmaker, &args)?;
            register_ssh(&mut matchmaker, &args)?;
            register_websockets(&mut matchmaker, &args)?;
            init_handover(&args);

            // No need to hold onto this anymore, so drop it to get a bit of memory back
            drop(args);

            info!("{} {} running", PROGRAM_NAME, PROGRAM_VERSION);

            matchmaker.run().await
        })
}

fn init_executor(args: &ArgMatches) -> Result<Runtime> {
    let mut builder = Builder::new_multi_thread();
    builder.enable_all();
    if let Some(worker_threads) = args.value_of("worker-threads") {
        builder.worker_threads(worker_threads.parse().unwrap());
    }

    Ok(builder.build()?)
}

async fn register_kubernetes(
    _service_manager: &Arc<ServiceManager>,
    _args: &ArgMatches<'_>,
) -> Result<()> {
    #[cfg(feature = "discovery-kubernetes")]
    {
        let config = if _args.is_present("k8s-from-kubeconfig") {
            KubernetesConfig::KubeConfig {
                context: _args.value_of("k8s-context").map(|x| x.to_owned()),
            }
        } else if let Some(url) = _args.value_of("k8s-url") {
            KubernetesConfig::Explicit {
                url: url.try_into()?,
            }
        } else {
            warn!("Neither --k8s-from-kubeconfig nor --k8s-url specified. Kubernetes discovery service disabled.");

            return Ok(());
        };
        _service_manager
            .register_service_discovery(
                KubernetesServiceDiscovery::new(
                    config,
                    _args.value_of("k8s-namespace").map(|x| x.to_owned()),
                )
                .await?,
            )
            .await?;
    }

    Ok(())
}

fn register_http(
    _matchmaker: &mut Matchmaker,
    _args: &ArgMatches,
) -> Result<()> {
    #[cfg(feature = "protocol-http")]
    {
        if _args.is_present("accept-http") {
            _matchmaker.register_client(HttpClient::new())?;
            _matchmaker.register_server(HttpServer)?;
        }
    }

    Ok(())
}

fn register_ssh(
    _matchmaker: &mut Matchmaker,
    _args: &ArgMatches,
) -> Result<()> {
    #[cfg(feature = "protocol-ssh")]
    {
        if _args.is_present("accept-ssh") {}
    }

    Ok(())
}

fn register_websockets(
    _matchmaker: &mut Matchmaker,
    _args: &ArgMatches,
) -> Result<()> {
    #[cfg(feature = "protocol-websockets")]
    {
        if _args.is_present("accept-websockets") {}
    }

    Ok(())
}

fn init_handover(_args: &ArgMatches) {
    #[cfg(feature = "handover")]
    {
        if let Some(handover_socket) = _args.value_of("handover-socket") {
            handover::start(handover_socket.to_owned());
        } else {
            warn!(
                "--handover-socket not specified. This instance will not be able to send or receive state from other local instances",
            );
        }
    }
}
