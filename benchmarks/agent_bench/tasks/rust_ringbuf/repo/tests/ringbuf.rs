use ringbuf::RingBuffer;

fn filled(capacity: usize, values: std::ops::Range<i32>) -> RingBuffer<i32> {
    let mut rb = RingBuffer::with_capacity(capacity);
    for v in values {
        rb.push(v);
    }
    rb
}

fn contents(rb: &RingBuffer<i32>) -> Vec<i32> {
    rb.iter().copied().collect()
}

#[test]
fn new_buffer_is_empty() {
    let rb: RingBuffer<i32> = RingBuffer::with_capacity(3);
    assert!(rb.is_empty());
    assert_eq!(rb.len(), 0);
    assert_eq!(rb.capacity(), 3);
}

#[test]
#[should_panic]
fn zero_capacity_panics() {
    let _ = RingBuffer::<i32>::with_capacity(0);
}

#[test]
fn push_until_full() {
    let rb = filled(3, 0..3);
    assert!(rb.is_full());
    assert_eq!(contents(&rb), vec![0, 1, 2]);
}

#[test]
fn push_when_full_returns_oldest() {
    let mut rb = filled(3, 0..3);
    assert_eq!(rb.push(3), Some(0));
    assert_eq!(rb.len(), 3);
}

#[test]
fn iter_after_wraparound() {
    let rb = filled(3, 0..5);
    assert_eq!(contents(&rb), vec![2, 3, 4]);
}

#[test]
fn get_after_wraparound() {
    let rb = filled(4, 0..6);
    assert_eq!(rb.get(0), Some(&2));
    assert_eq!(rb.get(3), Some(&5));
    assert_eq!(rb.get(4), None);
}

#[test]
fn pop_is_fifo() {
    let mut rb = filled(3, 0..3);
    assert_eq!(rb.pop(), Some(0));
    assert_eq!(rb.pop(), Some(1));
    assert_eq!(rb.pop(), Some(2));
    assert_eq!(rb.pop(), None);
}

#[test]
fn pop_after_wraparound() {
    let mut rb = filled(3, 0..7);
    assert_eq!(rb.pop(), Some(4));
    rb.push(7);
    assert_eq!(contents(&rb), vec![5, 6, 7]);
}

#[test]
fn interleaved_push_pop() {
    let mut rb = RingBuffer::with_capacity(2);
    for i in 0..10 {
        rb.push(i);
        if i % 3 == 0 {
            rb.pop();
        }
    }
    assert_eq!(contents(&rb), vec![9]);
}

#[test]
fn many_wraps_keep_order() {
    for cap in 1..8 {
        let rb = filled(cap, 0..100);
        let want: Vec<i32> = (100 - cap as i32..100).collect();
        assert_eq!(contents(&rb), want, "capacity {cap}");
    }
}

#[test]
fn clear_resets() {
    let mut rb = filled(3, 0..5);
    rb.clear();
    assert!(rb.is_empty());
    rb.push(9);
    assert_eq!(contents(&rb), vec![9]);
}

#[test]
fn works_with_strings() {
    let mut rb = RingBuffer::with_capacity(2);
    for s in ["a", "b", "c"] {
        rb.push(s.to_string());
    }
    let got: Vec<&str> = rb.iter().map(String::as_str).collect();
    assert_eq!(got, ["b", "c"]);
}
