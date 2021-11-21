use crate::{backend::K8sConfig, matchmaking::Matchmaker};
use anyhow::Result;
use clap::{crate_authors, crate_description, crate_version, App, Arg, ArgMatches};
use kube::Config as KubeConfig;
use std::ffi::{OsStr, OsString};
use tokio::runtime::{Builder, Runtime};
use tracing::{warn, Level};
use tracing_subscriber::fmt::time::Uptime;

mod backend;
mod matchmaking;

#[cfg(feature = "handover")]
mod handover;
#[cfg(feature = "http")]
mod http;
#[cfg(feature = "kubernetes")]
mod kubernetes;
#[cfg(feature = "ssh")]
mod ssh;
#[cfg(feature = "websockets")]
mod websockets;

static PROGRAM_NAME: &str = "State Proxy";

static ARG_K8S_URL: &str = "k8s-url";
static ARG_K8S_NAMESPACE: &str = "k8s-namespace";
static ARG_INFER_K8S_CONFIG: &str = "infer-k8s-config";
static ARG_WORKER_THREADS: &str = "worker-threads";

#[cfg(feature = "handover")]
static ARG_HANDOVER_SOCKET: &str = "handover-socket";

#[cfg(feature = "http")]
static ARG_ACCEPT_HTTP: &str = "accept-http";

#[cfg(feature = "websockets")]
static ARG_ACCEPT_WEBSOCKETS: &str = "accept-websockets";

#[cfg(feature = "ssh")]
static ARG_ACCEPT_SSH: &str = "accept-ssh";

fn main() {
    let args = parse_command_line();
    init_logging();
    // TODO: proper error handling and reporting
    let mut matchmaker = Matchmaker::new();
    let executor = init_executor(&args).unwrap();
    #[cfg(feature = "handover")]
    init_handover(&args, &executor);
    #[cfg(feature = "http")]
    init_http(&mut matchmaker, &args);
    #[cfg(feature = "websockets")]
    init_websockets(&mut matchmaker, &args);
    #[cfg(feature = "ssh")]
    init_ssh(&mut matchmaker, &args);
    // TODO: Error handling here too
    run_backend_communication(args, executor).unwrap();
}

fn init_logging() {
    tracing_subscriber::fmt()
        .with_timer(Uptime::default())
        .with_thread_ids(true)
        .with_max_level(Level::DEBUG)
        .init();
}

fn parse_command_line() -> ArgMatches<'static> {
    let app = App::new(PROGRAM_NAME)
        .author(crate_authors!(", "))
        .version(crate_version!())
        .about(crate_description!())
        .args(&[
            Arg::with_name(ARG_K8S_URL)
                .long(ARG_K8S_URL)
                .takes_value(true)
                .value_name("URL")
                .required_unless(ARG_INFER_K8S_CONFIG)
                .help("Connect to the Kubernetes cluster given by the URL."),
            Arg::with_name(ARG_K8S_NAMESPACE)
                .long(ARG_K8S_NAMESPACE)
                .takes_value(true)
                .value_name("NAMESPACE")
                .requires(ARG_K8S_URL)
                .help("List Kubernetes pods from the given namespace."),
            Arg::with_name(ARG_INFER_K8S_CONFIG)
                .short("k")
                .long(ARG_INFER_K8S_CONFIG)
                .conflicts_with(ARG_K8S_URL)
                .required_unless(ARG_K8S_URL)
                .help("Connect to a Kubernetes cluster and infer the configuration from the environment."),
            Arg::with_name(ARG_WORKER_THREADS)
                .long(ARG_WORKER_THREADS)
                .takes_value(true)
                .value_name("COUNT")
                .validator_os(arg_is_number)
                .help("Number of worker threads to spawn. Defaults to the number of CPU threads if not specified."),
        ]);
    #[cfg(feature = "handover")]
    let app = app.args(&[Arg::with_name(ARG_HANDOVER_SOCKET)
        .long(ARG_HANDOVER_SOCKET)
        .takes_value(true)
        .value_name("ADDRESS")
        .help("Address of the Unix Domain Socket used for handover.")]);
    #[cfg(feature = "http")]
    let app = app.args(&[Arg::with_name(ARG_ACCEPT_HTTP)
        .long(ARG_ACCEPT_HTTP)
        .help("Accept external HTTP connections.")]);
    #[cfg(feature = "websockets")]
    let app = app.args(&[Arg::with_name(ARG_ACCEPT_WEBSOCKETS)
        .long(ARG_ACCEPT_WEBSOCKETS)
        .help("Accept external websockets connections.")]);
    #[cfg(feature = "ssh")]
    let app = app.args(&[Arg::with_name(ARG_ACCEPT_SSH)
        .long(ARG_ACCEPT_SSH)
        .help("Accept external SSH connections.")]);

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

fn init_executor(args: &ArgMatches) -> Result<Runtime> {
    let mut builder = Builder::new_multi_thread();
    builder.enable_all();
    if let Some(worker_threads) = args.value_of(ARG_WORKER_THREADS) {
        builder.worker_threads(worker_threads.parse().unwrap());
    }

    Ok(builder.build()?)
}

#[cfg(feature = "handover")]
fn init_handover(args: &ArgMatches, executor: &Runtime) {
    if let Some(handover_socket) = args.value_of(ARG_HANDOVER_SOCKET) {
        executor.spawn(handover::run(handover_socket.to_owned()));
    } else {
        warn!(
            "--{} not specified. This instance will not be able to send or receive state from other local instances",
            ARG_HANDOVER_SOCKET,
        );
    }
}

#[cfg(feature = "http")]
fn init_http(matchmaker: &mut Matchmaker, args: &ArgMatches) {
    if args.is_present(ARG_ACCEPT_HTTP) {
        http::register(matchmaker);
    }
}

#[cfg(feature = "websockets")]
fn init_websockets(matchmaker: &mut Matchmaker, args: &ArgMatches) {
    if args.is_present(ARG_ACCEPT_WEBSOCKETS) {
        websockets::register(matchmaker);
    }
}

#[cfg(feature = "ssh")]
fn init_ssh(matchmaker: &mut Matchmaker, args: &ArgMatches) {
    if args.is_present(ARG_ACCEPT_SSH) {
        ssh::register(matchmaker);
    }
}

fn run_backend_communication(args: ArgMatches, executor: Runtime) -> Result<()> {
    let config = if args.is_present(ARG_INFER_K8S_CONFIG) {
        K8sConfig::Infer
    } else {
        let mut kube_config = KubeConfig::new(args.value_of(ARG_K8S_URL).unwrap().try_into()?);
        if let Some(namespace) = args.value_of(ARG_K8S_NAMESPACE) {
            kube_config.default_namespace = namespace.to_owned();
        }

        K8sConfig::Explicit(kube_config)
    };
    drop(args);
    executor.block_on(backend::run_communication(config));

    Ok(())
}
