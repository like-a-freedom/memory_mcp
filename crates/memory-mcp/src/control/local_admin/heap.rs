//! Operator-gated glibc heap dump.
//!
//! `malloc_info(3)` renders the allocator's own map of arenas and bins — the
//! only view that says *which* arena holds the memory when RSS is high and the
//! application's own accounting is flat. It is expensive (it walks the whole
//! structure it describes), it exposes allocator internals rather than tenant
//! data, and so the route is off unless the deployment sets
//! `MEMORY_MCP_HTTP_HEAP_DUMP_ENABLED`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[cfg(all(target_os = "linux", target_env = "gnu"))]
use std::os::raw::{c_char, c_int, c_void};

#[cfg(all(target_os = "linux", target_env = "gnu"))]
unsafe extern "C" {
    /// `FILE *open_memstream(char **ptr, size_t *sizeloc)`.
    fn open_memstream(buffer: *mut *mut c_char, size: *mut usize) -> *mut c_void;
    /// `int malloc_info(int options, FILE *stream)`.
    fn malloc_info(options: c_int, stream: *mut c_void) -> c_int;
    /// `int fclose(FILE *stream)`.
    fn fclose(stream: *mut c_void) -> c_int;
    /// `void free(void *ptr)` — the allocator that produced the stream buffer.
    fn free(pointer: *mut c_void);
}

/// `GET /api/v1/admin/heap` — glibc's own report on this process's heap.
///
/// Operator-only and expensive: `malloc_info` walks every arena and every bin,
/// so on a large heap it can hold the allocator's internal locks for hundreds
/// of milliseconds. It answers with allocator internals only — never a request
/// body, a credential or a tenant's memory.
pub(crate) async fn heap_dump(super::RequireAdmin(_principal): super::RequireAdmin) -> Response {
    match malloc_info_text() {
        Some(text) => text_response(StatusCode::OK, text),
        None => text_response(
            StatusCode::NOT_IMPLEMENTED,
            "malloc_info is unavailable: the heap dump needs glibc malloc".to_string(),
        ),
    }
}

/// Run `malloc_info(0, stream)` and return the report glibc wrote.
///
/// `None` off Linux/glibc, and `None` if any step fails — never a partial or a
/// fabricated report.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub(crate) fn malloc_info_text() -> Option<String> {
    let mut buffer: *mut c_char = std::ptr::null_mut();
    let mut length: usize = 0;
    // SAFETY: `open_memstream` returns a stream we own and fills `buffer` and
    // `length` when the stream is closed; `malloc_info` writes its report into
    // that stream; `fclose` flushes and closes it. The buffer is allocated by
    // libc and released by the libc `free` below — the same allocator that
    // produced it, so a non-glibc Rust global allocator is never asked to free
    // a libc pointer. The bytes are copied out and validated as UTF-8 before the
    // pointer is dropped, and nothing outside this block is read or written.
    unsafe {
        let stream = open_memstream(&mut buffer, &mut length);
        if stream.is_null() {
            return None;
        }
        let wrote = malloc_info(0, stream);
        let closed = fclose(stream);
        if wrote != 0 || closed != 0 || buffer.is_null() {
            if !buffer.is_null() {
                free(buffer.cast());
            }
            return None;
        }
        let text = std::str::from_utf8(std::slice::from_raw_parts(buffer.cast::<u8>(), length))
            .map(str::to_owned)
            .ok();
        free(buffer.cast());
        text
    }
}

/// Off Linux/glibc there is no `malloc_info` to call.
#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub(crate) fn malloc_info_text() -> Option<String> {
    None
}

