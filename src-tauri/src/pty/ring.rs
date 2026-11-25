//! Fixed-capacity byte ring used to replay terminal history when a view remounts.

use std::collections::VecDeque;

/// Per-session replay capacity (256 KB, per HANDOFF §4.2).
pub const RING_CAPACITY: usize = 256 * 1024;

pub struct RingBuffer {
    data: VecDeque<u8>,
    capacity: usize,
}

impl RingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            data: VecDeque::with_capacity(capacity.min(64 * 1024)),
            capacity,
        }
    }

    /// Appends bytes, evicting the oldest ones once capacity is exceeded.
    pub fn push(&mut self, bytes: &[u8]) {
        if bytes.len() >= self.capacity {
            self.data.clear();
            self.data.extend(&bytes[bytes.len() - self.capacity..]);
            return;
        }
        let overflow = (self.data.len() + bytes.len()).saturating_sub(self.capacity);
        self.data.drain(..overflow);
        self.data.extend(bytes);
    }

    pub fn snapshot(&self) -> Vec<u8> {
        self.data.iter().copied().collect()
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_oldest_bytes_when_full() {
        let mut ring = RingBuffer::new(8);
        ring.push(b"abcdef");
        ring.push(b"ghij");
        assert_eq!(ring.snapshot(), b"cdefghij");
    }

    #[test]
    fn oversized_write_keeps_only_tail() {
        let mut ring = RingBuffer::new(4);
        ring.push(b"0123456789");
        assert_eq!(ring.snapshot(), b"6789");
    }

    #[test]
    fn respects_capacity_across_many_writes() {
        let mut ring = RingBuffer::new(RING_CAPACITY);
        let chunk = vec![b'x'; 10_000];
        for _ in 0..100 {
            ring.push(&chunk);
        }
        assert_eq!(ring.len(), RING_CAPACITY);
    }
}
