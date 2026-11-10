//! Scheduling of outgoing MIDI against the audio render timeline: the timestamp arithmetic, the
//! lock-free queue from the audio thread to the sender thread, and the sender's ordered backlog.
//! Pure: no devices and no clock reads.

use std::cell::UnsafeCell;
use std::cmp::Ordering as CmpOrdering;
use std::collections::BinaryHeap;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use crate::message::Message;

/// Device time of a note to an external instrument: the render block's device time plus the
/// event's frame offset, less the route's lead. The lead never moves a note to or before the
/// clock's origin; a block without device time (`host_time` 0) gives 0, "send now".
pub fn note_timestamp(host_time: u64, frame_offset: i32, ticks_per_frame: f64, lead_ticks: u32) -> u64 {
    if host_time == 0 {
        return 0;
    }
    let stamp = host_time.saturating_add((f64::from(frame_offset) * ticks_per_frame).max(0.0) as u64);
    if stamp > u64::from(lead_ticks) {
        stamp - u64::from(lead_ticks)
    } else {
        stamp
    }
}

/// One message on its way to an output device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimedMessage {
    /// Device time to send at, in ticks; 0 sends as soon as the sender sees it.
    pub time: u64,
    /// The output's slot in the sender's table.
    pub output: u8,
    /// The message.
    pub message: Message,
}

struct Ring<T> {
    slots: Box<[UnsafeCell<MaybeUninit<T>>]>,
    /// Next slot to read; written by the consumer only.
    head: AtomicUsize,
    /// Next slot to write; written by the producer only.
    tail: AtomicUsize,
    producer_alive: AtomicBool,
}

// SAFETY: the ring is shared by exactly one `Producer` and one `Consumer`. A slot is written by
// the producer before `tail` passes it and read by the consumer before `head` passes it, so no
// slot is accessed from both sides at once; `T: Copy` values need no drop.
unsafe impl<T: Copy + Send> Sync for Ring<T> {}
unsafe impl<T: Copy + Send> Send for Ring<T> {}

/// The writing end of a bounded single-producer single-consumer queue. `push` never blocks,
/// allocates or makes a system call: it is safe on the audio thread.
pub struct Producer<T: Copy + Send> {
    ring: Arc<Ring<T>>,
}

/// The reading end of the queue.
pub struct Consumer<T: Copy + Send> {
    ring: Arc<Ring<T>>,
}

/// A queue that holds up to `capacity` items (rounded up to a power of two, at least 2).
pub fn queue<T: Copy + Send>(capacity: usize) -> (Producer<T>, Consumer<T>) {
    let size = capacity.max(2).next_power_of_two();
    let slots = (0..size).map(|_| UnsafeCell::new(MaybeUninit::uninit())).collect();
    let ring = Arc::new(Ring { slots, head: AtomicUsize::new(0), tail: AtomicUsize::new(0), producer_alive: AtomicBool::new(true) });
    (Producer { ring: ring.clone() }, Consumer { ring })
}

impl<T: Copy + Send> Producer<T> {
    /// Adds an item; returns false, dropping it, when the queue is full.
    pub fn push(&mut self, item: T) -> bool {
        let ring = &*self.ring;
        let tail = ring.tail.load(Ordering::Relaxed);
        if tail.wrapping_sub(ring.head.load(Ordering::Acquire)) >= ring.slots.len() {
            return false;
        }
        // SAFETY: the slot at `tail` is outside the readable range [head, tail), and this is the
        // only producer (`&mut self`).
        unsafe { (*ring.slots[tail & (ring.slots.len() - 1)].get()).write(item) };
        ring.tail.store(tail.wrapping_add(1), Ordering::Release);
        true
    }
}

impl<T: Copy + Send> Drop for Producer<T> {
    fn drop(&mut self) {
        self.ring.producer_alive.store(false, Ordering::Release);
    }
}

impl<T: Copy + Send> Consumer<T> {
    /// Takes the oldest item.
    pub fn pop(&mut self) -> Option<T> {
        let ring = &*self.ring;
        let head = ring.head.load(Ordering::Relaxed);
        if head == ring.tail.load(Ordering::Acquire) {
            return None;
        }
        // SAFETY: the slot at `head` was written before `tail` moved past it, and this is the only
        // consumer (`&mut self`).
        let item = unsafe { (*ring.slots[head & (ring.slots.len() - 1)].get()).assume_init() };
        ring.head.store(head.wrapping_add(1), Ordering::Release);
        Some(item)
    }

    /// True when the producer is gone and everything it sent has been taken.
    pub fn is_finished(&self) -> bool {
        // The flag is read first: once it is false and the queue is empty, nothing more can come.
        !self.ring.producer_alive.load(Ordering::Acquire) && self.ring.head.load(Ordering::Relaxed) == self.ring.tail.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    message: TimedMessage,
    sequence: u64,
}

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == CmpOrdering::Equal
    }
}
impl Eq for Entry {}
impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}
impl Ord for Entry {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        // Reversed: the heap's greatest element is the earliest message, and among equal times
        // the one that arrived first.
        (other.message.time, other.sequence).cmp(&(self.message.time, self.sequence))
    }
}

/// The sender thread's backlog: messages wait here until their time, and leave in time order
/// (messages for the same time leave in the order they arrived).
#[derive(Debug)]
pub struct Schedule {
    heap: BinaryHeap<Entry>,
    capacity: usize,
    sequence: u64,
}

