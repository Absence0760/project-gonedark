//! In-process **loopback** transport (the [`gonedark_pal::Transport`] seam, D27). This is the
//! dev/single-process double: a connected pair of endpoints where a `send` on one side makes the
//! bytes available to the other side's `poll`, with no socket anywhere. It's how `core::lockstep`
//! (sans-I/O) is driven end-to-end in one process — the lockstep loop produces opaque frames, the
//! host hands them to one endpoint, and the peer's host polls them off the other (exactly the
//! "two-instance, zero-socket" verification D27 describes).
//!
//! **Threading choice:** single-threaded `Rc<RefCell<VecDeque<…>>>`. The whole point of the
//! loopback is in-process dev + tests, which run both endpoints on one thread; the simplest correct
//! thing is a shared single-threaded queue, and it keeps `Send`/`Sync` baggage out of the dev path.
//! A thread-safe pair (a real cross-thread channel) is the job of the `server` socket backend, not
//! this dev double. Each direction is its own FIFO queue, so order is preserved per direction and
//! the two directions never interfere; frames are stored as whole `Vec<u8>`, so framing is exact
//! (never split or merged).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use gonedark_pal::Transport;

/// One direction's frame queue, shared between the producing endpoint's `send` and the consuming
/// endpoint's `poll`.
type Queue = Rc<RefCell<VecDeque<Vec<u8>>>>;

/// One end of an in-process loopback link. `send` enqueues onto the partner's inbound queue; `poll`
/// drains this end's own inbound queue. Construct a connected pair with [`LoopbackTransport::pair`].
pub struct LoopbackTransport {
    /// Frames this endpoint has received and not yet drained (the partner's `send` writes here).
    inbound: Queue,
    /// The partner's inbound queue — this endpoint's `send` writes here.
    outbound: Queue,
}

impl LoopbackTransport {
    /// Create a connected pair `(a, b)`: bytes sent on `a` arrive on `b`'s `poll` (and vice versa).
    /// The two directions are independent FIFO queues.
    pub fn pair() -> (LoopbackTransport, LoopbackTransport) {
        let a_to_b: Queue = Rc::new(RefCell::new(VecDeque::new()));
        let b_to_a: Queue = Rc::new(RefCell::new(VecDeque::new()));
        let a = LoopbackTransport {
            inbound: Rc::clone(&b_to_a),
            outbound: Rc::clone(&a_to_b),
        };
        let b = LoopbackTransport {
            inbound: a_to_b,
            outbound: b_to_a,
        };
        (a, b)
    }
}

impl Transport for LoopbackTransport {
    fn send(&mut self, frame: &[u8]) {
        // Copy the opaque frame whole onto the partner's inbound queue — never inspected, never
        // split or coalesced with its neighbours.
        self.outbound.borrow_mut().push_back(frame.to_vec());
    }

    fn poll(&mut self) -> Vec<Vec<u8>> {
        // Drain everything received since the last poll, in arrival (FIFO) order; leave the queue
        // empty so the next poll only sees newly-arrived frames.
        self.inbound.borrow_mut().drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_is_object_safe() {
        // `core::lockstep`'s host drives a `&mut dyn Transport`; this fails to compile if the trait
        // ever loses object-safety.
        let (mut a, _b) = LoopbackTransport::pair();
        let dynamic: &mut dyn Transport = &mut a;
        dynamic.send(b"via dyn");
        // (Nothing to poll on this end; the point is that the dyn coercion type-checks.)
    }

    #[test]
    fn frames_flow_a_to_b_and_b_to_a_in_fifo_order() {
        let (mut a, mut b) = LoopbackTransport::pair();

        a.send(b"first");
        a.send(b"second");
        a.send(b"third");
        let got = b.poll();
        assert_eq!(got, vec![b"first".to_vec(), b"second".to_vec(), b"third".to_vec()]);

        // Reverse direction is independent and equally FIFO.
        b.send(b"reply-1");
        b.send(b"reply-2");
        let got = a.poll();
        assert_eq!(got, vec![b"reply-1".to_vec(), b"reply-2".to_vec()]);
    }

    #[test]
    fn poll_is_empty_when_nothing_sent() {
        let (mut a, mut b) = LoopbackTransport::pair();
        assert!(a.poll().is_empty());
        assert!(b.poll().is_empty());
    }

    #[test]
    fn multiple_frames_drained_in_one_poll_then_empty() {
        let (mut a, mut b) = LoopbackTransport::pair();

        a.send(b"one");
        a.send(b"two");
        a.send(b"three");

        let first = b.poll();
        assert_eq!(first.len(), 3);
        assert_eq!(first, vec![b"one".to_vec(), b"two".to_vec(), b"three".to_vec()]);

        // A second poll with nothing new returns empty — drain consumed the queue.
        assert!(b.poll().is_empty());
    }

    #[test]
    fn frames_are_byte_exact_with_no_merge_across_boundaries() {
        let (mut a, mut b) = LoopbackTransport::pair();

        // Frames with embedded zero bytes, empty frames, and a binary payload: each must come back
        // whole and byte-identical, never concatenated with its neighbours.
        let frames: Vec<Vec<u8>> = vec![
            vec![0x00, 0x01, 0x02],
            vec![],
            vec![0xFF; 64],
            b"\x00mid\x00null\x00".to_vec(),
        ];
        for f in &frames {
            a.send(f);
        }

        let got = b.poll();
        assert_eq!(got, frames);
    }

    #[test]
    fn directions_do_not_interfere() {
        let (mut a, mut b) = LoopbackTransport::pair();

        a.send(b"a-only");
        // b has sent nothing, so a's own poll sees nothing even though a just sent.
        assert!(a.poll().is_empty());
        // b receives a's frame.
        assert_eq!(b.poll(), vec![b"a-only".to_vec()]);
    }
}
