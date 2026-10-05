//! The three cache models (MLM-20..23). A cache entry is named by the BLAKE3 of its
//! tenant and its prefix bytes, so equal prefixes of one tenant are one entry and
//! tenants never share. Prefix hashes are computed incrementally, one pass over
//! the prompt for any number of candidate lengths.

use std::collections::BTreeMap;

use crate::profile::{CacheModel, Profile};
use crate::prompt::Prompt;

type Hash = [u8; 32];

/// What a request read from and wrote to the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Accounting {
    pub cached_tokens: u64,
    pub cache_write_tokens: u64,
}

#[derive(Debug, Clone)]
struct Block {
    last_use: i64,
    parent: Option<Hash>,
    children: u64,
}

/// The cache of one profile in one mock instance (MLM-8: it starts empty).
#[derive(Debug, Clone, Default)]
pub struct Cache {
    /// Prefix entries (explicit and automatic models): hash → last use.
    prefixes: BTreeMap<Hash, i64>,
    /// Blocks (block-granular model).
    blocks: BTreeMap<Hash, Block>,
}

/// The hashes of the prefixes of `bytes` that end at `ends` (ascending byte
/// lengths), salted with the tenant.
fn prefix_hashes(tenant: &str, bytes: &[u8], ends: &[usize]) -> Vec<Hash> {
    let mut h = blake3::Hasher::new();
    h.update(&(tenant.len() as u64).to_le_bytes());
    h.update(tenant.as_bytes());
    let mut fed = 0usize;
    let mut out = Vec::with_capacity(ends.len());
    for &end in ends {
        let end = end.min(bytes.len());
        if end > fed {
            h.update(&bytes[fed..end]);
            fed = end;
        }
        out.push(*h.clone().finalize().as_bytes());
    }
    out
}

impl Cache {
    /// The number of stored prefix entries and blocks (for tests and audits).
    #[must_use]
    pub fn sizes(&self) -> (usize, usize) {
        (self.prefixes.len(), self.blocks.len())
    }

    fn expire(&mut self, profile: &Profile, now: i64) {
        let alive = |last: i64| now.saturating_sub(last) <= profile.ttl_ns;
        self.prefixes.retain(|_, last| alive(*last));
        // A parent is touched whenever a child is, so it never expires before
        // one: removing every expired block keeps the store prefix-closed.
        let expired: Vec<Hash> = self
            .blocks
            .iter()
            .filter(|(_, b)| !alive(b.last_use))
            .map(|(h, _)| *h)
            .collect();
        for h in expired {
            self.remove_block(&h);
        }
    }

    fn remove_block(&mut self, h: &Hash) {
        if let Some(b) = self.blocks.remove(h)
            && let Some(p) = b.parent
            && let Some(pb) = self.blocks.get_mut(&p)
        {
            pb.children = pb.children.saturating_sub(1);
        }
    }

    /// Look the prompt up and store what the model stores (MLM-20..23). The
    /// breakpoint count was checked when the prompt was built (MLM-21).
    pub fn account(
        &mut self,
        profile: &Profile,
        tenant: &str,
        prompt: &Prompt,
        now: i64,
    ) -> Accounting {
        self.expire(profile, now);
        let tokens = prompt.tokens();
        match profile.cache_model {
            CacheModel::ExplicitBreakpoints => {
                // A breakpoint at byte e marks the prefix of ⌊e/4⌋ tokens (ADR-16).
                let mut lengths: Vec<u64> = prompt
                    .breakpoints
                    .iter()
                    .map(|end| (*end as u64) / 4)
                    .filter(|n| *n >= profile.min_cacheable_tokens && *n > 0)
                    .collect();
                lengths.sort_unstable();
                lengths.dedup();
                let ends: Vec<usize> = lengths.iter().map(|n| (*n as usize) * 4).collect();
                let hashes = prefix_hashes(tenant, &prompt.bytes, &ends);
                let mut read = 0;
                let mut write = 0;
                for (n, h) in lengths.iter().zip(&hashes) {
                    if self.prefixes.contains_key(h) {
                        read = read.max(*n);
                    } else {
                        write = write.max(*n);
                    }
                    self.prefixes.insert(*h, now);
                }
                // MLM-21: tokens written beyond the read, as Anthropic counts
                // them, so that uncached, read and written partition the prompt.
                Accounting {
                    cached_tokens: read,
                    cache_write_tokens: write.saturating_sub(read),
                }
            }
            CacheModel::AutomaticPrefix => {
                let mut lengths = Vec::new();
                let mut n = profile.min_cacheable_tokens.max(1);
                while n <= tokens {
                    lengths.push(n);
                    n += profile.increment_tokens;
                }
                let ends: Vec<usize> = lengths.iter().map(|n| (*n as usize) * 4).collect();
                let hashes = prefix_hashes(tenant, &prompt.bytes, &ends);
                let mut read = 0;
                for (n, h) in lengths.iter().zip(&hashes) {
                    if self.prefixes.contains_key(h) {
                        read = read.max(*n);
                    }
                    self.prefixes.insert(*h, now);
                }
                Accounting {
                    cached_tokens: read,
                    cache_write_tokens: 0,
                }
            }
            CacheModel::BlockGranular => {
                let full = tokens / profile.block_tokens;
                let ends: Vec<usize> = (1..=full)
                    .map(|j| (j * profile.block_tokens) as usize * 4)
                    .collect();
                let hashes = prefix_hashes(tenant, &prompt.bytes, &ends);
                let mut leading = 0u64;
                let mut counting = true;
                let mut written = 0u64;
                let mut parent: Option<Hash> = None;
                for h in &hashes {
                    if let Some(b) = self.blocks.get_mut(h) {
                        b.last_use = now;
                        if counting {
                            leading += 1;
                        }
                    } else {
                        counting = false;
                        written += 1;
                        self.blocks.insert(
                            *h,
                            Block {
                                last_use: now,
                                parent,
                                children: 0,
                            },
                        );
                        if let Some(p) = parent
                            && let Some(pb) = self.blocks.get_mut(&p)
                        {
                            pb.children += 1;
                        }
                    }
                    parent = Some(*h);
                }
                self.evict(profile);
                Accounting {
                    cached_tokens: leading * profile.block_tokens,
                    cache_write_tokens: written * profile.block_tokens,
                }
            }
        }
    }

    /// Evict leaves (no stored block extends them) with the earliest last use,
    /// ties by the lowest hash, until the store fits (MLM-23).
    fn evict(&mut self, profile: &Profile) {
        while self.blocks.len() as u64 > profile.capacity_blocks {
            let victim = self
                .blocks
                .iter()
                .filter(|(_, b)| b.children == 0)
                .min_by(|(ha, a), (hb, b)| a.last_use.cmp(&b.last_use).then(ha.cmp(hb)))
                .map(|(h, _)| *h);
            match victim {
                Some(h) => self.remove_block(&h),
                None => break,
            }
        }
    }
}
