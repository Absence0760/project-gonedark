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
use std::io::ErrorKind;
use std::net::{SocketAddr, UdpSocket};
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

/// Largest receive buffer for a single inbound datagram. One frame == one UDP datagram, so this
/// caps the frame size we can deliver. 64 KiB is the maximum a single UDP datagram can carry
/// (the 16-bit length field), and lockstep frames are tiny (a tick's worth of orders), so this is
/// generously oversized — a frame is never split or truncated in practice. If frames ever grow
/// past a path's MTU, IP fragmentation handles it for now; an explicit application-level
/// fragmentation/reassembly scheme is a future concern (and a reason the QUIC swap exists, D27).
const MAX_DATAGRAM: usize = 64 * 1024;

/// Real-socket [`gonedark_pal::Transport`] over a plain `std::net::UdpSocket` — the production
/// sibling of [`LoopbackTransport`]. Where the loopback double moves frames through an in-process
/// queue, this ships each opaque frame as one UDP datagram to a configured peer and drains arrived
/// datagrams non-blocking. No async runtime, no extra dependency: the lockstep host calls `send`
/// and `poll` exactly as it does for the loopback double; only the wiring (a real socket vs a
/// shared queue) differs.
///
/// **UDP is unreliable, unordered, and may duplicate.** This transport adds **no** reliability of
/// its own — it ships and drains opaque bytes verbatim (the opaque-frame contract, D27). That is
/// by design: `core::lockstep`'s retransmit + dedup window already tolerates loss, reordering, and
/// duplicates, so layering reliability here would be redundant work in the wrong place. The
/// transport never inspects, splits, or merges a frame; one `send` is exactly one datagram and one
/// arrived datagram is exactly one polled frame.
///
/// **Framing:** one frame ↔ one datagram. Frames are assumed to fit in a single datagram (see
/// [`MAX_DATAGRAM`]); the small lockstep frames always do. Oversized frames are a future
/// fragmentation concern, not handled here.
///
/// Per D27 this is **UDP now**; a QUIC transport (for Wi-Fi↔cellular path migration) is the
/// documented future option and would be a separate concrete impl behind the same trait.
pub struct UdpTransport {
    socket: UdpSocket,
    /// The peer every `send` targets. Set at construction; `poll` accepts datagrams from any
    /// source (UDP has no connection), so a stray sender is simply delivered as opaque bytes —
    /// the lockstep layer's framing/dedup is the authority on what's a valid frame.
    peer: SocketAddr,
}

impl UdpTransport {
    /// Bind a local UDP socket to `local_addr` and target `peer_addr` for every [`send`](Self::send).
    /// The socket is put into non-blocking mode so [`poll`](Self::poll) never stalls the tick loop.
    ///
    /// `local_addr` may use port `0` to let the OS pick an ephemeral port (read it back with
    /// [`local_addr`](Self::local_addr) — that's how the localhost [`pair`](Self::pair) wires two
    /// ends together).
    pub fn new(local_addr: SocketAddr, peer_addr: SocketAddr) -> std::io::Result<UdpTransport> {
        let socket = UdpSocket::bind(local_addr)?;
        socket.set_nonblocking(true)?;
        Ok(UdpTransport {
            socket,
            peer: peer_addr,
        })
    }

    /// The actual local address the socket is bound to — useful when `local_addr` was bound with
    /// an ephemeral (`:0`) port and the peer needs to be told where to send.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// Create a connected localhost pair `(a, b)`: bytes sent on `a` arrive on `b`'s `poll` (and
    /// vice versa). Both ends bind `127.0.0.1:0` (OS-chosen ephemeral ports) and target each
    /// other — the real-socket analogue of [`LoopbackTransport::pair`], for tests and local
    /// two-instance runs.
    pub fn pair() -> std::io::Result<(UdpTransport, UdpTransport)> {
        let loopback = |port: u16| SocketAddr::from(([127, 0, 0, 1], port));

        // Bind both sockets first (ephemeral ports), then learn each one's address so we can point
        // them at each other.
        let sock_a = UdpSocket::bind(loopback(0))?;
        let sock_b = UdpSocket::bind(loopback(0))?;
        sock_a.set_nonblocking(true)?;
        sock_b.set_nonblocking(true)?;
        let addr_a = sock_a.local_addr()?;
        let addr_b = sock_b.local_addr()?;

        let a = UdpTransport {
            socket: sock_a,
            peer: addr_b,
        };
        let b = UdpTransport {
            socket: sock_b,
            peer: addr_a,
        };
        Ok((a, b))
    }
}

