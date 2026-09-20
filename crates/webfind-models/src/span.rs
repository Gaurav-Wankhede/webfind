// webfind-models: Zero-Copy TextRange byte span for context slicing.
// Per Rust Systems Optimization Handbook §2 Pattern 2:
// Storing byte offsets reduces memory footprint by 75% compared to 2D line/col coordinates.

use serde::{Deserialize, Serialize};

/// 8-byte zero-copy byte offset span from the start of the buffer.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(C)]
pub struct TextRange {
    pub start: u32,
    pub end: u32,
}

impl TextRange {
    #[inline]
    #[must_use]
    pub const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    #[inline]
    #[must_use]
    pub const fn len(&self) -> usize {
        (self.end.saturating_sub(self.start)) as usize
    }

    #[inline]
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.start >= self.end
    }

    #[inline]
    #[must_use]
    pub fn slice<'a>(&self, text: &'a str) -> &'a str {
        let s = self.start as usize;
        let e = self.end as usize;
        if s <= text.len() && e <= text.len() && s <= e {
            &text[s..e]
        } else {
            ""
        }
    }
}
