/// Cites: FIX-1
fn helper_that_is_not_a_test() {}

#[cfg(test)]
/// Cites: FIX-1
fn cfg_test_helper_is_not_a_test_either() {}
