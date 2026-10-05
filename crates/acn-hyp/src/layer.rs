//! The layer of an evidence object (LOOP-1): derived from what the object
//! records, never asserted beside it.

use acn_trace::bundle::{MOCK_BACKEND, Manifest};
use acn_trace::identity::Mode;

use crate::verdict::{Label, Verdict};

/// The layers of SPEC 085 §1 that produce evidence objects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Layer {
    /// `sim`, always on the mock.
    L1,
    /// `live` on the mock: the simulator's twin.
    L2,
    /// `live` on a real provider, and `netem` on either backend.
    L3,
}

impl Layer {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::L1 => "L1",
            Self::L2 => "L2",
            Self::L3 => "L3",
        }
    }

    fn of(mode: Mode, mock: bool) -> Self {
        match (mode, mock) {
            (Mode::Sim, _) => Self::L1,
            (Mode::Live, true) => Self::L2,
            (Mode::Live, false) | (Mode::Netem, _) => Self::L3,
        }
    }
}

/// A bundle's layer, fixed by its manifest's mode and backend.
pub fn of_bundle(m: &Manifest) -> Result<Layer, String> {
    let mode = Mode::parse(&m.mode).map_err(|e| e.to_string())?;
    let mock = m.backend == MOCK_BACKEND;
    if mode == Mode::Sim && !mock {
        return Err(format!(
            "{}: a sim bundle on `{}`; sim runs only on the mock",
            m.run_id, m.backend
        ));
    }
    Ok(Layer::of(mode, mock))
}

/// A verdict's layer: the highest among its bundles, so sim bundles with their
/// live twins make an L2 verdict. A verdict's set is all mock or all real
/// (HYP-20); the mock one carries the `mock-gated` label (HYP-23).
#[must_use]
pub fn of_verdict(v: &Verdict) -> Layer {
    let mock = v.labels.contains(&Label::MockGated);
    v.bundles
        .iter()
        .map(|(_, _, mode)| Layer::of(*mode, mock))
        .max()
        .unwrap_or(Layer::L1)
}

/// A loop report's layer: `acn loop run` executes L1 only (LOOP-10, LOOP-12).
pub const REPORT: Layer = Layer::L1;
