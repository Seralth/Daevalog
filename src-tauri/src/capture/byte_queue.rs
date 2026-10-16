//! The bytes of one stream that have arrived but not been framed yet.

/// Past this many held bytes the stream is lost anyway: start over.
const RESET_ABOVE: usize = 2 * 1024 * 1024;

#[derive(Debug, Default)]
pub struct ByteQueue {
    held: Vec<u8>,
}

impl ByteQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn size(&self) -> usize {
        self.held.len()
    }

    /// Oldest byte first.
    pub fn snapshot(&self) -> &[u8] {
        self.held.as_slice()
    }

    pub fn append(&mut self, data: &[u8]) {
        let before = self.size();
        if before > RESET_ABOVE {
            tracing::error!("{before} bytes held with no frame in them: starting the stream over");
            self.held = Vec::with_capacity(data.len());
        }
        self.held.extend(data.iter().copied());
    }

    /// Drops the first `length` bytes, or all of them.
    pub fn drop_front(&mut self, length: usize) {
        if length >= self.size() {
            self.held.clear();
        } else {
            self.held.copy_within(length.., 0);
            self.held.truncate(self.size() - length);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_runaway_buffer_starts_over() {
        let mut acc = ByteQueue::new();
        acc.append(&vec![1; RESET_ABOVE + 1]);
        acc.append(&[2]);
        assert_eq!(acc.snapshot(), &[2]);
    }

    #[test]
    fn exactly_the_limit_is_kept() {
        let mut acc = ByteQueue::new();
        acc.append(&vec![1; RESET_ABOVE]);
        acc.append(&[2]);
        assert_eq!(acc.size(), RESET_ABOVE + 1);
    }

    #[test]
    fn discarding_drops_the_oldest_bytes() {
        let mut acc = ByteQueue::new();
        acc.append(&[1, 2]);
        acc.append(&[3]);
        assert_eq!(acc.snapshot(), &[1, 2, 3]);
        acc.drop_front(2);
        assert_eq!(acc.snapshot(), &[3]);
        acc.drop_front(5);
        assert_eq!(acc.size(), 0);
    }
}
