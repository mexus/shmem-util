use super::chunks::Chunks;
use std::{cmp::Ordering, ops::Add, ptr::NonNull};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct FreeSpace {
    /// Shift from the beginning of the memory (in chunks).
    relative_beginning: RelativeBeginning,

    /// Amount of elementary chunks.
    count: usize,
}

/// Relative beginning.
#[derive(Debug, Clone, Copy, Eq, PartialEq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct RelativeBeginning(usize);

impl Add<usize> for RelativeBeginning {
    type Output = RelativeBeginning;
    fn add(self, rhs: usize) -> Self::Output {
        Self(self.0 + rhs)
    }
}

impl RelativeBeginning {
    pub const fn zero() -> Self {
        Self(0)
    }

    pub const fn from_bytes(bytes: usize) -> Self {
        Self(bytes / Chunks::CHUNK_SIZE)
    }

    pub fn into_absolute(self, base: NonNull<u8>) -> NonNull<u8> {
        let ptr = base.as_ptr() as usize + self.0 * Chunks::CHUNK_SIZE;
        unsafe { NonNull::new_unchecked(ptr as *mut u8) }
    }
}

impl FreeSpace {
    pub fn new(shift: RelativeBeginning, count: usize) -> Self {
        debug_assert_ne!(count, 0);
        Self {
            count,
            relative_beginning: shift,
        }
    }

    /// Where the space begins.
    pub fn begin(&self) -> RelativeBeginning {
        self.relative_beginning
    }

    /// Amount of elementary chunks.
    pub fn count(&self) -> usize {
        self.count
    }

    /// Checks whether `left` and `right` chunks are adjacent.
    pub fn are_contiguous(left: &Self, right: &Self) -> bool {
        left.relative_beginning + left.count == right.relative_beginning
    }

    /// Splits the current free space into required and optional unused.
    pub fn split(self, required_chunks: usize) -> (Self, Option<FreeSpace>) {
        let unused_chunks = self
            .count
            .checked_sub(required_chunks)
            .expect("Not enough chunks at the first place");
        if unused_chunks == 0 {
            (self, None)
        } else {
            let current = FreeSpace {
                count: required_chunks,
                relative_beginning: self.relative_beginning,
            };
            let unused = FreeSpace {
                count: unused_chunks,
                relative_beginning: self.relative_beginning + required_chunks,
            };
            (current, Some(unused))
        }
    }

    /// Merges this space with the adjacent one.
    pub fn merge(self, other: Self) -> Option<Self> {
        if FreeSpace::are_contiguous(&self, &other) {
            Some(Self {
                count: self.count + other.count,
                relative_beginning: self.relative_beginning,
            })
        } else if FreeSpace::are_contiguous(&other, &self) {
            Some(Self {
                count: self.count + other.count,
                relative_beginning: other.relative_beginning,
            })
        } else {
            None
        }
    }
}

impl PartialOrd for FreeSpace {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FreeSpace {
    fn cmp(&self, other: &Self) -> Ordering {
        self.count
            .cmp(&other.count)
            .then_with(|| self.relative_beginning.cmp(&other.relative_beginning))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use quickcheck::TestResult;
    use quickcheck_macros::quickcheck;

    #[quickcheck]
    fn same(begin: usize, count: usize) -> TestResult {
        if count == 0 {
            return TestResult::discard();
        }
        let chunk = FreeSpace::new(RelativeBeginning(begin), count);
        TestResult::from_bool(!FreeSpace::are_contiguous(&chunk, &chunk))
    }

    #[quickcheck]
    fn contiguous(begin: usize, count1: usize) -> TestResult {
        if count1 == 0 {
            return TestResult::discard();
        }
        let chunk1 = FreeSpace::new(RelativeBeginning(begin), count1);
        let chunk2 = FreeSpace::new(RelativeBeginning(begin + count1), 10);
        TestResult::from_bool(FreeSpace::are_contiguous(&chunk1, &chunk2))
    }

    #[quickcheck]
    fn non_contiguous(begin: usize, count: usize, additional_shift: usize) -> TestResult {
        if count == 0 || additional_shift == 0 {
            return TestResult::discard();
        }
        let chunk1 = FreeSpace::new(RelativeBeginning(begin), count);
        let chunk2 = FreeSpace::new(RelativeBeginning(begin + count + additional_shift), 10);
        TestResult::from_bool(!FreeSpace::are_contiguous(&chunk1, &chunk2))
    }

    #[test]
    fn check_contiguous() {
        let first = FreeSpace::new(RelativeBeginning(0), 3);
        let second = FreeSpace::new(RelativeBeginning(3), 10);
        let third = FreeSpace::new(RelativeBeginning(14), 10);

        assert!(FreeSpace::are_contiguous(&first, &second));
        assert!(!FreeSpace::are_contiguous(&second, &third));
    }

    #[quickcheck]
    fn split(count: usize) -> TestResult {
        if count == 0 {
            return TestResult::discard();
        }
        let first = FreeSpace::new(RelativeBeginning(0), count);
        let (new, free) = first.split(count);
        assert_eq!(new, first);
        assert!(free.is_none());

        for required in 1..count {
            let (new, free) = first.split(required);
            let free = free.unwrap();
            assert_eq!(first.relative_beginning, new.relative_beginning);

            assert!(FreeSpace::are_contiguous(&new, &free));
            assert_eq!(new.count + free.count, count);
        }
        TestResult::passed()
    }
}
