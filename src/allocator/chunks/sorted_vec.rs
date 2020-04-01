use alloc_collections::{raw_vec, Alloc, Vec};
use std::ops::Deref;
use superslice::Ext;

pub struct FixesSortedVec<T, A: Alloc>(Vec<T, A>);

impl<T, A> FixesSortedVec<T, A>
where
    T: Ord,
    A: Alloc,
{
    pub fn new(capacity: usize, allocator: A) -> Result<Self, raw_vec::Error> {
        Vec::with_capacity_in(capacity, allocator).map(Self)
    }

    pub fn push(&mut self, item: T) {
        assert_ne!(self.0.capacity(), self.0.len());
        let idx = self.0.upper_bound(&item);
        self.0
            .insert(idx, item)
            .expect("No allocation => no failure")
    }

    pub fn pop(&mut self, index: usize) -> T {
        self.0.remove(index)
    }
}

impl<T, A: Alloc> Deref for FixesSortedVec<T, A> {
    type Target = [T];
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use alloc_collections::alloc;
    use quickcheck_macros::quickcheck;

    #[quickcheck]
    fn check(original: std::vec::Vec<usize>) {
        crate::init_logging();

        let mut v = FixesSortedVec::<usize, _>::new(original.len(), alloc::Global).unwrap();
        for item in &original {
            v.push(*item);
        }

        let mut original = original;
        original.sort();
        assert_eq!(&v[..], &original[..]);
    }
}
