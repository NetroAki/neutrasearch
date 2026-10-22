//! FIFO admission for durable-store writers.
//! Tickets prevent replay from reacquiring ahead of waiting live events.
use std::io;
use std::sync::{Condvar, Mutex};

#[derive(Default)]
struct Turn {
    next: u64,
    serving: u64,
}

#[derive(Default)]
pub(crate) struct WriteGate {
    turn: Mutex<Turn>,
    wake: Condvar,
}

pub(crate) struct Ticket<'a> {
    gate: &'a WriteGate,
}

impl WriteGate {
    pub(crate) fn ticket(&self) -> io::Result<Ticket<'_>> {
        let mut turn = self
            .turn
            .lock()
            .map_err(|_| io::Error::other("write gate lock poisoned"))?;
        let mine = turn.next;
        turn.next = turn.next.wrapping_add(1);
        while turn.serving != mine {
            turn = self
                .wake
                .wait(turn)
                .map_err(|_| io::Error::other("write gate lock poisoned"))?;
        }
        Ok(Ticket { gate: self })
    }
}

impl Drop for Ticket<'_> {
    fn drop(&mut self) {
        let mut turn = self
            .gate
            .turn
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        turn.serving = turn.serving.wrapping_add(1);
        drop(turn);
        self.gate.wake.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};

    fn wait_for_queue(gate: &WriteGate, count: u64) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while gate.turn.lock().unwrap().next < count {
            assert!(Instant::now() < deadline, "writer did not queue");
            thread::yield_now();
        }
    }

    #[test]
    fn queued_writer_wins_before_reacquire() {
        let gate = Arc::new(WriteGate::default());
        let first = gate.ticket().unwrap();
        let (live_turn, live_rx) = channel();
        let live_gate = Arc::clone(&gate);
        let replay_turn = live_turn.clone();
        let live = thread::spawn(move || {
            let _ticket = live_gate.ticket().unwrap();
            live_turn.send("live").unwrap();
        });
        wait_for_queue(&gate, 2);
        let replay_gate = Arc::clone(&gate);
        let replay = thread::spawn(move || {
            let _ticket = replay_gate.ticket().unwrap();
            replay_turn.send("replay").unwrap();
        });
        wait_for_queue(&gate, 3);
        drop(first);
        assert_eq!(
            live_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            "live"
        );
        assert_eq!(
            live_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            "replay"
        );
        live.join().unwrap();
        replay.join().unwrap();
    }
}
