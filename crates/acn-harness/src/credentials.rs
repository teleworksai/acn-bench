//! Provider credentials (HAR-22): read from the environment, sent only in the
//! authentication header, never logged. Reading them reaches nothing; only a build
//! with `real-api` uses them for a request (HAR-20).

use crate::HarnessError;
use crate::wire::Backend;

/// The pinned Anthropic API version (HAR-21).
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// The headers that authenticate a request to `backend`, from the environment.
pub fn headers(backend: Backend) -> Result<Vec<(String, String)>, HarnessError> {
    headers_with(backend, |n| std::env::var(n).ok())
}

/// [`headers`] with the variables read through `lookup`. An error names the
/// variable, never a value.
pub fn headers_with(
    backend: Backend,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<Vec<(String, String)>, HarnessError> {
    let var = |name: &str, required: bool| match lookup(name) {
        Some(v) if !v.is_empty() => Ok(Some(v)),
        _ if required => Err(HarnessError::Config(format!(
            "{name} is not set; credentials come from the environment (HAR-22)"
        ))),
        _ => Ok(None),
    };
    let bearer = |k: Option<String>| {
        k.map(|k| vec![("authorization".to_owned(), format!("Bearer {k}"))])
            .unwrap_or_default()
    };
    Ok(match backend {
        Backend::Openai => bearer(var("OPENAI_API_KEY", true)?),
        Backend::Vllm => bearer(var("VLLM_API_KEY", false)?),
        Backend::Sglang => bearer(var("SGLANG_API_KEY", false)?),
        Backend::Anthropic => {
            let key = var("ANTHROPIC_API_KEY", true)?.unwrap_or_default();
            vec![
                ("x-api-key".to_owned(), key),
                ("anthropic-version".to_owned(), ANTHROPIC_VERSION.to_owned()),
            ]
        }
        Backend::Mockllm => Vec::new(),
    })
}