impl Schedule {
    /// A backlog that holds at most `capacity` messages.
    pub fn new(capacity: usize) -> Self {
        Schedule { heap: BinaryHeap::with_capacity(capacity), capacity, sequence: 0 }
    }

    /// Adds a message; returns false, dropping it, when the backlog is full.
    pub fn push(&mut self, message: TimedMessage) -> bool {
        if self.heap.len() >= self.capacity {
            return false;
        }
        self.sequence += 1;
        self.heap.push(Entry { message, sequence: self.sequence });
        true
    }

    /// Takes the next message whose time has come (`time <= now`).
    pub fn pop_due(&mut self, now: u64) -> Option<TimedMessage> {
        if self.heap.peek()?.message.time > now {
            return None;
        }
        self.heap.pop().map(|entry| entry.message)
    }

    /// When the earliest waiting message is due.
    pub fn next_time(&self) -> Option<u64> {
        self.heap.peek().map(|entry| entry.message.time)
    }

    /// Messages waiting.
    pub fn len(&self) -> usize {
        self.heap.len()
    }

    /// True when nothing waits.
    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    /// Removes every message for `output` (its device went away) and returns how many.
    pub fn remove_output(&mut self, output: u8) -> usize {
        let before = self.heap.len();
        self.heap.retain(|entry| entry.message.output != output);
        before - self.heap.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_timestamps_follow_the_frame_offset_and_lead() {
        // One tick per frame: an event 240 frames into the block is 240 ticks after its start.
        assert_eq!(note_timestamp(1_000_000, 240, 1.0, 0), 1_000_240);
        assert_eq!(note_timestamp(1_000_000, 0, 20_833.3, 0), 1_000_000);
        assert_eq!(note_timestamp(1_000_000, 48, 20_833.333, 0), 1_000_000 + 999_999);
        assert_eq!(note_timestamp(1_000_000, 240, 1.0, 480), 999_760, "the lead sends the note early");
        assert_eq!(note_timestamp(100, 10, 1.0, 110), 110, "never at or before the clock's origin");
        assert_eq!(note_timestamp(1_000_000, -5, 1.0, 0), 1_000_000, "a negative offset is the block's start");
        assert_eq!(note_timestamp(0, 240, 1.0, 480), 0, "no device time: send now");
        assert_eq!(note_timestamp(u64::MAX - 1, 100, 1.0, 0), u64::MAX, "no overflow");
    }

    #[test]
    fn the_queue_is_bounded_and_ordered() {
        let (mut producer, mut consumer) = queue::<u32>(4);
        assert_eq!(consumer.pop(), None);
        for value in 0..4 {
            assert!(producer.push(value));
        }
        assert!(!producer.push(99), "a full queue drops instead of blocking");
        assert_eq!(consumer.pop(), Some(0));
        assert!(producer.push(4));
        assert_eq!((1..=4).map(|_| consumer.pop().unwrap()).collect::<Vec<_>>(), [1, 2, 3, 4]);
        assert!(!consumer.is_finished());
        producer.push(5);
        drop(producer);
        assert!(!consumer.is_finished(), "what was sent is still delivered");
        assert_eq!(consumer.pop(), Some(5));
        assert!(consumer.is_finished());
        let (producer, _consumer) = queue::<u8>(3);
        assert_eq!(producer.ring.slots.len(), 4, "capacity rounds up to a power of two");
    }

    #[test]
    fn the_queue_carries_everything_between_two_threads() {
        let (mut producer, mut consumer) = queue::<u64>(64);
        let writer = std::thread::spawn(move || {
            for value in 0..200_000u64 {
                while !producer.push(value) {
                    std::thread::yield_now();
                }
            }
        });
        let mut expected = 0u64;
        while expected < 200_000 {
            match consumer.pop() {
                Some(value) => {
                    assert_eq!(value, expected);
                    expected += 1;
                }
                None => std::thread::yield_now(),
            }
        }
        writer.join().unwrap();
        assert!(consumer.is_finished());
    }

    fn timed(time: u64, output: u8, note: u8) -> TimedMessage {
        TimedMessage { time, output, message: Message::new(0x90, note, 100) }
    }

    #[test]
    fn the_backlog_releases_messages_in_time_order() {
        let mut schedule = Schedule::new(8);
        assert!(schedule.is_empty() && schedule.next_time().is_none() && schedule.pop_due(u64::MAX).is_none());
        for (time, note) in [(300, 3), (100, 1), (200, 2), (100, 9), (0, 0)] {
            assert!(schedule.push(timed(time, 0, note)));
        }
        assert_eq!(schedule.next_time(), Some(0));
        assert_eq!(schedule.pop_due(50).map(|message| message.message.data1), Some(0), "time 0 is always due");
        assert_eq!(schedule.pop_due(50), None);
        assert_eq!(schedule.next_time(), Some(100));
        let due: Vec<u8> = std::iter::from_fn(|| schedule.pop_due(200)).map(|message| message.message.data1).collect();
        assert_eq!(due, [1, 9, 2], "equal times leave in arrival order");
        assert_eq!(schedule.len(), 1);
        for index in 0..7 {
            assert!(schedule.push(timed(1000 + index, 1, 0)));
        }
        assert!(!schedule.push(timed(5, 1, 0)), "a full backlog drops");
        assert_eq!(schedule.remove_output(1), 7);
        assert_eq!(schedule.pop_due(u64::MAX).map(|message| message.message.data1), Some(3));
    }
}
