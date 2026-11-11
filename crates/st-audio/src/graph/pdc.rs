//! Plug-in delay compensation: the latency plan of the routing graph and the delay line that
//! realizes it.
//!
//! Every node of the graph has a latency in frames (non-zero only for inserts that report one).
//! A node's *arrival* is how late its output is against the engine's clock: the largest arrival
//! among its inputs, plus its own latency. Where inputs of different arrival meet (a group bus
//! fed by a compensating and a plain track, the main and the sidechain input of a compressor, a
//! dry path next to a latent send return), the earlier ones are delayed on their edge so that
//! all inputs of a node line up. Main and auxiliary (sidechain) edges are planned alike, and
//! gain, mute or a disabled solo path never change the plan: it depends on topology only.

/// The result of [`plan`]. Indexed like the nodes and edges it was given.
#[derive(Debug, Clone, PartialEq)]
pub struct LatencyPlan {
    /// A topological order of the nodes (ties: lower index first).
    pub order: Vec<usize>,
    /// Frames by which each node's output lags the source clock.
    pub arrival: Vec<u64>,
    /// The same at each node's input (before its own latency): the time base of its automation.
    pub input_arrival: Vec<u64>,
    /// Frames each edge must delay its signal.
    pub edge_delays: Vec<u64>,
    /// The largest arrival: what an offline render trims from its start.
    pub total: u64,
}

/// Plans a graph of `latencies.len()` nodes and `edges` as `(source, destination)`.
///
/// Fails for an edge that names a missing node, for a cycle, and when an arrival exceeds
/// `maximum` frames.
pub fn plan(latencies: &[u64], edges: &[(usize, usize)], maximum: u64) -> Result<LatencyPlan, String> {
    let count = latencies.len();
    let mut incoming: Vec<Vec<usize>> = vec![Vec::new(); count];
    let mut outgoing: Vec<Vec<usize>> = vec![Vec::new(); count];
    let mut degree = vec![0usize; count];
    for (index, &(source, destination)) in edges.iter().enumerate() {
        if source >= count || destination >= count {
            return Err(format!("Invalid routing connection {index}: it names a stage that does not exist."));
        }
        incoming[destination].push(index);
        outgoing[source].push(index);
        degree[destination] += 1;
    }
    let mut ready: std::collections::BinaryHeap<std::cmp::Reverse<usize>> =
        (0..count).filter(|node| degree[*node] == 0).map(std::cmp::Reverse).collect();
    let mut result = LatencyPlan {
        order: Vec::with_capacity(count),
        arrival: vec![0; count],
        input_arrival: vec![0; count],
        edge_delays: vec![0; edges.len()],
        total: 0,
    };
    while let Some(std::cmp::Reverse(node)) = ready.pop() {
        let at_input = incoming[node].iter().map(|edge| result.arrival[edges[*edge].0]).max().unwrap_or(0);
        let at_output = at_input.checked_add(latencies[node]).filter(|frames| *frames <= maximum);
        let Some(at_output) = at_output else {
            return Err(format!("Routing latency exceeds the {maximum}-frame compensation capacity."));
        };
        result.input_arrival[node] = at_input;
        result.arrival[node] = at_output;
        result.total = result.total.max(at_output);
        result.order.push(node);
        for &edge in &incoming[node] {
            result.edge_delays[edge] = at_input - result.arrival[edges[edge].0];
        }
        for &edge in &outgoing[node] {
            let destination = edges[edge].1;
            degree[destination] -= 1;
            if degree[destination] == 0 {
                ready.push(std::cmp::Reverse(destination));
            }
        }
    }
    if result.order.len() != count {
        return Err("Routing feedback is unsupported: the mixer's routes form a loop.".to_string());
    }
    Ok(result)
}

/// A stereo delay of a fixed number of frames. All memory is allocated by [`DelayLine::new`].
pub struct DelayLine {
    left: Box<[f32]>,
    right: Box<[f32]>,
    position: usize,
}

impl DelayLine {
    /// A delay of `frames` frames (at least one).
    pub fn new(frames: usize) -> DelayLine {
        let frames = frames.max(1);
        DelayLine { left: vec![0.0; frames].into_boxed_slice(), right: vec![0.0; frames].into_boxed_slice(), position: 0 }
    }

    pub fn frames(&self) -> usize {
        self.left.len()
    }

    /// Writes the input delayed by `frames()` to the output. Slices must have equal lengths.
    pub fn process(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        let size = self.left.len();
        let mut position = self.position;
        for (((x_l, x_r), y_l), y_r) in in_l.iter().zip(in_r).zip(out_l.iter_mut()).zip(out_r.iter_mut()) {
            *y_l = std::mem::replace(&mut self.left[position], *x_l);
            *y_r = std::mem::replace(&mut self.right[position], *x_r);
            position += 1;
            if position == size {
                position = 0;
            }
        }
        self.position = position;
    }

