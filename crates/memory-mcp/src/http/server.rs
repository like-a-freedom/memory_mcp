//! HTTP server bind + serve loop.

use axum::serve as axum_serve;
use tokio::net::TcpListener;

use super::config::HttpConfig;

/// Binds, reports the local address on stdout as `memory_mcp_http bound=<addr>`
/// (integration tests parse this line), then serves until the shutdown
/// token is cancelled or the listener closes.
///
/// `shutdown_grace` bounds only the **drain** that follows a shutdown
/// signal. It is not a lifetime cap: wrapping the whole serve future in
/// the timeout would terminate a perfectly healthy server `grace`
/// seconds after start, which is what an earlier revision did.
pub async fn serve(
    cfg: HttpConfig,
    router: axum::Router,
    shutdown: super::shutdown::ShutdownState,
) -> std::io::Result<()> {
    let listener = TcpListener::bind(cfg.bind).await?;
    let local_addr = listener.local_addr()?;
    eprintln!("memory_mcp::http: listener bound at {local_addr}");
    println!("memory_mcp_http bound={local_addr}");
    let token = shutdown.token();
    let grace = cfg.shutdown_grace;
    let drain = token.clone();
    let serving = axum_serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async move { token.cancelled().await });

    // Race the serve loop against the drain deadline, but arm the
    // deadline only once the shutdown signal has actually fired.
    let shutdown_armed = async move {
        drain.cancelled().await;
        tokio::time::sleep(grace).await;
    };
    tokio::select! {
        result = serving => result,
        _ = shutdown_armed => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "HTTP graceful shutdown exceeded configured deadline",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A live server must outlive `shutdown_grace` when no shutdown has
    /// been requested. This is the regression guard for the revision that
    /// used the grace period as a total server lifetime.
    #[tokio::test]
    async fn server_outlives_the_shutdown_grace_without_a_signal() {
        let mut cfg = HttpConfig::default_for_test();
        cfg.bind = "127.0.0.1:0".parse().expect("test bind");
        cfg.shutdown_grace = Duration::from_millis(50);
        let shutdown = crate::http::shutdown::ShutdownState::new();
        let router = axum::Router::new().route("/healthz", axum::routing::get(|| async { "ok" }));

        let handle = tokio::spawn(serve(cfg, router, shutdown.clone()));
        // Wait well past the grace period.
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(
            !handle.is_finished(),
            "the serve loop must still be running 5x the grace period after start"
        );

        shutdown.begin();
        let result = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("shutdown completes")
            .expect("join");
        assert!(
            result.is_ok(),
            "graceful shutdown is not an error: {result:?}"
        );
    }
}
