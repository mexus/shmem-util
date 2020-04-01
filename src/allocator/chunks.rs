use super::free_chunk::{FreeSpace, RelativeBeginning};
use crate::memmap::MemmapAlloc;
use superslice::Ext;

mod sorted_vec;
use sorted_vec::FixesSortedVec;

/// Free chunks manager.
pub struct Chunks {
    /// Sorted list of free chunks.
    free_chunks: FixesSortedVec<FreeSpace, MemmapAlloc>,
}

impl Chunks {
    /// Size of a single memory chunk.
    pub const CHUNK_SIZE: usize = super::ShmemAlloc::ALIGNMENT;

    /// Initializes a new chunks manager.
    pub fn new(max_chunks: usize) -> Result<Self, alloc_collections::raw_vec::Error> {
        let mut free_chunks = FixesSortedVec::new(max_chunks, MemmapAlloc)?;
        free_chunks.push(FreeSpace::new(RelativeBeginning::zero(), max_chunks));
        Ok(Self { free_chunks })
    }

    /// Extracts free space of a given size in bytes, if possible.
    ///
    /// If there is no enough contiguous free space available, returns `None`.
    pub fn get(&mut self, size: usize) -> Option<FreeSpace> {
        let required_chunks = if size % Self::CHUNK_SIZE == 0 {
            size / Self::CHUNK_SIZE
        } else {
            (size / Self::CHUNK_SIZE).checked_add(1)?
        };
        let idx = self
            .free_chunks
            .lower_bound_by(|free_space| free_space.count().cmp(&required_chunks));
        if idx >= self.free_chunks.len() {
            return None;
        }
        let space = self.free_chunks.pop(idx);

        let (space, unused) = space.split(required_chunks);
        if let Some(unused) = unused {
            self.insert_chunk_and_merge(unused);
        }

        Some(space)
    }

    fn insert_chunk_and_merge(&mut self, chunk: FreeSpace) {
        if self.free_chunks.is_empty() {
            // 1. If there are no chunks.
            self.free_chunks.push(chunk);
            return;
        }
        // 2. Check if we can merge anything.
        let mut chunk = chunk;
        while let Some((id, merged)) = self.find_and_merge(chunk) {
            self.free_chunks.pop(id);
            chunk = merged;
        }

        // 3. Push the chunk to the sorted vector.
        self.free_chunks.push(chunk);

        #[cfg(test)]
        self.self_check();
    }

    fn find_and_merge(&self, chunk: FreeSpace) -> Option<(usize, FreeSpace)> {
        self.free_chunks
            .iter()
            .enumerate()
            .filter_map(|(id, existing_chunk)| {
                existing_chunk.merge(chunk).map(|merged| (id, merged))
            })
            .next()
    }

    /// Returns free space of `size` (in bytes) back, which begins at `shift` (in bytes).
    pub fn return_back(&mut self, shift: usize, size: usize) {
        let count = if size % Self::CHUNK_SIZE == 0 {
            size / Self::CHUNK_SIZE
        } else {
            (size / Self::CHUNK_SIZE)
                .checked_add(1)
                .expect("Should be ok")
        };
        assert_eq!(shift % Self::CHUNK_SIZE, 0);
        let shift = RelativeBeginning::from_bytes(shift);
        self.insert_chunk_and_merge(FreeSpace::new(shift, count))
    }

    #[cfg(test)]
    /// Returns how much space in available, in bytes.
    fn free_space(&self) -> usize {
        self.free_chunks.last().map(FreeSpace::count).unwrap_or(0) * Self::CHUNK_SIZE
    }

    #[cfg(test)]
    fn self_check(&self) {
        if self.free_chunks.is_empty() {
            return;
        }
        for idx in 0..self.free_chunks.len() - 1 {
            let current = &self.free_chunks[idx];
            let next = &self.free_chunks[idx + 1];
            assert!(current.count() <= next.count());
        }

        for i in 0..self.free_chunks.len() {
            for j in i + 1..self.free_chunks.len() {
                let left = &self.free_chunks[i];
                let right = &self.free_chunks[j];

                assert_ne!(left, right, "i = {}, j = {}", i, j);
                assert!(
                    !FreeSpace::are_contiguous(left, right),
                    "{:?}, {:?}",
                    left,
                    right
                );
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use rand::Rng;

    #[test]
    fn initial() {
        crate::init_logging();

        let chunks = Chunks::new(10).unwrap();
        assert_eq!(chunks.free_chunks.len(), 1);
        assert_eq!(chunks.free_space(), 10 * Chunks::CHUNK_SIZE);
    }

    #[test]
    fn find_free() {
        crate::init_logging();

        let max_chunks = 10;
        let size = max_chunks * Chunks::CHUNK_SIZE;

        for size in 1..size {
            let mut chunks = Chunks::new(max_chunks).unwrap();
            let free_chunk = chunks
                .get(size)
                .unwrap_or_else(|| panic!("size = {}", size));
            assert!(
                free_chunk.count() * Chunks::CHUNK_SIZE >= size,
                "size = {}",
                size
            );
            chunks.self_check();
        }
    }

    #[test]
    fn random_actions() {
        crate::init_logging();
        const ITERATIONS: usize = 100_000;

        let max_chunks = 1000;
        let mut chunks = Chunks::new(max_chunks).unwrap();
        let mut rng = rand::thread_rng();

        let mut taken_away = std::vec::Vec::new();
        for _ in 0..ITERATIONS {
            if rng.gen_bool(0.5) {
                // Take.
                let size: usize = rng.gen_range(0, max_chunks * Chunks::CHUNK_SIZE);
                let free_space = chunks.free_space();
                let maybe_chunk = chunks.get(size);
                if size <= free_space {
                    taken_away.push(maybe_chunk.unwrap());
                    chunks.self_check();
                } else {
                    assert!(
                        maybe_chunk.is_none(),
                        "size = {}, free = {}, chunk = {:?}",
                        size,
                        free_space,
                        maybe_chunk
                    );
                }
            } else {
                // Return back.
                if taken_away.is_empty() {
                    continue;
                }
                let n = rng.gen_range(0, taken_away.len());
                let chunk = taken_away.remove(n);
                chunks.insert_chunk_and_merge(chunk);
                chunks.self_check();
            }
        }
    }
}
