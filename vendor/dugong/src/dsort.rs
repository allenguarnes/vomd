//! VOMD: one stable sort per element type (size). Every `sort_by`/`sort_by_key` call with its
//! own closure is a copy of std's sort, about 3.5 KB each; these hand the comparison to one
//! `sort_by` per element type through `&mut dyn FnMut`. The same std sort on the same elements
//! with the same comparisons, so the same order.

use std::cmp::Ordering;

pub(crate) trait DSort<T> {
    /// [`slice::sort_by`].
    fn dsort_by(&mut self, compare: impl FnMut(&T, &T) -> Ordering);
    /// [`slice::sort_by_key`].
    fn dsort_by_key<K: Ord>(&mut self, key: impl FnMut(&T) -> K);
}

impl<T> DSort<T> for [T] {
    fn dsort_by(&mut self, mut compare: impl FnMut(&T, &T) -> Ordering) {
        sort(self, &mut compare);
    }

    fn dsort_by_key<K: Ord>(&mut self, mut key: impl FnMut(&T) -> K) {
        sort(self, &mut |a, b| key(a).cmp(&key(b)));
    }
}

#[inline(never)]
fn sort<T>(v: &mut [T], compare: &mut dyn FnMut(&T, &T) -> Ordering) {
    v.sort_by(|a, b| compare(a, b));
}
