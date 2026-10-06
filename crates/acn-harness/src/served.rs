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

/// One replicate's mock, served until dropped.
pub struct ServedMock {
    addr: SocketAddr,
    task: tokio::task::JoinHandle<()>,
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
        let task = tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, router).await {
                tracing::error!(error = %e, "served mock");
            }
        });
        Ok(Self { addr, task })
    }

    /// The base URL the replicate's calls go to.
    #[must_use]
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

impl Drop for ServedMock {
    /// The replicate is over: stop listening (HAR-26).
    fn drop(&mut self) {
        self.task.abort();
    }
}
