use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamKind {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TaggedByte {
    stream: StreamKind,
    byte: u8,
}

#[derive(Debug, Clone)]
pub(crate) struct TaggedHeadTailBuffer {
    head_capacity: usize,
    tail_capacity: usize,
    head: Vec<TaggedByte>,
    tail: VecDeque<TaggedByte>,
    total_bytes: u64,
}

impl TaggedHeadTailBuffer {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            head_capacity: capacity,
            tail_capacity: 0,
            head: Vec::with_capacity(capacity),
            tail: VecDeque::with_capacity(0),
            total_bytes: 0,
        }
    }

    pub(crate) fn push(&mut self, stream: StreamKind, bytes: &[u8]) {
        self.total_bytes = self
            .total_bytes
            .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        for &byte in bytes {
            let tagged = TaggedByte { stream, byte };
            if self.head.len() < self.head_capacity {
                self.head.push(tagged);
                continue;
            }
            if self.tail_capacity == 0 {
                continue;
            }
            if self.tail.len() == self.tail_capacity {
                self.tail.pop_front();
            }
            self.tail.push_back(tagged);
        }
    }

    pub(crate) fn finish(self) -> TaggedOutput {
        let retained = self.head.len().saturating_add(self.tail.len());
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        for tagged in self.head.into_iter().chain(self.tail) {
            match tagged.stream {
                StreamKind::Stdout => stdout.push(tagged.byte),
                StreamKind::Stderr => stderr.push(tagged.byte),
            }
        }
        TaggedOutput {
            stdout,
            stderr,
            omitted_bytes: self
                .total_bytes
                .saturating_sub(u64::try_from(retained).unwrap_or(u64::MAX)),
        }
    }

    pub(crate) fn snapshot(&self) -> TaggedOutput {
        self.clone().finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaggedOutput {
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) omitted_bytes: u64,
}

impl TaggedOutput {
    #[cfg(test)]
    pub(crate) fn retained_bytes(&self) -> usize {
        self.stdout.len().saturating_add(self.stderr.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdout_and_stderr_share_one_head_budget() {
        let mut buffer = TaggedHeadTailBuffer::new(8);
        buffer.push(StreamKind::Stdout, b"abcd");
        buffer.push(StreamKind::Stderr, b"EFGH");
        buffer.push(StreamKind::Stdout, b"ijkl");

        let output = buffer.finish();
        assert_eq!(output.retained_bytes(), 8);
        assert_eq!(output.omitted_bytes, 4);
        // First 8 bytes: "abcd" (stdout) + "EFGH" (stderr). Subsequent "ijkl" (stdout) omitted.
        assert_eq!(output.stdout, b"abcd");
        assert_eq!(output.stderr, b"EFGH");
    }
}
