//! A fixed-capacity ring buffer that overwrites the oldest element when full.

#[derive(Debug, Clone)]
pub struct RingBuffer<T> {
    slots: Vec<Option<T>>,
    head: usize, // index of the oldest element
    len: usize,
}

impl<T> RingBuffer<T> {
    pub fn with_capacity(capacity: usize) -> Self {
        assert!(capacity > 0, "capacity must be positive");
        let mut slots = Vec::with_capacity(capacity);
        slots.resize_with(capacity, || None);
        RingBuffer {
            slots,
            head: 0,
            len: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn is_full(&self) -> bool {
        self.len == self.capacity()
    }

    /// Append `value`; when full, the oldest element is dropped and returned.
    pub fn push(&mut self, value: T) -> Option<T> {
        let tail = (self.head + self.len) % self.capacity();
        if self.is_full() {
            let old = self.slots[tail].replace(value);
            self.head = (self.head + 1) % self.capacity();
            old
        } else {
            self.slots[tail] = Some(value);
            self.len += 1;
            None
        }
    }

    /// Remove and return the oldest element.
    pub fn pop(&mut self) -> Option<T> {
        if self.is_empty() {
            return None;
        }
        let value = self.slots[self.head].take();
        self.head = (self.head + 1) % self.capacity();
        self.len -= 1;
        value
    }

    /// The `i`-th element, oldest first.
    pub fn get(&self, i: usize) -> Option<&T> {
        if i >= self.len {
            return None;
        }
        self.slots[(self.head + i) % self.capacity()].as_ref()
    }

    /// Iterate oldest to newest.
    pub fn iter(&self) -> impl Iterator<Item = &T> + '_ {
        (0..self.len).filter_map(move |i| self.get(i))
    }

    pub fn clear(&mut self) {
        self.slots.iter_mut().for_each(|s| *s = None);
        self.head = 0;
        self.len = 0;
    }
}
