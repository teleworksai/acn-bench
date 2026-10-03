//! CON-5(b): time comes from an injected clock; virtual time moves only when
//! waited on, wall time is monotonic from the run's start.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use acn_emu::clock::{Clock, SimClock, WallClock};

/// Cites: CON-5
#[test]
fn virtual_time_starts_at_zero_and_moves_only_forward_when_waited_on() {
    let c = SimClock::new();
    assert_eq!(c.now_ns(), 0);
    futures_block(c.sleep_until(1_500));
    assert_eq!(c.now_ns(), 1_500, "waiting advances the clock at once");
    futures_block(c.sleep_until(1_000));
    assert_eq!(c.now_ns(), 1_500, "never backwards");
    c.advance_to(2_000);
    assert_eq!(c.now_ns(), 2_000);
}

/// Cites: CON-5, TRC-26
#[tokio::test(start_paused = true)]
async fn wall_time_is_monotonic_from_the_clocks_start() {
    let c = WallClock::start();
    let a = c.now_ns();
    c.sleep_until(5_000_000).await;
    let b = c.now_ns();
    assert!(a >= 0 && b >= 5_000_000, "{a} {b}");
}

fn futures_block(f: acn_emu::clock::Sleep<'_>) {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(f);
}
