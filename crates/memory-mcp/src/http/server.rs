//! HTTP server bind + serve loop.

use axum::serve as axum_serve;
use tokio::net::TcpListener;

use super::config::HttpConfig;

/// Binds, reports the local address on stdout as `memory_mcp_http bound=<addr>`
/// (integration tests parse this line), then serves until the shutdown
/// token is cancelled or the listener closes.
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
    let serving = axum_serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async move { token.cancelled().await });

    match tokio::time::timeout(grace, serving).await {
        Ok(result) => result,
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "HTTP graceful shutdown exceeded configured deadline",
        )),
    }
}
