//! The mock the harness serves itself (SPEC 040 HAR-26): for the endpoint
//! `acn-mock://loopback`, each `live` replicate gets a fresh mock, built as in
//! `sim` from the run's profiles and the replicate's seed, over HTTP on the
//! loopback interface, on the run's clock.

use std::net::SocketAddr;
use std::sync::Arc;

use acn_emu::clock::Clock;
use acn_mockllm::Mock;

use crate::HarnessError;

/// The endpoint that asks for a served mock (HAR-26).
pub const LOOPBACK: &str = "acn-mock://loopback";

/// The host a served mock's manifest names (HAR-26).
pub const LOOPBACK_HOST: &str = "loopback";

/// One replicate's mock, served until [`ServedMock::shutdown`] (or dropped).
pub struct ServedMock {
    addr: SocketAddr,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<std::io::Result<()>>>,
}

impl ServedMock {
    /// Serve `mock` on `127.0.0.1`, at a port the system picks.
    pub async fn start(mock: Mock, clock: Arc<dyn Clock>) -> Result<Self, HarnessError> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| HarnessError::Backend(format!("serving the mock: {e}")))?;
        let addr = listener
            .local_addr()
            .map_err(|e| HarnessError::Backend(format!("serving the mock: {e}")))?;
        let router = acn_mockllm::server::router(mock, clock);
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = stopped.await;
                })
                .await
        });
        Ok(Self {
            addr,
            stop: Some(stop),
            task: Some(task),
        })
    }

    /// The base URL the replicate's calls go to.
    #[must_use]
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// The replicate is over (HAR-26): stop listening, close idle
    /// connections, and wait for any request still being answered. A server
    /// that failed says so here.
    pub async fn shutdown(mut self) -> Result<(), HarnessError> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let Some(task) = self.task.take() else {
            return Ok(());
        };
        match task.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(HarnessError::Backend(format!("the served mock: {e}"))),
            Err(e) => Err(HarnessError::Internal(format!("the served mock: {e}"))),
        }
    }
}

impl Drop for ServedMock {
    /// A replicate that failed before [`ServedMock::shutdown`]: stop at once.
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn mock() -> Mock {
        Mock::with_profiles(acn_mockllm::profile::embedded().unwrap(), 7).unwrap()
    }

    /// Cites: HAR-26
    #[tokio::test]
    async fn a_served_mock_stops_listening_when_shut_down_or_dropped() {
        let clock: Arc<dyn Clock> = Arc::new(acn_emu::clock::SimClock::new());
        let s = ServedMock::start(mock(), Arc::clone(&clock)).await.unwrap();
        let addr = s.addr;
        assert!(tokio::net::TcpStream::connect(addr).await.is_ok());
        s.shutdown().await.unwrap();
        assert!(tokio::net::TcpStream::connect(addr).await.is_err());

        let s = ServedMock::start(mock(), clock).await.unwrap();
        let addr = s.addr;
        drop(s);
        let mut closed = false;
        for _ in 0..100 {
            if tokio::net::TcpStream::connect(addr).await.is_err() {
                closed = true;
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(closed, "still listening");
    }
}
