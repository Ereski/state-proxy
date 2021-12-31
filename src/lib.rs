//! A high performance, load balancing proxy. State Proxy can also store the application state for
//! each connection so that proxied services can crash or restart without losing that state or
//! external connections.
//!
//! Additional features include:
//!
//! - Support for multiple protocols, including HTTP(S) and SSH.
//! - Protocol translation between external and internal connections. For example, external
//!   clients connecting through HTTPS while the proxy communicates with backend endpoints through
//!   SSH.
//! - Endpoint discovery through Kubernetes.
//! - State handover between different proxy instances. With additional configuration, neither
//!   state nor external connections are lost through upgrades or restarts of proxy instances.
//!   This requires that both instances are able to communicate through Unix Domain Sockets.
//!
//! Planned features:
//!
//! - Balancing based on health and performance metrics.
//! - Dynamic balancing, with migration of long-lived connections between proxied instances.
//! - Handover to remote instances with `TCP_REPAIR`.
//! - Dynamic self-balance and controlled scaling by migrating long-lived connections between
//!   proxy instances.

pub mod backend;
pub mod matchmaking;
pub mod protocol;

#[cfg(feature = "handover")]
pub mod handover;
#[cfg(feature = "protocol-http")]
pub mod http;
#[cfg(feature = "discovery-kubernetes")]
pub mod kubernetes;
#[cfg(feature = "protocol-ssh")]
pub mod ssh;
#[cfg(feature = "protocol-websockets")]
pub mod websockets;

#[cfg(test)]
pub mod test_utils;
