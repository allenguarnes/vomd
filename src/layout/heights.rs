//! Block heights with prefix sums (a Fenwick tree): update one height, read a block's top,
//! and find the block at a y, all in O(log n). Measuring a block must not touch the position of
//! every block after it, or measuring a document in small idle slices would be quadratic.

#[cfg(test)]
use std::cell::Cell;

/// Sums are kept in f64: heights are f32, so their sums stay exact (or nearly), and the tops
/// handed out are the f32 roundings of them.
pub(super) struct Heights {
    /// 1-based Fenwick tree: `tree[i]` holds the sum of the `i & -i` heights ending at `i`.
    tree: Vec<f64>,
    /// Tree nodes visited so far (for tests of the cost).
    #[cfg(test)]
    pub ops: Cell<usize>,
}

impl Heights {
    /// A tree over `heights`, built in linear time.
    pub fn new(heights: impl Iterator<Item = f32>) -> Heights {
        let mut tree = vec![0.0];
        tree.extend(heights.map(f64::from));
        for i in 1..tree.len() {
            let parent = i + (i & i.wrapping_neg());
            if parent < tree.len() {
                tree[parent] += tree[i];
            }
        }
        Heights {
            tree,
            #[cfg(test)]
            ops: Cell::new(0),
        }
    }

    pub fn len(&self) -> usize {
        self.tree.len() - 1
    }

    /// Add `delta` to the height of block `i`.
    pub fn add(&mut self, i: usize, delta: f64) {
        let mut at = i + 1;
        while at < self.tree.len() {
            self.tree[at] += delta;
            at += at & at.wrapping_neg();
            self.count(1);
        }
    }

    /// Sum of the first `count` heights: the top of block `count`, the total for `len()`.
    pub fn prefix(&self, count: usize) -> f64 {
        let mut sum = 0.0;
        let mut at = count;
        while at > 0 {
            sum += self.tree[at];
            at &= at - 1;
            self.count(1);
        }
        sum
    }

    /// The most blocks whose heights add up to at most `y` (less than `y` if `strict`).
    pub fn blocks_within(&self, y: f64, strict: bool) -> usize {
        let n = self.len();
        if n == 0 {
            return 0;
        }
        let (mut pos, mut rest) = (0, y);
        let mut step = 1 << n.ilog2();
        while step > 0 {
            let next = pos + step;
            if next <= n {
                let sum = self.tree[next];
                if if strict { sum < rest } else { sum <= rest } {
                    pos = next;
                    rest -= sum;
                }
            }
            step >>= 1;
            self.count(1);
        }
        pos
    }

    #[cfg(test)]
    fn count(&self, n: usize) {
        self.ops.set(self.ops.get() + n);
    }

    #[cfg(not(test))]
    #[inline(always)]
    fn count(&self, _: usize) {}
}
