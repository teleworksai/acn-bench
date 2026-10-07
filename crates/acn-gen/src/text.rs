//! Synthetic text (SPEC 050 GEN-13): words of four lowercase ASCII letters, so
//! that *n* tokens of text are 4*n* content bytes (MLM-11), none of which
//! JSON escapes.

use acn_harness::agent::below;
use rand_chacha::ChaCha20Rng;

/// The letters of a word.
const LETTERS: &[u8; 26] = b"abcdefghijklmnopqrstuvwxyz";

/// `tokens` words drawn from `rng`, 4 bytes each, with no separator.
pub fn words(rng: &mut ChaCha20Rng, tokens: u64) -> String {
    let n = usize::try_from(tokens.saturating_mul(4)).unwrap_or(usize::MAX);
    let mut s = String::with_capacity(n);
    for _ in 0..n {
        let i = usize::try_from(below(rng, 26)).unwrap_or(0);
        s.push(char::from(LETTERS.get(i).copied().unwrap_or(b'a')));
    }
    s
}
