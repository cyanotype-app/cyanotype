//! A byte budget shared by all in-flight jobs. The heavy-work semaphore bounds *how many* jobs run;
//! this bounds *how much memory* they may need together, so a few big images can't push the worker
//! past its container limit (an OOM kill would crash-loop the job instead of failing it cleanly).
use std::sync::Arc;

use anyhow::Result;
use cyanotype_core::metrics;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const UNIT: u64 = 1 << 20; // 1 MiB

#[derive(Clone)]
pub struct MemoryBudget {
    sem: Arc<Semaphore>,
    total_units: u64,
}

/// Held for as long as a job needs its memory; dropping it frees the bytes.
pub struct Reservation(OwnedSemaphorePermit);

impl Reservation {
    /// Give back everything above `bytes` (rounded up to MiB), keeping the rest reserved.
    pub fn shrink_to(&mut self, bytes: u64) {
        let keep = bytes.div_ceil(UNIT).max(1) as usize;
        let held = self.0.num_permits();
        if held > keep {
            drop(self.0.split(held - keep));
        }
    }
}

impl MemoryBudget {
    pub fn new(total_bytes: u64) -> Self {
        let total_units = (total_bytes / UNIT).clamp(1, u64::from(u32::MAX));
        Self {
            sem: Arc::new(Semaphore::new(total_units as usize)),
            total_units,
        }
    }

    pub fn total_bytes(&self) -> u64 {
        self.total_units * UNIT
    }

    /// Wait until `bytes` (rounded up to MiB, clamped to the whole budget) are free. Requests are
    /// served first-come first-served, so a big job is not starved by a stream of small ones.
    pub async fn reserve(&self, bytes: u64) -> Result<Reservation> {
        let units = bytes.div_ceil(UNIT).clamp(1, self.total_units) as u32;
        if (self.sem.available_permits() as u64) >= u64::from(units) {
            return Ok(Reservation(self.sem.clone().acquire_many_owned(units).await?));
        }
        // Not enough free memory right now: the job waits for others to finish. Counting these shows when the
        // budget (not the CPU or the network) is what limits throughput.
        let started = std::time::Instant::now();
        let held = self.sem.clone().acquire_many_owned(units).await?;
        let m = metrics::global();
        m.count("worker.memory_budget.waits", 1, &[]);
        m.histogram(
            "worker.memory_budget.wait_ms",
            started.elapsed().as_secs_f64() * 1000.0,
            &[],
        );
        Ok(Reservation(held))
    }

    /// Bytes currently reserved by in-flight jobs.
    pub fn used_bytes(&self) -> u64 {
        self.total_units.saturating_sub(self.sem.available_permits() as u64) * UNIT
    }

    #[cfg(test)]
    pub fn available_bytes(&self) -> u64 {
        self.sem.available_permits() as u64 * UNIT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reservations_block_until_released() {
        let b = MemoryBudget::new(100 * UNIT);
        let a = b.reserve(60 * UNIT).await.unwrap();
        assert_eq!(b.available_bytes(), 40 * UNIT);
        let b2 = b.clone();
        let waiter = tokio::spawn(async move { b2.reserve(60 * UNIT).await.unwrap() });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!waiter.is_finished(), "second 60 MB job must wait for the first");
        drop(a);
        let _second = waiter.await.unwrap();
        assert_eq!(b.available_bytes(), 40 * UNIT);
    }

    #[tokio::test]
    async fn shrink_returns_the_excess() {
        let b = MemoryBudget::new(100 * UNIT);
        let mut r = b.reserve(80 * UNIT).await.unwrap();
        r.shrink_to(30 * UNIT);
        assert_eq!(b.available_bytes(), 70 * UNIT);
        r.shrink_to(50 * UNIT); // never grows
        assert_eq!(b.available_bytes(), 70 * UNIT);
        drop(r);
        assert_eq!(b.available_bytes(), 100 * UNIT);
    }

    #[tokio::test]
    async fn oversized_request_is_clamped_to_the_whole_budget() {
        let b = MemoryBudget::new(10 * UNIT);
        let r = b.reserve(1 << 40).await.unwrap();
        assert_eq!(b.available_bytes(), 0);
        drop(r);
        assert_eq!(b.available_bytes(), 10 * UNIT);
    }
}
