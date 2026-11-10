//! Sample-accurate automation of the mixer: timed values for faders, pans, route gains and
//! insert parameters.
//!
//! The engine is the clock of all automation. It evaluates lanes and step locks while it
//! renders and reports every value as an event with a frame offset in the block: track volume,
//! pan and the two built-in sends as track events, everything the host owns (bus and master
//! faders, send and sidechain gains, insert parameters) as host events numbered by
//! [`crate::graph::spec::Destination`]. Stepped lanes report each change at its exact frame;
//! linear lanes report on the engine's 32-frame control interval, so a ramp is a staircase of
//! 32-frame steps whose values lie on the line.
//!
//! The render plan turns each event into a [`TimedEvent`] for one *target* (a strip's fader,
//! a route's gain, an insert) and files it here. A target that sits behind latent inserts hears
//! the engine late, so its events are due later by the same number of frames (the target's
//! input arrival, see [`crate::graph::pdc`]); events due after the current block wait in a heap.
//! When a target renders, [`EventQueue::segments`] walks its block: apply the events due, render
//! up to the next one, and so on. Values are held between events (no smoothing).
//!
//! Everything is allocated by [`EventQueue::new`]. When the queue is full an event is dropped and
//! counted; the next value of that lane corrects the target.

/// Parameter numbers of fader targets.
pub const PARAM_VOLUME: u32 = 0;
pub const PARAM_PAN: u32 = 1;
/// Parameter number of a route's gain.
pub const PARAM_GAIN: u32 = 0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimedEvent {
    /// Absolute frame on the graph clock at which the value applies.
    pub time: u64,
    pub target: u32,
    /// Fader: [`PARAM_VOLUME`] or [`PARAM_PAN`]; route: [`PARAM_GAIN`]; insert: the parameter address.
    pub param: u32,
    pub value: f32,
    order: u32,
}

const NONE: u32 = u32::MAX;

pub struct EventQueue {
    /// Events of the current block, chained per target in time order.
    block: Vec<TimedEvent>,
    next: Vec<u32>,
    heads: Vec<u32>,
    tails: Vec<u32>,
    touched: Vec<u32>,
    /// Events due after the current block: a binary min-heap by (time, order).
    future: Vec<TimedEvent>,
    capacity: usize,
    order: u32,
    block_start: u64,
    block_end: u64,
    dropped: u64,
}

/// One step of [`EventQueue::segments`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Segment {
    /// A value that applies from the frames that follow.
    Event(TimedEvent),
    /// Frames `from..to` of the block, to render with the values applied so far.
    Run(usize, usize),
}

impl EventQueue {
    pub fn new(targets: usize, capacity: usize) -> EventQueue {
        EventQueue {
            block: Vec::with_capacity(capacity),
            next: Vec::with_capacity(capacity),
            heads: vec![NONE; targets],
            tails: vec![NONE; targets],
            touched: Vec::with_capacity(targets),
            future: Vec::with_capacity(capacity),
            capacity,
            order: 0,
            block_start: 0,
            block_end: 0,
            dropped: 0,
        }
    }

    /// Events dropped because the queue was full.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Starts the block `start..start + frames`: forgets the previous block's events and files
    /// the waiting events that fall into this one.
    pub fn begin_block(&mut self, start: u64, frames: usize) {
        for &target in &self.touched {
            self.heads[target as usize] = NONE;
            self.tails[target as usize] = NONE;
        }
        self.touched.clear();
        self.block.clear();
        self.next.clear();
        self.block_start = start;
        self.block_end = start + frames as u64;
        while self.future.first().is_some_and(|event| event.time < self.block_end) {
            let event = self.pop_future();
            self.file(event);
        }
    }

    /// Forgets every event (a new timeline).
    pub fn clear(&mut self) {
        self.begin_block(self.block_start, 0);
        self.future.clear();
    }

    /// Adds a value for `target`, due at `time`. Call after `begin_block`, in the order the
    /// engine reported the events.
    pub fn schedule(&mut self, time: u64, target: u32, param: u32, value: f32) {
        if target as usize >= self.heads.len() || !value.is_finite() {
            return;
        }
        self.order = self.order.wrapping_add(1);
        let event = TimedEvent { time: time.max(self.block_start), target, param, value, order: self.order };
        if event.time < self.block_end {
            self.file(event);
        } else if self.future.len() < self.capacity {
            self.future.push(event);
            let mut index = self.future.len() - 1;
            while index > 0 {
                let parent = (index - 1) / 2;
                if !precedes(&self.future[index], &self.future[parent]) {
                    break;
                }
                self.future.swap(index, parent);
                index = parent;
            }
        } else {
            self.dropped += 1;
        }
    }

    fn file(&mut self, event: TimedEvent) {
        if self.block.len() >= self.capacity {
            self.dropped += 1;
            return;
        }
        let index = self.block.len() as u32;
        let target = event.target as usize;
        self.block.push(event);
        self.next.push(NONE);
        if self.tails[target] == NONE {
            self.heads[target] = index;
            self.touched.push(event.target);
        } else {
            self.next[self.tails[target] as usize] = index;
        }
        self.tails[target] = index;
    }