impl Transport for UdpTransport {
    fn send(&mut self, frame: &[u8]) {
        // One opaque frame → one datagram to the peer. UDP is lossy by contract, so a failed send
        // is swallowed (never panics, never blocks): the lockstep layer's retransmit window is the
        // recovery mechanism, not this transport. `send_to` never partially sends a datagram — it's
        // all-or-nothing — so there is no short-write framing hazard.
        let _ = self.socket.send_to(frame, self.peer);
    }

    fn poll(&mut self) -> Vec<Vec<u8>> {
        // Drain every datagram currently buffered, in arrival order, without blocking. The socket
        // is non-blocking, so `recv_from` returns `WouldBlock` once the buffer is empty — that's
        // our loop terminator, not an error. Any *other* error degrades to "no more frames this
        // poll" (UDP is lossy by design and the lockstep layer is loss-tolerant), so we never
        // panic and never spin.
        let mut frames = Vec::new();
        let mut buf = [0u8; MAX_DATAGRAM];
        loop {
            match self.socket.recv_from(&mut buf) {
                // One datagram = one whole frame; copy exactly the received bytes (never the whole
                // buffer) so framing is byte-exact, including zero-length and embedded-NUL frames.
                Ok((len, _from)) => frames.push(buf[..len].to_vec()),
                // Buffer drained: stop without blocking. This is the normal, expected exit.
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                // Any other transient socket error: stop draining this poll and return what we
                // have. UDP loss is expected; the lockstep layer recovers via its retransmit/dedup
                // window. Never panic.
                Err(_) => break,
            }
        }
        frames
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
        assert_eq!(
            got,
            vec![b"first".to_vec(), b"second".to_vec(), b"third".to_vec()]
        );

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
        assert_eq!(
            first,
            vec![b"one".to_vec(), b"two".to_vec(), b"three".to_vec()]
        );

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

#[cfg(test)]
mod udp_tests {
    use super::*;

    /// Maximum non-blocking `poll` attempts before we conclude the expected frames are not
    /// coming. Localhost UDP is effectively reliable (the kernel hands the datagram to the peer
    /// socket's receive buffer synchronously on loopback), but it is *technically* lossy, so the
    /// tests never assume a single poll sees the frame: they poll in a bounded busy loop until the
    /// expected count arrives or this cap is hit. The loop is a tight non-blocking spin (no
    /// `sleep`), so it is fast and not a timing race — the cap exists only so a genuinely dropped
    /// datagram fails the test deterministically instead of hanging forever.
    const POLL_ATTEMPTS: usize = 10_000;

    /// Poll `t` repeatedly (non-blocking) until at least `want` frames have accumulated or the
    /// attempt cap is reached, then return everything collected. Pure busy-loop, no sleeps, so it
    /// is deterministic and fast on loopback.
    fn poll_until(t: &mut UdpTransport, want: usize) -> Vec<Vec<u8>> {
        let mut got = Vec::new();
        for _ in 0..POLL_ATTEMPTS {
            got.extend(t.poll());
            if got.len() >= want {
                break;
            }
        }
        got
    }

    #[test]
    fn transport_is_object_safe() {
        // Same compile-time guard as the loopback double: the lockstep host drives a
        // `&mut dyn Transport`, so this must coerce.
        let (mut a, _b) = UdpTransport::pair().expect("bind localhost udp pair");
        let dynamic: &mut dyn Transport = &mut a;
        dynamic.send(b"via dyn");
    }

    #[test]
    fn poll_is_empty_when_nothing_sent() {
        // Non-blocking: a fresh pair with no traffic must return immediately with no frames and
        // must not hang.
        let (mut a, mut b) = UdpTransport::pair().expect("bind localhost udp pair");
        assert!(a.poll().is_empty());
        assert!(b.poll().is_empty());
    }

    #[test]
    fn frame_flows_a_to_b() {
        let (mut a, mut b) = UdpTransport::pair().expect("bind localhost udp pair");

        a.send(b"hello-udp");
        let got = poll_until(&mut b, 1);
        assert_eq!(got, vec![b"hello-udp".to_vec()]);
    }

    #[test]
    fn multiple_frames_drained_one_datagram_each() {
        // One frame == one datagram: three sends arrive as three distinct frames, never merged or
        // split. Order on localhost loopback is preserved, but the assertion only relies on the
        // set + per-frame byte-exactness if reordering were to occur; here we check the exact FIFO
        // sequence since loopback does not reorder.
        let (mut a, mut b) = UdpTransport::pair().expect("bind localhost udp pair");

        a.send(b"one");
        a.send(b"two");
        a.send(b"three");

        let got = poll_until(&mut b, 3);
        assert_eq!(
            got,
            vec![b"one".to_vec(), b"two".to_vec(), b"three".to_vec()]
        );
    }

    #[test]
    fn frames_are_byte_exact_including_empty_and_embedded_nul() {
        // A zero-length datagram, a binary payload, and a frame with embedded NULs must each come
        // back whole and byte-identical — `poll` copies exactly `len` bytes, never the whole recv
        // buffer.
        let (mut a, mut b) = UdpTransport::pair().expect("bind localhost udp pair");

        let frames: Vec<Vec<u8>> = vec![
            vec![0x00, 0x01, 0x02],
            vec![],
            vec![0xFF; 512],
            b"\x00mid\x00null\x00".to_vec(),
        ];
        for f in &frames {
            a.send(f);
        }

        let got = poll_until(&mut b, frames.len());
        assert_eq!(got, frames);
    }

    #[test]
    fn directions_are_independent() {
        // a→b and b→a are separate sockets; a frame in one direction never appears in the other.
        let (mut a, mut b) = UdpTransport::pair().expect("bind localhost udp pair");

        a.send(b"to-b");
        b.send(b"to-a");

        let on_a = poll_until(&mut a, 1);
        let on_b = poll_until(&mut b, 1);
        assert_eq!(on_a, vec![b"to-a".to_vec()]);
        assert_eq!(on_b, vec![b"to-b".to_vec()]);
    }

    #[test]
    fn explicit_new_binds_and_targets_peer() {
        // The general constructor (not the convenience `pair`): bind one ephemeral socket, learn
        // its address, bind a second pointed at the first, then point the first back at the second
        // (a `local_addr` round-trip — what a real handshake does to learn the remote addr). Then
        // confirm a frame crosses each way.
        let mut a = UdpTransport::new(
            SocketAddr::from(([127, 0, 0, 1], 0)),
            // Placeholder peer; corrected below once `b`'s real address is known.
            SocketAddr::from(([127, 0, 0, 1], 0)),
        )
        .expect("bind a");
        let addr_a = a.local_addr().expect("a local_addr");

        let mut b = UdpTransport::new(SocketAddr::from(([127, 0, 0, 1], 0)), addr_a)
            .expect("bind b targeting a");
        let addr_b = b.local_addr().expect("b local_addr");

        // Now that `b`'s address is known, point `a` at it.
        a.peer = addr_b;

        a.send(b"ping");
        assert_eq!(poll_until(&mut b, 1), vec![b"ping".to_vec()]);

        b.send(b"pong");
        assert_eq!(poll_until(&mut a, 1), vec![b"pong".to_vec()]);
    }
}
