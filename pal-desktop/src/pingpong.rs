//! Transport-level RTT **ping/pong** — the live sample source for the host-side adaptive-input-delay
//! estimator (Phase 3, workstream B; `engine::net_tuning`).
//!
//! `engine`'s `RttDelayEstimator` is fed measured round-trip times through `Game::observe_rtt`, but
//! until something *measures* an RTT the estimator stays inert (no samples → no proposals → it never
//! fabricates a delay change). This module is that measurement: a tiny ping/pong multiplexed over an
//! existing [`Transport`].
//!
//! **Why this is NOT a `core::lockstep` wire frame.** Adding a ping/pong message to the lockstep
//! protocol would touch `core`'s sans-I/O wire codec — and `core` must stay clock-free and
//! float-free (invariant #1), seeing only an integer delay (`net_tuning`'s contract). RTT is a
//! wall-clock measurement, so it belongs entirely on the host/transport side. We therefore keep it
//! **out** of the lockstep protocol and layer it *underneath* the [`Transport`] seam instead: a
//! [`PingPongTransport`] wraps any inner transport, tags each outbound datagram (lockstep payload vs
//! its own ping/pong), and on the way in peels its own ping/pong traffic off before handing the
//! lockstep bytes back to the host. `core::lockstep` never sees a ping; the transport never inspects
//! a `Command`. The 1-byte envelope is purely this backend's framing — both ends run the wrapper, so
//! the lockstep bytes round-trip byte-identical (the tag is stripped before delivery). Floats and a
//! clock are fine here: this is `pal-desktop`, not the sim (the determinism guard scopes to
//! `core`/`sim`).
//!
//! **Testable seam.** All the logic with a decision in it is pure and lives in free functions + the
//! clock-free [`RttMeter`]: the frame codec ([`encode_ping`]/[`encode_pong`]/[`wrap_lockstep`] /
//! [`decode`]) and the RTT bookkeeping (sequence/echo matching, `recv - send` → RTT seconds, stale-
//! /duplicate-sample rejection, bounded outstanding set under loss). [`RttMeter`] takes timestamps as
//! explicit `f64` arguments — no clock inside — so the whole measurement is unit-tested without IO or
//! timing. [`PingPongTransport`] is the thin wrapper that pulls `now` from an injected
//! [`Clock`](gonedark_pal::Clock) and delegates; it is still fully constructible/testable with a fake
//! clock + a [`LoopbackTransport`](crate::LoopbackTransport) pair (see the tests), so nothing is
//! exempted as un-constructible glue.
//!
//! **Wiring to `engine`.** Measured RTTs accumulate in a shared queue the host drains via
//! [`PingPongTransport::samples`] (a cloneable [`RttSamples`] handle, the same `Rc<RefCell<…>>` idiom
//! the loopback double uses). The host owns the `Game` (which owns the boxed transport) *and* a clone
//! of the handle; each frame it drains the handle and calls `Game::observe_rtt(rtt)` for every
//! sample. That host loop sits behind a `wgpu`-bearing `Game`, so it is the one genuinely
//! un-constructible seam — the sample *source* is fully tested here; the one-line `observe_rtt` feed
//! is documented, not unit-tested.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Instant;

use gonedark_pal::{Clock, Transport};

/// Outer envelope tag: the datagram carries an opaque `core::lockstep` frame (payload follows).
const TAG_LOCKSTEP: u8 = 0;
/// Outer envelope tag: a ping — the peer must echo it back as a pong with the same sequence.
const TAG_PING: u8 = 1;
/// Outer envelope tag: a pong — an echo of a ping we sent, carrying its sequence for matching.
const TAG_PONG: u8 = 2;

/// Wire length of a ping/pong datagram: one tag byte + a 4-byte LE sequence number.
const PINGPONG_LEN: usize = 1 + 4;

/// How often (seconds) [`PingPongTransport`] emits a fresh ping. ~4 Hz: enough samples to keep the
/// estimator's EWMA fed without meaningful bandwidth (a 5-byte datagram), and well under the
/// estimator's multi-second dwell so a delay decision always rests on several measurements.
const DEFAULT_PING_INTERVAL_SECS: f64 = 0.25;

/// Cap on un-answered pings tracked at once. Bounds memory under loss (a dropped ping or pong leaves
/// an orphan entry): once full, the oldest orphan is evicted when a new ping is registered. 8 covers
/// ~2 s of in-flight pings at the default rate — far more than any healthy link has outstanding.
const DEFAULT_MAX_OUTSTANDING: usize = 8;

