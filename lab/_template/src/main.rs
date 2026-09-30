//! Lab spike template (CON-23). Copy the directory, rename the package, and
//! write `docs/lab/<yyyy-mm-dd>-<slug>.md` when you are done.
#![forbid(unsafe_code)] // CON-19 still binds lab crates: CON-23 exempts CON-4 to CON-18 only

fn main() {
    // Ambient clocks are fine in lab/: lab/clippy.toml carries no determinism bans.
    let started = std::time::Instant::now();
    println!("lab spike ran in {:?}", started.elapsed());
}