    /// Number of frames `out_l` is long: feeds silence and writes what comes out.
    pub fn process_silence(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let size = self.left.len();
        let mut position = self.position;
        for (y_l, y_r) in out_l.iter_mut().zip(out_r.iter_mut()) {
            *y_l = std::mem::replace(&mut self.left[position], 0.0);
            *y_r = std::mem::replace(&mut self.right[position], 0.0);
            position += 1;
            if position == size {
                position = 0;
            }
        }
        self.position = position;
    }

    pub fn clear(&mut self) {
        self.left.fill(0.0);
        self.right.fill(0.0);
        self.position = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Pure routing validation and DAG timing.
    #[test]
    fn inputs_of_different_latency_are_aligned_on_their_edges() {
        // a (64) and b (257) meet in mix (0), which feeds out (64).
        let latencies = [64, 257, 0, 64];
        let edges = [(0, 2), (1, 2), (2, 3)];
        let planned = plan(&latencies, &edges, 321).unwrap();
        assert_eq!(planned.order, [0, 1, 2, 3]);
        assert_eq!(planned.edge_delays, [193, 0, 0]);
        assert_eq!((planned.arrival[2], planned.input_arrival[3], planned.arrival[3], planned.total), (257, 257, 321, 321));
        // The result does not depend on the order edges are listed in.
        let reversed = plan(&latencies, &[(2, 3), (1, 2), (0, 2)], 321).unwrap();
        assert_eq!((reversed.order, reversed.arrival, reversed.edge_delays), (planned.order, planned.arrival, vec![0, 0, 193]));
    }

    #[test]
    fn the_planner_refuses_what_it_cannot_compensate() {
        let latencies = [64, 257, 0, 64];
        let edges = [(0, 2), (1, 2), (2, 3)];
        assert!(plan(&latencies, &edges, 320).unwrap_err().contains("320-frame"));
        assert!(plan(&latencies, &[(0, 2), (1, 2), (2, 3), (3, 0)], 500).unwrap_err().contains("loop"));
        assert!(plan(&latencies, &[(0, 9)], 500).is_err());
        assert!(plan(&[u64::MAX, 1], &[(0, 1)], u64::MAX).is_err(), "overflow is an error, not a wrap");
        assert_eq!(plan(&[], &[], 0).unwrap().total, 0);
        assert_eq!(plan(&[64], &[], 64).unwrap().total, 64, "a lone latent node still reports its latency");
    }

    #[test]
    fn a_sidechain_slower_than_the_main_input_delays_the_main_input() {
        // track a: source -> main (64) -> compressor (64); track b: source -> aux (257) -> key of the compressor.
        // nodes: 0 a.source, 1 main, 2 compressor, 3 b.source, 4 aux, 5 master
        let latencies = [0, 64, 64, 0, 257, 0];
        let edges = [(0, 1), (1, 2), (3, 4), (4, 2), (2, 5)];
        let planned = plan(&latencies, &edges, 96_000).unwrap();
        assert_eq!((planned.input_arrival[2], planned.arrival[2]), (257, 321));
        assert_eq!((planned.edge_delays[1], planned.edge_delays[3]), (193, 0));
        assert_eq!((planned.arrival[5], planned.total), (321, 321));
        // Swap the latencies: now the key waits for the main input.
        let swapped = plan(&[0, 257, 64, 0, 64, 0], &edges, 321).unwrap();
        assert_eq!((swapped.edge_delays[1], swapped.edge_delays[3], swapped.total), (0, 193, 321));
    }

    #[test]
    fn a_delay_line_delays_by_exactly_its_length_across_any_block_sizes() {
        let input: Vec<f32> = (1..=40).map(|v| v as f32).collect();
        let mut expected = vec![0.0; 7];
        expected.extend_from_slice(&input[..33]);
        for block in [1usize, 3, 7, 16, 40] {
            let mut line = DelayLine::new(7);
            let (mut out_l, mut out_r) = (vec![0.0; 40], vec![0.0; 40]);
            let mut at = 0;
            while at < 40 {
                let end = (at + block).min(40);
                let (l, r) = (&mut out_l[at..end], &mut out_r[at..end]);
                line.process(&input[at..end], &input[at..end], l, r);
                at = end;
            }
            assert_eq!(out_l, expected, "block {block}");
            assert_eq!(out_r, expected);
            let (mut tail_l, mut tail_r) = (vec![9.0; 7], vec![9.0; 7]);
            line.process_silence(&mut tail_l, &mut tail_r);
            assert_eq!(tail_l, input[33..]);
        }
    }
}