/// A decoded inbound datagram: either an opaque lockstep payload to hand back to the host, or one of
/// our own ping/pong control frames (with its sequence number).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decoded {
    /// An opaque `core::lockstep` frame (the tag stripped); deliver it to the host verbatim.
    Lockstep(Vec<u8>),
    /// A ping from the peer carrying `seq`; the receiver must reply with [`encode_pong`]`(seq)`.
    Ping(u32),
    /// A pong from the peer echoing the `seq` of a ping we sent; matched by [`RttMeter::match_pong`].
    Pong(u32),
}

/// Envelope an opaque outbound lockstep frame: prepend the [`TAG_LOCKSTEP`] byte. The peer's
/// [`decode`] strips it back to the original bytes, so the lockstep wire format is untouched.
pub fn wrap_lockstep(frame: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + frame.len());
    out.push(TAG_LOCKSTEP);
    out.extend_from_slice(frame);
    out
}

/// Encode a ping datagram for sequence `seq`: `[TAG_PING, seq_le_0..4]`.
pub fn encode_ping(seq: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(PINGPONG_LEN);
    out.push(TAG_PING);
    out.extend_from_slice(&seq.to_le_bytes());
    out
}

/// Encode a pong datagram echoing sequence `seq`: `[TAG_PONG, seq_le_0..4]`.
pub fn encode_pong(seq: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(PINGPONG_LEN);
    out.push(TAG_PONG);
    out.extend_from_slice(&seq.to_le_bytes());
    out
}

/// Decode one inbound datagram into its [`Decoded`] form, or `None` if it is malformed / unknown.
///
/// Pure and total: an empty datagram, an unrecognised tag, or a ping/pong of the wrong length all
/// yield `None` (the caller drops them — a stray/garbage datagram is never a panic, mirroring the
/// loss-tolerant posture of the UDP transport). A lockstep frame of *any* length (including empty)
/// decodes to its exact payload bytes.
pub fn decode(frame: &[u8]) -> Option<Decoded> {
    let (&tag, rest) = frame.split_first()?;
    match tag {
        TAG_LOCKSTEP => Some(Decoded::Lockstep(rest.to_vec())),
        TAG_PING | TAG_PONG => {
            // Exactly a 4-byte LE sequence must follow; anything else is corrupt framing.
            let bytes: [u8; 4] = rest.try_into().ok()?;
            let seq = u32::from_le_bytes(bytes);
            Some(if tag == TAG_PING {
                Decoded::Ping(seq)
            } else {
                Decoded::Pong(seq)
            })
        }
        _ => None,
    }
}

/// Clock-free RTT bookkeeping: hands out monotonic ping sequence numbers, remembers each ping's send
/// timestamp, and matches an echoed pong back to its ping to yield an RTT. **Pure** — every method
/// takes `now` as an explicit argument, so the whole measurement is unit-testable with no clock or
/// IO. [`PingPongTransport`] is the only thing that supplies a real clock.
#[derive(Clone, Debug)]
pub struct RttMeter {
    /// Next sequence number to hand out; wraps (a 32-bit space is astronomically more than enough,
    /// but wrapping keeps it total and panic-free).
    next_seq: u32,
    /// Un-answered pings, oldest first: `(seq, send_time_secs)`. FIFO so the oldest orphan evicts
    /// first when the bound is hit. Tiny (`<= max_outstanding`), so a linear scan to match is cheap.
    outstanding: VecDeque<(u32, f64)>,
    /// Cap on `outstanding` — bounds memory under packet loss (see [`DEFAULT_MAX_OUTSTANDING`]).
    max_outstanding: usize,
}

impl RttMeter {
    /// A fresh meter tracking at most `max_outstanding` un-answered pings (clamped to at least 1 so
    /// a registered ping is always retained long enough to match its immediate pong).
    pub fn new(max_outstanding: usize) -> Self {
        RttMeter {
            next_seq: 0,
            outstanding: VecDeque::new(),
            max_outstanding: max_outstanding.max(1),
        }
    }