    fn pop_future(&mut self) -> TimedEvent {
        let last = self.future.len() - 1;
        self.future.swap(0, last);
        let event = self.future.pop().unwrap_or(TimedEvent { time: 0, target: 0, param: 0, value: 0.0, order: 0 });
        let count = self.future.len();
        let mut index = 0;
        loop {
            let mut least = index;
            for child in [2 * index + 1, 2 * index + 2] {
                if child < count && precedes(&self.future[child], &self.future[least]) {
                    least = child;
                }
            }
            if least == index {
                break;
            }
            self.future.swap(index, least);
            index = least;
        }
        event
    }

    /// Whether `target` has events in the current block.
    pub fn has_events(&self, target: u32) -> bool {
        self.heads.get(target as usize).is_some_and(|head| *head != NONE)
    }

    /// The current block of `frames` frames for `target`, as events and the runs between them.
    pub fn segments(&self, target: u32, frames: usize) -> Segments<'_> {
        Segments { queue: self, cursor: self.heads.get(target as usize).copied().unwrap_or(NONE), frame: 0, frames }
    }
}

fn precedes(a: &TimedEvent, b: &TimedEvent) -> bool {
    a.time < b.time || (a.time == b.time && a.order.wrapping_sub(b.order) > u32::MAX / 2)
}

pub struct Segments<'a> {
    queue: &'a EventQueue,
    cursor: u32,
    frame: usize,
    frames: usize,
}

impl Iterator for Segments<'_> {
    type Item = Segment;

    fn next(&mut self) -> Option<Segment> {
        if self.cursor != NONE {
            let event = self.queue.block[self.cursor as usize];
            let offset = (event.time - self.queue.block_start) as usize;
            if offset <= self.frame {
                self.cursor = self.queue.next[self.cursor as usize];
                return Some(Segment::Event(event));
            }
            if self.frame < self.frames {
                let end = offset.min(self.frames);
                let run = Segment::Run(self.frame, end);
                self.frame = end;
                return Some(run);
            }
            // Past the block's end (cannot happen for filed events): drop the rest.
            self.cursor = NONE;
        }
        if self.frame < self.frames {
            let run = Segment::Run(self.frame, self.frames);
            self.frame = self.frames;
            return Some(run);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walk(queue: &EventQueue, target: u32, frames: usize) -> Vec<String> {
        queue
            .segments(target, frames)
            .map(|segment| match segment {
                Segment::Event(event) => format!("{}={}", event.param, event.value),
                Segment::Run(from, to) => format!("{from}..{to}"),
            })
            .collect()
    }

    #[test]
    fn events_split_a_block_at_their_frames_in_order() {
        let mut queue = EventQueue::new(3, 64);
        queue.begin_block(1000, 100);
        queue.schedule(1000, 1, PARAM_VOLUME, 0.5);
        queue.schedule(1040, 1, PARAM_PAN, -1.0);
        queue.schedule(1040, 1, PARAM_VOLUME, 0.25);
        queue.schedule(1010, 2, PARAM_GAIN, 2.0);
        assert_eq!(walk(&queue, 1, 100), ["0=0.5", "0..40", "1=-1", "0=0.25", "40..100"]);
        assert_eq!(walk(&queue, 2, 100), ["0..10", "0=2", "10..100"]);
        assert_eq!(walk(&queue, 0, 100), ["0..100"]);
        assert!(queue.has_events(1) && !queue.has_events(0));
        queue.begin_block(1100, 50);
        assert_eq!(walk(&queue, 1, 50), ["0..50"], "a new block starts without the old events");
    }

    #[test]
    fn late_targets_get_their_events_in_a_later_block_at_the_exact_frame() {
        let mut queue = EventQueue::new(1, 64);
        queue.begin_block(0, 64);
        // Due 257 frames late, reported in reverse order of time within one call sequence.
        queue.schedule(10 + 257, 0, PARAM_VOLUME, 1.0);
        queue.schedule(20 + 257, 0, PARAM_VOLUME, 2.0);
        assert_eq!(walk(&queue, 0, 64), ["0..64"]);
        for block in 1..4 {
            queue.begin_block(block * 64, 64);
            assert_eq!(walk(&queue, 0, 64), ["0..64"]);
        }
        queue.begin_block(256, 64);
        assert_eq!(walk(&queue, 0, 64), ["0..11", "0=1", "11..21", "0=2", "21..64"]);
    }

    #[test]
    fn a_full_queue_drops_and_counts() {
        let mut queue = EventQueue::new(1, 4);
        queue.begin_block(0, 16);
        for index in 0..6 {
            queue.schedule(index, 0, PARAM_VOLUME, index as f32);
            queue.schedule(100 + index, 0, PARAM_VOLUME, index as f32);
        }
        queue.schedule(0, 9, PARAM_VOLUME, 1.0);
        queue.schedule(0, 0, PARAM_VOLUME, f32::NAN);
        assert_eq!(queue.dropped(), 4);
        queue.clear();
        queue.begin_block(96, 16);
        assert_eq!(walk(&queue, 0, 16), ["0..16"], "clear forgets waiting events too");
    }
}
