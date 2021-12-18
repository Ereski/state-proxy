use crate::{backend::ServiceManager, matchmaking::Matchmaker};
use anyhow::{anyhow, Context, Result};
use clap::{crate_authors, crate_description, App, Arg, ArgMatches};
use std::{
    ffi::{OsStr, OsString},
    process,
};
use tokio::runtime::{Builder, Runtime};
use tracing::{error, info, warn, Level};
use tracing_subscriber::fmt::time::Uptime;

#[cfg(feature = "discovery-kubernetes")]
use crate::kubernetes::KubernetesConfig;

mod backend;
mod matchmaking;
mod protocol;

#[cfg(feature = "handover")]
mod handover;
#[cfg(feature = "protocol-http")]
mod http;
#[cfg(feature = "discovery-kubernetes")]
mod kubernetes;
#[cfg(feature = "protocol-ssh")]
mod ssh;
#[cfg(feature = "protocol-websockets")]
mod websockets;

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
        Arg::with_name("k8s-url")
            .long("k8s-url")
            .takes_value(true)
            .value_name("URL")
            .conflicts_with("infer-k8s-config")
            .help("Connect to the Kubernetes cluster given by the URL."),
        Arg::with_name("k8s-namespace")
            .long("k8s-namespace")
            .takes_value(true)
            .value_name("NAMESPACE")
            .requires("k8s-url")
            .help("List Kubernetes pods from the given namespace."),
        Arg::with_name("infer-k8s-config")
            .short("k")
            .long("infer-k8s-config")
            .help("Connect to the Kubernetes cluster inferred from the environment."),
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

    let executor = init_executor(&args).with_context(|| {
        anyhow!("failed to initialize the asynchronous runtime")
    })?;

    let mut matchmaker = Matchmaker::new();
    register_http(&mut matchmaker, &args)?;
    register_ssh(&mut matchmaker, &args)?;
    register_websockets(&mut matchmaker, &args)?;
    init_handover(&args, &executor);

    let mut service_manager = ServiceManager::new(matchmaker);
    register_kubernetes(&mut service_manager, &args)?;

    // No need to hold onto this anymore, so drop it to get a bit of memory back
    drop(args);

    info!("{} {} running", PROGRAM_NAME, PROGRAM_VERSION);

    executor.block_on(service_manager.run())
}

fn init_executor(args: &ArgMatches) -> Result<Runtime> {
    let mut builder = Builder::new_multi_thread();
    builder.enable_all();
    if let Some(worker_threads) = args.value_of("worker-threads") {
        builder.worker_threads(worker_threads.parse().unwrap());
    }

    Ok(builder.build()?)
}

fn register_kubernetes(
    _service_manager: &mut ServiceManager,
    _args: &ArgMatches,
) -> Result<()> {
    #[cfg(feature = "discovery-kubernetes")]
    {
        let config = if _args.is_present("infer-k8s-config") {
            KubernetesConfig::Infer
        } else if let Some(url) = _args.value_of("k8s-url") {
            KubernetesConfig::Explicit {
                url: url.try_into()?,
            }
        } else {
            warn!("Neither --infer-k8s-config nor --k8s-url specified. Kubernetes discovery service disabled.");

            return Ok(());
        };
        kubernetes::register(
            _service_manager,
            config,
            _args.value_of("k8s-namespace").map(|x| x.to_owned()),
        )?;
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
            http::register(_matchmaker)?;
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
        if _args.is_present("accept-ssh") {
            ssh::register(_matchmaker)?;
        }
    }

    Ok(())
}

fn register_websockets(
    _matchmaker: &mut Matchmaker,
    _args: &ArgMatches,
) -> Result<()> {
    #[cfg(feature = "protocol-websockets")]
    {
        if _args.is_present("accept-websockets") {
            websockets::register(_matchmaker)?;
        }
    }

    Ok(())
}

fn init_handover(_args: &ArgMatches, _executor: &Runtime) {
    #[cfg(feature = "handover")]
    {
        if let Some(handover_socket) = _args.value_of("handover-socket") {
            _executor.spawn(handover::run(handover_socket.to_owned()));
        } else {
            warn!(
                "--handover-socket not specified. This instance will not be able to send or receive state from other local instances",
            );
        }
    }
}