    /// Register a new ping sent at `now`, returning its sequence number. If the outstanding set is
    /// full, the oldest un-answered ping is evicted first (it was lost — its pong will never match,
    /// which is correct: a lost ping must not produce a sample, and must not grow memory).
    pub fn register_ping(&mut self, now: f64) -> u32 {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        while self.outstanding.len() >= self.max_outstanding {
            self.outstanding.pop_front();
        }
        self.outstanding.push_back((seq, now));
        seq
    }

    /// Match an arrived pong (`seq`, arrived at `now`) to its ping and return the RTT in seconds,
    /// consuming the entry. Returns `None` for a **stale** sample: an unknown or already-consumed
    /// sequence (a duplicate pong, or a pong for an evicted/lost ping), or a non-monotonic clock read
    /// (`now < send_time` ⇒ negative/garbage RTT). A `None` never poisons the estimate — the host
    /// simply has no sample this round.
    pub fn match_pong(&mut self, seq: u32, now: f64) -> Option<f64> {
        let idx = self.outstanding.iter().position(|&(s, _)| s == seq)?;
        let (_, send_time) = self.outstanding.remove(idx)?;
        let rtt = now - send_time;
        // Reject a non-finite or negative RTT (clock skew / bad read) rather than feed it onward.
        (rtt.is_finite() && rtt >= 0.0).then_some(rtt)
    }

    /// How many pings are currently awaiting a pong (for tests / introspection).
    pub fn outstanding_len(&self) -> usize {
        self.outstanding.len()
    }
}

/// A cloneable handle the host drains to pull measured RTT samples (seconds) out of a
/// [`PingPongTransport`]. Shares the transport's sample queue via `Rc<RefCell<…>>` — the same
/// single-threaded in-process idiom the loopback double uses — so the host can drain samples while
/// the `Game` owns the boxed transport. Each drained `f64` is exactly a `Game::observe_rtt` input.
#[derive(Clone, Debug)]
pub struct RttSamples {
    queue: Rc<RefCell<VecDeque<f64>>>,
}

impl RttSamples {
    /// Take every RTT sample (seconds) measured since the last drain, in measurement order, leaving
    /// the queue empty. The host feeds each into `Game::observe_rtt`.
    pub fn drain(&self) -> Vec<f64> {
        self.queue.borrow_mut().drain(..).collect()
    }

    /// True when no samples are currently buffered.
    pub fn is_empty(&self) -> bool {
        self.queue.borrow().is_empty()
    }
}

/// A monotonic [`Clock`] over `std::time::Instant` for production use (the host's real wall clock).
/// Seconds are measured from construction, so values are small and never go backwards. Tests inject a
/// fake clock instead; this is the real-time impl the desktop host would hand to a
/// [`PingPongTransport`].
#[derive(Clone, Debug)]
pub struct SystemClock {
    start: Instant,
}

impl SystemClock {
    /// Start the clock now (t = 0 at construction).
    pub fn new() -> Self {
        SystemClock {
            start: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        SystemClock::new()
    }
}

impl Clock for SystemClock {
    fn now_secs(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }
}

/// A [`Transport`] decorator that measures RTT with a ping/pong multiplexed over an inner transport,
/// surfacing samples for `engine`'s adaptive-input-delay estimator (see the module docs).
///
/// It is a drop-in `Transport`: the host's lockstep drive loop calls [`send`](Self::send) /
/// [`poll`](Self::poll) exactly as for a bare transport. Outbound lockstep frames are tagged and
/// forwarded; [`poll`](Self::poll) additionally (a) emits a fresh ping on its cadence, (b) replies to
/// inbound pings with pongs, and (c) matches inbound pongs to record an RTT sample — returning to the
/// host **only** the lockstep payloads. Generic over the inner transport `T` and the clock `C` so it
/// composes over [`UdpTransport`](crate::UdpTransport) / [`LoopbackTransport`](crate::LoopbackTransport)
/// and any [`Clock`] (a real [`SystemClock`] in production, a fake one in tests).
pub struct PingPongTransport<T: Transport, C: Clock> {
    inner: T,
    clock: C,
    meter: RttMeter,
    /// Minimum seconds between successive outgoing pings.
    ping_interval_secs: f64,
    /// Clock time of the last ping we sent, or `None` until the first poll (so the first poll pings
    /// immediately and the estimator starts measuring without waiting out a full interval).
    last_ping_secs: Option<f64>,
    /// Measured RTT samples awaiting drain by the host; shared with every [`RttSamples`] handle.
    samples: Rc<RefCell<VecDeque<f64>>>,
}

impl<T: Transport, C: Clock> PingPongTransport<T, C> {
    /// Wrap `inner` with the default cadence ([`DEFAULT_PING_INTERVAL_SECS`]) and outstanding bound
    /// ([`DEFAULT_MAX_OUTSTANDING`]), driving timing off `clock`.
    pub fn new(inner: T, clock: C) -> Self {
        Self::with_config(
            inner,
            clock,
            DEFAULT_PING_INTERVAL_SECS,
            DEFAULT_MAX_OUTSTANDING,
        )
    }

