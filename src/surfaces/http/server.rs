//! Blocking HTTP/1.1 server.

use std::io;
use std::net::{TcpListener, TcpStream};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::error::{SurfaceKind, TexoError};

use super::request::{parse, ParseFailure};
use super::response::HttpResponse;
use super::routes::{route, RouteState};

const REQUEST_PERMITS: usize = 64;
const SSE_PERMITS: usize = 8;

/// Server configuration.
#[derive(Clone)]
pub struct ServerConfig {
    /// Listen address.
    pub addr: String,
    /// Route state.
    pub state: RouteState,
    /// Accept-loop idle sleep.
    pub idle_sleep: Duration,
    /// SSE keep-alive interval.
    pub sse_keep_alive: Duration,
}

impl ServerConfig {
    /// Build a config with default sleeps.
    #[must_use]
    pub fn new(addr: String, state: RouteState) -> Self {
        Self {
            addr,
            state,
            idle_sleep: Duration::from_millis(10),
            sse_keep_alive: Duration::from_secs(15),
        }
    }
}

/// Shared shutdown flag.
#[derive(Clone, Debug, Default)]
pub struct ShutdownHandle {
    inner: Arc<AtomicBool>,
}

impl ShutdownHandle {
    /// Create an unset shutdown handle.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Request shutdown.
    pub fn shutdown(&self) {
        self.inner.store(true, Ordering::Release);
    }

    /// Return true after shutdown was requested.
    #[must_use]
    pub fn is_shutdown(&self) -> bool {
        self.inner.load(Ordering::Acquire)
    }

    /// Register first-signal graceful shutdown and second-signal default
    /// termination for SIGTERM and SIGINT.
    ///
    /// # Errors
    /// Returns a surface error when the OS signal handlers cannot be installed.
    #[cfg(unix)]
    pub fn register_termination_signals(&self) -> Result<(), TexoError> {
        use signal_hook::consts::signal::{SIGINT, SIGTERM};

        for signal in [SIGTERM, SIGINT] {
            signal_hook::flag::register_conditional_default(signal, Arc::clone(&self.inner))
                .map_err(surface_error)?;
            signal_hook::flag::register(signal, Arc::clone(&self.inner)).map_err(surface_error)?;
        }
        Ok(())
    }

    /// Non-Unix platforms retain programmatic shutdown only.
    ///
    /// # Errors
    /// This implementation cannot fail.
    #[cfg(not(unix))]
    pub const fn register_termination_signals(&self) -> Result<(), TexoError> {
        Ok(())
    }
}

/// Server counters.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ServeStats {
    /// Accepted connections.
    pub accepted: usize,
    /// Served requests.
    pub served: usize,
    /// Failed requests.
    pub failed_requests: usize,
    /// Parser rejections.
    pub parse_rejections: usize,
    /// Caught worker panics.
    pub worker_panics: usize,
}

#[derive(Default)]
struct AtomicStats {
    accepted: AtomicUsize,
    served: AtomicUsize,
    failed_requests: AtomicUsize,
    parse_rejections: AtomicUsize,
    worker_panics: AtomicUsize,
}

impl AtomicStats {
    fn snapshot(&self) -> ServeStats {
        ServeStats {
            accepted: self.accepted.load(Ordering::Acquire),
            served: self.served.load(Ordering::Acquire),
            failed_requests: self.failed_requests.load(Ordering::Acquire),
            parse_rejections: self.parse_rejections.load(Ordering::Acquire),
            worker_panics: self.worker_panics.load(Ordering::Acquire),
        }
    }
}

/// Serve until shutdown.
///
/// # Errors
///
/// Returns [`TexoError::Surface`] when the listener cannot bind or configure.
pub fn serve(config: ServerConfig, shutdown: &ShutdownHandle) -> Result<ServeStats, TexoError> {
    let listener = TcpListener::bind(&config.addr).map_err(surface_error)?;
    serve_listener(listener, config, shutdown)
}