/// The glibc allocator tunables, plus this process's `mallinfo2` summary.
///
/// Reported as the value the variable holds in *this* process: `unset` when it
/// is absent, because glibc then uses its own default and a later `setenv`
/// changes nothing. The summary rides along so the tunables and the state they
/// produced are read from one line. Keys are the log field names, each mirroring
/// one glibc variable (`malloc_arena_max` for `MALLOC_ARENA_MAX`, and so on).
pub fn effective_glibc_tunables() -> Vec<(&'static str, String)> {
    let read = |name: &str| std::env::var(name).unwrap_or_else(|_| "unset".to_string());
    let alloc = crate::platform::process_memory::read_alloc();
    let allocator_summary = match alloc.in_use_bytes {
        Some(in_use) => format!(
            "arena={} in_use={in_use} free={} keepcost={}",
            number_or_unknown(alloc.arena_bytes),
            number_or_unknown(alloc.free_bytes),
            number_or_unknown(alloc.keepcost_bytes),
        ),
        None => "unavailable (no glibc mallinfo2 on this platform)".to_string(),
    };
    vec![
        ("malloc_arena_max", read("MALLOC_ARENA_MAX")),
        ("malloc_mmap_threshold", read("MALLOC_MMAP_THRESHOLD_")),
        ("malloc_trim_threshold", read("MALLOC_TRIM_THRESHOLD_")),
        ("allocator_summary", allocator_summary),
    ]
}

fn number_or_unknown(value: Option<u64>) -> String {
    value.map_or_else(|| "unknown".to_string(), |bytes| bytes.to_string())
}

/// A `no-store` plain-text answer, so an intermediary never caches a heap dump.
fn text_response(status: StatusCode, body: String) -> Response {
    (
        status,
        [
            (
                axum::http::header::CONTENT_TYPE,
                "text/plain; charset=utf-8",
            ),
            (axum::http::header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower_service::Service;

    use crate::http::test_state::HttpStateTestBuilder;

    async fn status_of(state: std::sync::Arc<crate::http::HttpState>, uri: &str) -> StatusCode {
        let mut router =
            crate::http::router::build_router(state, None).expect("router builds in tests");
        let request = Request::builder()
            .method("GET")
            .uri(uri)
            .header("host", "localhost")
            .body(Body::empty())
            .expect("request builds");
        router.call(request).await.expect("dispatch").status()
    }

    /// The route is absent unless the operator asks for it.
    ///
    /// A heap dump is an operator diagnostic, not a shipped surface: a
    /// deployment that never sets the flag must not answer it at all, so
    /// `/api/v1/admin/heap` falls through to the reserved-surface 404.
    #[tokio::test]
    async fn heap_dump_route_is_disabled_by_default() {
        let (builder, _store) = HttpStateTestBuilder::local_admin().await;
        let state = builder.build().await.expect("local admin HTTP state");

        assert_eq!(
            status_of(state, "/api/v1/admin/heap").await,
            StatusCode::NOT_FOUND,
            "with the flag unset, the route must not be mounted"
        );
    }

    /// With the flag set the route mounts, and the admin session still guards it.
    ///
    /// Both halves matter: a mount assertion alone would pass for a route
    /// mounted without its guard, and the guard alone would pass for a route
    /// that was never mounted (a 404 is not a 401). Together they pin a route
    /// that exists *and* refuses an anonymous caller.
    #[tokio::test]
    async fn heap_dump_route_is_mounted_when_enabled_and_requires_a_session() {
        let (builder, _store) = HttpStateTestBuilder::local_admin().await;
        let state = builder
            .with_heap_dump_enabled(true)
            .build()
            .await
            .expect("local admin HTTP state");

        assert_eq!(
            status_of(state, "/api/v1/admin/heap").await,
            StatusCode::UNAUTHORIZED,
            "the route is mounted when the flag is set, and refuses an anonymous caller"
        );
    }

    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    #[test]
    fn malloc_info_text_is_present_on_glibc() {
        let text = malloc_info_text().expect("glibc has malloc_info");
        assert!(text.contains("<malloc"), "{text}");
    }

    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    #[test]
    fn malloc_info_text_is_none_off_glibc() {
        assert_eq!(malloc_info_text(), None);
    }

    /// The three knobs glibc reads once at start, plus this process's summary.
    ///
    /// Repeating the names here rather than deriving them keeps the log
    /// vocabulary pinned: an operator greps these keys, so a rename is a
    /// deliberate edit and not a silent drift.
    #[test]
    fn effective_glibc_tunables_name_the_three_knobs() {
        let tunables = effective_glibc_tunables();
        for expected in [
            "malloc_arena_max",
            "malloc_mmap_threshold",
            "malloc_trim_threshold",
            "allocator_summary",
        ] {
            assert_eq!(
                tunables.iter().filter(|(key, _)| *key == expected).count(),
                1,
                "`{expected}` must appear exactly once: {tunables:?}"
            );
        }
    }
}
