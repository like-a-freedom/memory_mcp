//! Optional Prometheus recorder/listener installation.
//!
//! Zero-config by design: without the `prometheus` feature or without a
//! valid `MEMORY_PROMETHEUS_LISTEN_ADDR`, no socket opens and the
//! `metrics` facade stays no-op. `127.0.0.1:0` is supported for tests.

use std::net::SocketAddr;

use crate::service::MemoryError;

/// Environment variable that opts into a Prometheus HTTP listener.
pub const ENV_PROMETHEUS_LISTEN_ADDR: &str = "MEMORY_PROMETHEUS_LISTEN_ADDR";

/// Install the Prometheus recorder/listener when the feature is enabled
/// and `MEMORY_PROMETHEUS_LISTEN_ADDR` is set.
///
/// Without the feature, this is a no-op. With the feature but without the
/// env var, the recorder stays unset and no socket opens. With both, a
/// duplicate recorder or invalid address is a startup error.
pub fn install() -> Result<(), MemoryError> {
    #[cfg(feature = "prometheus")]
    {
        let Some(addr) = parse_listen_addr()? else {
            return Ok(());
        };
        install_with_addr(addr)?;
    }
    Ok(())
}

#[cfg(feature = "prometheus")]
fn parse_listen_addr() -> Result<Option<SocketAddr>, MemoryError> {
    match std::env::var(ENV_PROMETHEUS_LISTEN_ADDR) {
        Ok(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            let addr: SocketAddr = trimmed.parse().map_err(|_| {
                MemoryError::ConfigInvalid(format!(
                    "{ENV_PROMETHEUS_LISTEN_ADDR}='{trimmed}' is not a valid SocketAddr (use ip:port, e.g. 127.0.0.1:9100)"
                ))
            })?;
            Ok(Some(addr))
        }
        Err(_) => Ok(None),
    }
}

#[cfg(feature = "prometheus")]
fn install_with_addr(addr: SocketAddr) -> Result<(), MemoryError> {
    metrics_exporter_prometheus::PrometheusBuilder::new()
        .with_http_listener(addr)
        .install()
        .map_err(|err| {
            MemoryError::ConfigInvalid(format!(
                "failed to install Prometheus exporter on {addr}: {err}"
            ))
        })?;
    Ok(())
}

#[cfg(not(feature = "prometheus"))]
#[allow(dead_code)]
fn parse_listen_addr() -> Result<Option<SocketAddr>, MemoryError> {
    Ok(None)
}

#[cfg(not(feature = "prometheus"))]
#[allow(dead_code)]
fn install_with_addr(_addr: SocketAddr) -> Result<(), MemoryError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_var_name_is_stable() {
        assert_eq!(ENV_PROMETHEUS_LISTEN_ADDR, "MEMORY_PROMETHEUS_LISTEN_ADDR");
    }

    #[test]
    #[cfg(not(feature = "prometheus"))]
    fn install_is_noop_without_feature() {
        // Without the feature, install always succeeds and never opens a socket.
        install().expect("install succeeds without prometheus feature");
    }
}