    /// Wrap `inner` with an explicit ping interval (seconds) and outstanding-ping bound — used by
    /// tests to ping every poll (interval `0.0`) and to size the loss bound.
    pub fn with_config(
        inner: T,
        clock: C,
        ping_interval_secs: f64,
        max_outstanding: usize,
    ) -> Self {
        PingPongTransport {
            inner,
            clock,
            meter: RttMeter::new(max_outstanding),
            ping_interval_secs: ping_interval_secs.max(0.0),
            last_ping_secs: None,
            samples: Rc::new(RefCell::new(VecDeque::new())),
        }
    }

    /// A handle the host drains each frame to feed `Game::observe_rtt`. Cheap to clone (shares the
    /// sample queue); keep one in the host loop while the `Game` owns this transport.
    pub fn samples(&self) -> RttSamples {
        RttSamples {
            queue: Rc::clone(&self.samples),
        }
    }

    /// Emit a fresh ping if the cadence has elapsed (or it is the very first poll). Idempotent within
    /// an interval: at most one ping per interval, registered with the meter so its pong can match.
    fn maybe_send_ping(&mut self, now: f64) {
        let due = match self.last_ping_secs {
            None => true,
            Some(last) => now - last >= self.ping_interval_secs,
        };
        if due {
            let seq = self.meter.register_ping(now);
            self.inner.send(&encode_ping(seq));
            self.last_ping_secs = Some(now);
        }
    }
}

impl<T: Transport, C: Clock> Transport for PingPongTransport<T, C> {
    fn send(&mut self, frame: &[u8]) {
        // Tag the opaque lockstep frame and forward it; the peer's wrapper strips the tag before
        // delivery, so the lockstep bytes are untouched on the wire.
        self.inner.send(&wrap_lockstep(frame));
    }

