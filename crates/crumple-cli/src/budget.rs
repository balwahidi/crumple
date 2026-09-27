//! Counting-semaphore memory gate (ARCHITECTURE §5).

use std::sync::{Condvar, Mutex};

pub(crate) struct MemoryBudget {
    capacity: u64,
    in_use: Mutex<u64>,
    freed: Condvar,
}

pub(crate) struct Guard<'a> {
    budget: &'a MemoryBudget,
    taken: u64,
}

impl MemoryBudget {
    pub(crate) fn new(capacity: u64) -> Self {
        Self {
            capacity,
            in_use: Mutex::new(0),
            freed: Condvar::new(),
        }
    }

    /// Blocks until `cost` fits. A cost above the capacity waits until nothing
    /// is in flight and then takes the whole budget, so it runs alone.
    ///
    /// A cost of 0 never waits, even beside an oversize item, because it takes nothing.
    /// Never call this while the same thread already holds a guard: an oversize
    /// item would wait for itself forever.
    pub(crate) fn acquire(&self, cost: u64) -> Guard<'_> {
        let mut in_use = self.in_use.lock().unwrap_or_else(|e| e.into_inner());
        let taken = if cost > self.capacity {
            while *in_use != 0 {
                in_use = self.freed.wait(in_use).unwrap_or_else(|e| e.into_inner());
            }
            self.capacity.max(1)
        } else {
            // Unlike `in_use + cost > capacity`, this cannot overflow for a huge
            // --max-memory. (Saturating only matters for a capacity of 0.)
            while cost > self.capacity.saturating_sub(*in_use) {
                in_use = self.freed.wait(in_use).unwrap_or_else(|e| e.into_inner());
            }
            cost
        };
        *in_use += taken;
        Guard {
            budget: self,
            taken,
        }
    }
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        let mut in_use = self.budget.in_use.lock().unwrap_or_else(|e| e.into_inner());
        *in_use -= self.taken;
        drop(in_use);
        // notify_all, not notify_one: the one waiter woken might not fit yet while
        // another, smaller one would.
        self.budget.freed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{mpsc, Arc};
    use std::time::{Duration, Instant};

    const CAP: u64 = 1000;

    /// Runs `f` on another thread and fails (instead of hanging the test run) if it
    /// does not finish in time, which is how a lost wakeup or deadlock shows up.
    fn within(limit: Duration, f: impl FnOnce() + Send + 'static) {
        let (tx, rx) = mpsc::channel();
        let h = std::thread::spawn(move || {
            f();
            let _ = tx.send(());
        });
        match rx.recv_timeout(limit) {
            Ok(()) => h.join().unwrap(),
            // The worker panicked (an assertion failed): surface that panic.
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                std::panic::resume_unwind(h.join().unwrap_err())
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!("did not finish in {limit:?}: deadlock or lost wakeup")
            }
        }
    }

    /// Busy-holds a guard for a few microseconds so that holders really overlap.
    fn hold(n: u64) {
        let until = Instant::now() + Duration::from_micros(n);
        while Instant::now() < until {
            std::hint::spin_loop();
        }
    }

    #[test]
    fn stress_never_exceeds_capacity() {
        within(Duration::from_secs(10), || {
            let budget = Arc::new(MemoryBudget::new(CAP));
            let in_use = Arc::new(AtomicU64::new(0));
            let holders = Arc::new(AtomicU64::new(0));
            let peak_in_use = Arc::new(AtomicU64::new(0));
            let peak_holders = Arc::new(AtomicU64::new(0));
            let handles: Vec<_> = (0..16u64)
                .map(|t| {
                    let budget = Arc::clone(&budget);
                    let in_use = Arc::clone(&in_use);
                    let holders = Arc::clone(&holders);
                    let peak_in_use = Arc::clone(&peak_in_use);
                    let peak_holders = Arc::clone(&peak_holders);
                    std::thread::spawn(move || {
                        let mut lcg: u64 = 0x9E37_79B9 ^ (t * 2_654_435_761 + 1);
                        for i in 0..200 {
                            lcg = lcg
                                .wrapping_mul(6_364_136_223_846_793_005)
                                .wrapping_add(1_442_695_040_888_963_407);
                            let r = lcg >> 33;
                            // Costs start at 1: a cost-0 item takes nothing, so it may run beside an
                            // oversize one, which the "alone" assertions below would flag.
                            let cost = if (i + t) % 37 == 0 { 1500 } else { 1 + r % 600 };
                            let guard = budget.acquire(cost);
                            let prev = in_use.fetch_add(cost, Ordering::SeqCst);
                            let h = holders.fetch_add(1, Ordering::SeqCst) + 1;
                            if cost > CAP {
                                assert_eq!(prev, 0, "oversize item must run alone");
                                assert_eq!(h, 1, "oversize item must run alone");
                            } else {
                                assert!(prev + cost <= CAP, "in_use {} > capacity", prev + cost);
                                peak_in_use.fetch_max(prev + cost, Ordering::SeqCst);
                            }
                            peak_holders.fetch_max(h, Ordering::SeqCst);
                            hold(r % 50);
                            holders.fetch_sub(1, Ordering::SeqCst);
                            in_use.fetch_sub(cost, Ordering::SeqCst);
                            drop(guard);
                        }
                    })
                })
                .collect();
            for h in handles {
                if let Err(p) = h.join() {
                    std::panic::resume_unwind(p);
                }
            }
            assert_eq!(in_use.load(Ordering::SeqCst), 0);
            // The test only proves something if holders overlapped and the budget filled up.
            assert!(peak_holders.load(Ordering::SeqCst) >= 2, "no contention");
            assert!(
                peak_in_use.load(Ordering::SeqCst) > CAP / 2,
                "budget never filled"
            );
        });
    }

    #[test]
    fn oversize_waits_for_idle_then_blocks_others() {
        within(Duration::from_secs(10), || {
            let budget = Arc::new(MemoryBudget::new(CAP));
            let small = budget.acquire(10);
            let (tx, rx) = mpsc::channel();
            let b = Arc::clone(&budget);
            let running = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let r = Arc::clone(&running);
            let big = std::thread::spawn(move || {
                let g = b.acquire(5 * CAP);
                r.store(true, Ordering::SeqCst);
                tx.send(()).unwrap();
                hold(20_000);
                r.store(false, Ordering::SeqCst);
                drop(g);
            });
            // The oversize item must not start while `small` is held.
            assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
            drop(small);
            rx.recv_timeout(Duration::from_secs(5)).unwrap();
            // While it runs, even a cost-1 item waits. (`running` is cleared just before
            // the oversize guard drops, so a correct gate can never observe it set.)
            let g = budget.acquire(1);
            assert!(!running.load(Ordering::SeqCst), "ran beside oversize");
            drop(g);
            big.join().unwrap();
        });
    }

    #[test]
    fn release_wakes_a_waiter_that_fits() {
        within(Duration::from_secs(10), || {
            let budget = Arc::new(MemoryBudget::new(CAP));
            let a = budget.acquire(500);
            let b = budget.acquire(500);
            // Oversize waiters queue first; they cannot run until everything is released.
            let big: Vec<_> = (0..4)
                .map(|_| {
                    let bb = Arc::clone(&budget);
                    std::thread::spawn(move || drop(bb.acquire(5 * CAP)))
                })
                .collect();
            std::thread::sleep(Duration::from_millis(50));
            let (tx, rx) = mpsc::channel();
            let bb = Arc::clone(&budget);
            let fits = std::thread::spawn(move || {
                let g = bb.acquire(400);
                tx.send(()).unwrap();
                drop(g);
            });
            std::thread::sleep(Duration::from_millis(50));
            drop(a);
            // 500 + 400 fits beside `b`. With notify_one the wakeup can go to an
            // oversize waiter, which goes back to sleep, and this one stays blocked.
            rx.recv_timeout(Duration::from_secs(2))
                .expect("a released budget must wake every waiter that fits");
            drop(b);
            fits.join().unwrap();
            for h in big {
                h.join().unwrap();
            }
        });
    }

    #[test]
    fn panic_while_holding_releases() {
        within(Duration::from_secs(10), || {
            let budget = Arc::new(MemoryBudget::new(CAP));
            let b = Arc::clone(&budget);
            let r = std::thread::spawn(move || {
                let _g = b.acquire(CAP);
                panic!("pipeline bug");
            })
            .join();
            assert!(r.is_err());
            // The whole budget is free again, including for an oversize item.
            drop(budget.acquire(CAP));
            drop(budget.acquire(10 * CAP));
        });
    }

    #[test]
    fn huge_capacity_does_not_overflow() {
        within(Duration::from_secs(10), || {
            // `--max-memory 18446744073709551615` is accepted; `in_use + cost` would
            // overflow here instead of waiting.
            let budget = Arc::new(MemoryBudget::new(u64::MAX));
            let a = budget.acquire(u64::MAX - 5);
            let b = Arc::clone(&budget);
            let (tx, rx) = mpsc::channel();
            let waiter = std::thread::spawn(move || {
                let _g = b.acquire(10);
                tx.send(()).unwrap();
            });
            assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
            drop(a);
            rx.recv_timeout(Duration::from_secs(5)).unwrap();
            waiter.join().unwrap();
        });
    }
}