/// Serve an already-bound listener until shutdown.
///
/// # Errors
///
/// Returns [`TexoError::Surface`] when listener configuration or worker spawn
/// fails.
#[expect(
    clippy::needless_pass_by_value,
    reason = "the server owns the listener lifecycle until shutdown"
)]
pub fn serve_listener(
    listener: TcpListener,
    config: ServerConfig,
    shutdown: &ShutdownHandle,
) -> Result<ServeStats, TexoError> {
    listener.set_nonblocking(true).map_err(surface_error)?;
    let route_state = Arc::new(config.state);
    let request_pool = Arc::new(PermitPool::new(REQUEST_PERMITS));
    let sse_pool = Arc::new(PermitPool::new(SSE_PERMITS));
    let counters = Arc::new(AtomicStats::default());
    let mut workers = Vec::<JoinHandle<()>>::new();
    while !shutdown.is_shutdown() {
        prune_workers(&mut workers);
        match listener.accept() {
            Ok((stream, _addr)) => {
                counters.accepted.fetch_add(1, Ordering::AcqRel);
                let route_state = Arc::clone(&route_state);
                let request_pool = Arc::clone(&request_pool);
                let sse_pool = Arc::clone(&sse_pool);
                let counters = Arc::clone(&counters);
                let shutdown = shutdown.clone();
                let keep_alive = config.sse_keep_alive;
                let idle_sleep = config.idle_sleep;
                let worker = std::thread::Builder::new()
                    .name("texo-http-conn".to_string())
                    .spawn(move || {
                        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                            serve_connection(
                                stream,
                                &route_state,
                                &request_pool,
                                &sse_pool,
                                &shutdown,
                                idle_sleep,
                                keep_alive,
                                &counters,
                            );
                        }));
                        if result.is_err() {
                            counters.worker_panics.fetch_add(1, Ordering::AcqRel);
                        }
                    })
                    .map_err(surface_error)?;
                workers.push(worker);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(config.idle_sleep);
            }
            Err(error) => {
                counters.failed_requests.fetch_add(1, Ordering::AcqRel);
                if error.kind() != io::ErrorKind::Interrupted {
                    std::thread::sleep(config.idle_sleep);
                }
            }
        }
    }
    for worker in workers {
        let _ = worker.join();
    }
    Ok(counters.snapshot())
}

#[expect(
    clippy::too_many_arguments,
    reason = "connection workers receive independent admission pools, shutdown, timing, and counters"
)]
fn serve_connection(
    mut stream: TcpStream,
    route_state: &RouteState,
    request_pool: &PermitPool,
    sse_pool: &PermitPool,
    shutdown: &ShutdownHandle,
    idle_sleep: Duration,
    keep_alive: Duration,
    counters: &AtomicStats,
) {
    let request = match parse(&mut stream) {
        Ok(request) => request,
        Err(ParseFailure::Request(error)) => {
            counters.parse_rejections.fetch_add(1, Ordering::AcqRel);
            let mut response = HttpResponse::json_error(error.status, &error.message);
            if let Some(allow) = error.allow {
                response
                    .headers
                    .push(("Allow".to_string(), allow.to_string()));
            }
            let _ = response.write_to(&mut stream);
            return;
        }
        Err(ParseFailure::Io(_)) => {
            counters.failed_requests.fetch_add(1, Ordering::AcqRel);
            return;
        }
    };
    if request.method == super::request::Method::Get && request.path == "/api/stream" {
        let Some(_permit) = sse_pool.acquire(shutdown, idle_sleep) else {
            return;
        };
        let resume_from = super::sse::resume_cursor(&request);
        match super::sse::serve(&mut stream, route_state, keep_alive, resume_from, shutdown) {
            Ok(()) => counters.served.fetch_add(1, Ordering::AcqRel),
            Err(_) => counters.failed_requests.fetch_add(1, Ordering::AcqRel),
        };
        return;
    }
    let Some(_permit) = request_pool.acquire(shutdown, idle_sleep) else {
        return;
    };
    match route(&request, route_state) {
        Ok(response) => {
            if response.status >= 400 {
                counters.failed_requests.fetch_add(1, Ordering::AcqRel);
            } else {
                counters.served.fetch_add(1, Ordering::AcqRel);
            }
            let _ = response.write_to(&mut stream);
        }
        Err(error) => {
            counters.failed_requests.fetch_add(1, Ordering::AcqRel);
            let _ = HttpResponse::json_error(500, &error.to_string()).write_to(&mut stream);
        }
    }
}

fn prune_workers(workers: &mut Vec<JoinHandle<()>>) {
    let mut remaining = Vec::new();
    for worker in workers.drain(..) {
        if worker.is_finished() {
            let _ = worker.join();
        } else {
            remaining.push(worker);
        }
    }
    *workers = remaining;
}

struct ConnectionPermit {
    release: Option<flume::Sender<()>>,
}

impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
    }
}

struct PermitPool {
    tx: flume::Sender<()>,
    rx: flume::Receiver<()>,
}

impl PermitPool {
    fn new(count: usize) -> Self {
        let (tx, rx) = flume::bounded(count);
        for _ in 0..count {
            let _ = tx.send(());
        }
        Self { tx, rx }
    }

    fn acquire(&self, shutdown: &ShutdownHandle, idle_sleep: Duration) -> Option<ConnectionPermit> {
        loop {
            if shutdown.is_shutdown() {
                return None;
            }
            match self.rx.recv_timeout(idle_sleep) {
                Ok(()) => {
                    return Some(ConnectionPermit {
                        release: Some(self.tx.clone()),
                    });
                }
                Err(flume::RecvTimeoutError::Timeout) => {}
                Err(flume::RecvTimeoutError::Disconnected) => return None,
            }
        }
    }
}

fn surface_error(error: impl std::fmt::Display) -> TexoError {
    TexoError::Surface {
        which: SurfaceKind::Http,
        detail: error.to_string(),
    }
}
