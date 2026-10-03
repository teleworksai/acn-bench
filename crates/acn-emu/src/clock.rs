//! The run's clock (CON-5(b)). Every component reads time through a [`Clock`]:
//! a [`SimClock`] in `sim`, whose time moves only when something waits on it, and
//! a [`WallClock`] in `live`, monotonic from the run's start. This module is the
//! one place the process's own clock is read (ADR-8, ADR-16). Times are integer
//! nanoseconds from the run's origin.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicI64, Ordering};

/// A future returned by [`Clock::sleep_until`].
pub type Sleep<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

/// The run's clock.
pub trait Clock: Send + Sync + std::fmt::Debug {
    /// Nanoseconds since the run's origin.
    fn now_ns(&self) -> i64;
    /// Resolve once the clock reads at least `t_ns`.
    fn sleep_until(&self, t_ns: i64) -> Sleep<'_>;
}

/// Virtual time (`sim`). It starts at 0 and only moves forward: waiting on it
/// advances it to the time waited for, at once, so a run takes no wall time and a
/// sequence of waits gives the same readings on every machine.
#[derive(Debug, Default)]
pub struct SimClock {
    now: AtomicI64,
}

impl SimClock {
    /// A clock at 0.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Move the clock to `t_ns` if that is later than now.
    pub fn advance_to(&self, t_ns: i64) {
        self.now.fetch_max(t_ns, Ordering::SeqCst);
    }
}

impl Clock for SimClock {
    fn now_ns(&self) -> i64 {
        self.now.load(Ordering::SeqCst)
    }

    fn sleep_until(&self, t_ns: i64) -> Sleep<'_> {
        self.advance_to(t_ns);
        Box::pin(std::future::ready(()))
    }
}

/// Wall time (`live`): monotonic nanoseconds since the clock was made, which is
/// the run's start (TRC-26). Sleeping waits on tokio's timer.
#[derive(Debug)]
pub struct WallClock {
    origin: tokio::time::Instant,
}

impl WallClock {
    /// A clock whose origin is now.
    #[must_use]
    #[allow(clippy::disallowed_methods)] // CON-5(b): the one sanctioned read of the process clock
    pub fn start() -> Self {
        Self {
            origin: tokio::time::Instant::now(),
        }
    }
}

impl Clock for WallClock {
    #[allow(clippy::disallowed_methods)] // CON-5(b): the one sanctioned read of the process clock
    fn now_ns(&self) -> i64 {
        let elapsed = tokio::time::Instant::now().saturating_duration_since(self.origin);
        i64::try_from(elapsed.as_nanos()).unwrap_or(i64::MAX)
    }

    fn sleep_until(&self, t_ns: i64) -> Sleep<'_> {
        let at = self.origin + std::time::Duration::from_nanos(u64::try_from(t_ns).unwrap_or(0));
        Box::pin(tokio::time::sleep_until(at))
    }
}
