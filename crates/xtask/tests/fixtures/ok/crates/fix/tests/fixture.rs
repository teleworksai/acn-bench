/// Cites: FIX-1,
///   FIX-2
#[test]
fn fixture_is_cited() {}

#[tokio::test(
    start_paused = true
)]
/// Cites: FIX-1
async fn attribute_before_doc_and_multiline() {}

/// Cites: FIX-2
#[test] // trailing comment
fn attribute_with_trailing_comment() {}

/// Cites: FIX-2
#[test] fn same_line() {}
