//! The joined view of specs, citations and scope that `trace-check` and
//! `docs-inventory` both consume.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::Result;
use crate::citations::{self, Citation, Problem};
use crate::scope;
use crate::specs::{self, Requirement};

/// Requirements, citations and the implemented set for one workspace root.
#[derive(Debug)]
pub struct Model {
    pub spec_count: usize,
    pub requirements: Vec<Requirement>,
    pub citations: Vec<Citation>,
    pub problems: Vec<Problem>,
    pub files_scanned: usize,
    pub implemented: BTreeSet<String>,
    pub scope_errors: Vec<String>,
}

impl Model {
    /// Build the model for `root`.
    pub fn load(root: &Path) -> Result<Self> {
        let spec_count = specs::spec_files(root)?.len();
        let requirements = specs::parse_specs(root)?;
        let scan = citations::scan(root)?;
        let scope_file = scope::load(root)?;
        let (implemented, scope_errors) = scope::resolve(&scope_file, &requirements);
        Ok(Self {
            spec_count,
            requirements,
            citations: scan.citations,
            problems: scan.problems,
            files_scanned: scan.files_scanned,
            implemented,
            scope_errors,
        })
    }

    /// Citations grouped by ID, in scan order.
    pub fn citations_by_id(&self) -> BTreeMap<&str, Vec<&Citation>> {
        let mut map: BTreeMap<&str, Vec<&Citation>> = BTreeMap::new();
        for c in &self.citations {
            map.entry(c.id.as_str()).or_default().push(c);
        }
        map
    }

    /// Citations whose ID no spec defines.
    pub fn unknown_citations(&self) -> Vec<&Citation> {
        let known: BTreeSet<&str> = self.requirements.iter().map(|r| r.id.as_str()).collect();
        self.citations
            .iter()
            .filter(|c| !known.contains(c.id.as_str()))
            .collect()
    }
}