    fn poll(&mut self) -> Vec<Vec<u8>> {
        let now = self.clock.now_secs();
        // Drive the ping cadence off poll (the host calls poll every tick); reply to pings and
        // record pongs as datagrams arrive, surfacing only the lockstep payloads to the host.
        self.maybe_send_ping(now);

        let inbound = self.inner.poll();
        let mut lockstep_frames = Vec::with_capacity(inbound.len());
        for raw in inbound {
            match decode(&raw) {
                Some(Decoded::Lockstep(payload)) => lockstep_frames.push(payload),
                Some(Decoded::Ping(seq)) => self.inner.send(&encode_pong(seq)),
                Some(Decoded::Pong(seq)) => {
                    if let Some(rtt) = self.meter.match_pong(seq, now) {
                        self.samples.borrow_mut().push_back(rtt);
                    }
                }
                // Malformed / unknown datagram: drop it (loss-tolerant, never a panic).
                None => {}
            }
        }
        lockstep_frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LoopbackTransport;
    use std::cell::Cell;

    // ----- frame codec (pure) -----

    #[test]
    fn lockstep_frame_roundtrips_byte_exact_including_empty_and_nul() {
        for payload in [
            &b""[..],
            &b"order-bytes"[..],
            &[0x00, 0x01, 0xFF][..],
            &b"\x00mid\x00null\x00"[..],
        ] {
            let wire = wrap_lockstep(payload);
            assert_eq!(wire[0], TAG_LOCKSTEP);
            assert_eq!(
                decode(&wire),
                Some(Decoded::Lockstep(payload.to_vec())),
                "lockstep payload must survive the envelope byte-exact"
            );
        }
    }

    #[test]
    fn ping_and_pong_roundtrip_their_sequence() {
        for seq in [0u32, 1, 42, u32::MAX] {
            assert_eq!(decode(&encode_ping(seq)), Some(Decoded::Ping(seq)));
            assert_eq!(decode(&encode_pong(seq)), Some(Decoded::Pong(seq)));
        }
    }

    #[test]
    fn decode_rejects_empty_unknown_tag_and_wrong_length() {
        assert_eq!(decode(&[]), None, "empty datagram is not a frame");
        assert_eq!(decode(&[0xEE]), None, "unknown tag");
        assert_eq!(decode(&[0xEE, 1, 2, 3, 4]), None, "unknown tag w/ body");
        // ping/pong must carry exactly 4 sequence bytes.
        assert_eq!(decode(&[TAG_PING]), None, "ping missing sequence");
        assert_eq!(decode(&[TAG_PING, 1, 2, 3]), None, "ping seq too short");
        assert_eq!(decode(&[TAG_PONG, 1, 2, 3, 4, 5]), None, "pong seq too long");
    }

    // ----- RttMeter (pure) -----

    #[test]
    fn match_pong_returns_recv_minus_send() {
        let mut m = RttMeter::new(8);
        let seq = m.register_ping(10.0);
        assert_eq!(m.outstanding_len(), 1);
        let rtt = m.match_pong(seq, 10.08).expect("matching pong yields rtt");
        assert!((rtt - 0.08).abs() < 1e-9, "rtt = recv - send, got {rtt}");
        assert_eq!(m.outstanding_len(), 0, "a matched ping is consumed");
    }

    #[test]
    fn match_pong_rejects_unknown_and_duplicate_sequences() {
        let mut m = RttMeter::new(8);
        let seq = m.register_ping(1.0);
        assert_eq!(m.match_pong(seq + 99, 1.1), None, "unknown seq is stale");
        assert!(m.match_pong(seq, 1.1).is_some(), "first echo matches");
        assert_eq!(
            m.match_pong(seq, 1.2),
            None,
            "a duplicate pong for a consumed seq is stale"
        );
    }

    #[test]
    fn match_pong_rejects_non_monotonic_clock() {
        let mut m = RttMeter::new(8);
        let seq = m.register_ping(5.0);
        assert_eq!(
            m.match_pong(seq, 4.9),
            None,
            "now < send_time ⇒ negative rtt rejected, not fed onward"
        );
        // The entry is still consumed (a bogus read shouldn't linger), so a retry finds nothing.
        assert_eq!(m.outstanding_len(), 0);
    }

    #[test]
    fn outstanding_set_is_bounded_evicting_oldest_under_loss() {
        let mut m = RttMeter::new(3);
        // Register 5 pings that are never answered; only the newest 3 are retained.
        let seqs: Vec<u32> = (0..5).map(|i| m.register_ping(i as f64)).collect();
        assert_eq!(m.outstanding_len(), 3, "bound holds under loss");
        // The two oldest were evicted — their late pongs no longer match (correct: they were lost).
        assert_eq!(m.match_pong(seqs[0], 9.0), None);
        assert_eq!(m.match_pong(seqs[1], 9.0), None);
        // The three newest still match.
        assert!(m.match_pong(seqs[2], 9.0).is_some());
        assert!(m.match_pong(seqs[3], 9.0).is_some());
        assert!(m.match_pong(seqs[4], 9.0).is_some());
    }

    #[test]
    fn new_clamps_zero_bound_to_one() {
        let mut m = RttMeter::new(0);
        let seq = m.register_ping(0.0);
        // Even with a requested bound of 0, the just-registered ping must survive to match its pong.
        assert!(m.match_pong(seq, 0.05).is_some());
    }

    // ----- PingPongTransport (wrapper) with a fake clock + loopback pair -----

    /// A test clock the test advances by hand; shared via `Rc<Cell<…>>` so a held handle can `set`
    /// the time the wrapped transport reads through `now_secs`.
    #[derive(Clone)]
    struct FakeClock(Rc<Cell<f64>>);
    impl FakeClock {
        fn new() -> Self {
            FakeClock(Rc::new(Cell::new(0.0)))
        }
        fn set(&self, t: f64) {
            self.0.set(t);
        }
    }
    impl Clock for FakeClock {
        fn now_secs(&self) -> f64 {
            self.0.get()
        }
    }

    /// Build a connected pair of ping/pong transports over a loopback double, pinging every poll
    /// (interval 0.0). Returns the two transports plus a handle to each clock and `a`'s sample sink.
    fn pingpong_pair() -> (
        PingPongTransport<LoopbackTransport, FakeClock>,
        PingPongTransport<LoopbackTransport, FakeClock>,
        FakeClock,
        FakeClock,
        RttSamples,
    ) {
        let (la, lb) = LoopbackTransport::pair();
        let ca = FakeClock::new();
        let cb = FakeClock::new();
        let a = PingPongTransport::with_config(la, ca.clone(), 0.0, 8);
        let b = PingPongTransport::with_config(lb, cb.clone(), 0.0, 8);
        let samples = a.samples();
        (a, b, ca, cb, samples)
    }

    #[test]
    fn measures_a_real_round_trip_time() {
        let (mut a, mut b, ca, _cb, samples) = pingpong_pair();

        // t=0: a's first poll emits ping#0 (no inbound yet).
        ca.set(0.0);
        assert!(a.poll().is_empty(), "no lockstep traffic yet");
        assert!(samples.is_empty(), "no pong has come back yet");

        // b polls: it sees a's ping and replies with a pong (and emits its own ping, ignored by a).
        b.poll();

        // t=0.05: a polls again, receives b's pong, and records RTT = 0.05 - 0.0.
        ca.set(0.05);
        assert!(a.poll().is_empty(), "ping/pong are never surfaced as lockstep frames");
        let got = samples.drain();
        assert_eq!(got.len(), 1, "exactly one RTT sample measured");
        assert!(
            (got[0] - 0.05).abs() < 1e-9,
            "measured RTT must be recv - send, got {}",
            got[0]
        );
        // Every sample is a valid `Game::observe_rtt` input (finite, non-negative).
        assert!(got[0].is_finite() && got[0] >= 0.0);
    }

    #[test]
    fn lockstep_frames_pass_through_both_directions_unmodified() {
        let (mut a, mut b, _ca, _cb, _samples) = pingpong_pair();

        // a → b: a real (opaque) lockstep frame rides through alongside the ping/pong noise.
        a.send(b"lockstep-a-to-b");
        let on_b = b.poll();
        assert_eq!(
            on_b,
            vec![b"lockstep-a-to-b".to_vec()],
            "lockstep payload delivered byte-exact, ping/pong stripped"
        );

        // b → a, independent direction.
        b.send(b"lockstep-b-to-a");
        let on_a = a.poll();
        assert_eq!(on_a, vec![b"lockstep-b-to-a".to_vec()]);
    }

    #[test]
    fn empty_lockstep_frame_survives_the_envelope() {
        let (mut a, mut b, _ca, _cb, _samples) = pingpong_pair();
        a.send(b"");
        // The empty lockstep frame must come back as an empty frame, not be lost as "no datagram".
        assert_eq!(b.poll(), vec![Vec::<u8>::new()]);
    }

    #[test]
    fn ping_cadence_respects_the_interval() {
        // interval 1.0s: a ping at the first poll, none again until a second has elapsed.
        let (la, _lb) = LoopbackTransport::pair();
        let clock = FakeClock::new();
        let mut a = PingPongTransport::with_config(la, clock.clone(), 1.0, 8);

        clock.set(0.0);
        a.poll(); // first poll always pings
        assert_eq!(a.meter.outstanding_len(), 1);
        clock.set(0.5);
        a.poll(); // too soon — no new ping
        assert_eq!(a.meter.outstanding_len(), 1, "interval not elapsed");
        clock.set(1.0);
        a.poll(); // exactly one interval later — a fresh ping
        assert_eq!(a.meter.outstanding_len(), 2, "ping emitted after the interval");
    }

    #[test]
    fn samples_handle_shares_the_queue() {
        let (mut a, mut b, ca, _cb, samples) = pingpong_pair();
        let second_handle = a.samples();

        ca.set(0.0);
        a.poll();
        b.poll();
        ca.set(0.02);
        a.poll();

        // Draining via one handle empties the shared queue seen by the other.
        assert!(!second_handle.is_empty(), "both handles see the same queue");
        let drained = samples.drain();
        assert_eq!(drained.len(), 1);
        assert!(second_handle.is_empty(), "drain emptied the shared queue");
    }

    #[test]
    fn object_safe_as_dyn_transport() {
        // The host drives a `Box<dyn Transport>`; a PingPongTransport must coerce into it so it can
        // slot into `Game`'s transport field unchanged.
        let (la, _lb) = LoopbackTransport::pair();
        let a = PingPongTransport::new(la, SystemClock::new());
        let _boxed: Box<dyn Transport> = Box::new(a);
    }
}
