//! Bounded CPU and disk work, separate from both GTK and Tokio's async workers.
use tokio::sync::Semaphore;

pub(crate) static CALCULATIONS: WorkQueue = WorkQueue::new(16);
/// The 500 ms meter tick. Details readers do not take the calculator mutex,
/// so the tick must not wait behind them in CALCULATIONS.
pub(crate) static DPS_TICK: WorkQueue = WorkQueue::new(1);
pub(crate) static HISTORY: WorkQueue = WorkQueue::new(16);

pub(crate) struct WorkQueue {
    admitted: Semaphore,
    running: Semaphore,
}

impl WorkQueue {
    const fn new(capacity: usize) -> Self {
        Self {
            admitted: Semaphore::const_new(capacity),
            running: Semaphore::const_new(1),
        }
    }

    pub(crate) async fn run<T, F>(&'static self, work: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let admitted = self
            .admitted
            .try_acquire()
            .map_err(|_| "Work queue is full; retry shortly".to_string())?;
        self.execute(admitted, work).await
    }

    /// Waits for a place instead of being refused. Only for a single internal
    /// producer that awaits each job before its next one, and for requests
    /// whose loss would go unseen: user writes (save or delete a fight) and
    /// the per-target Details reads a page merges into one total.
    pub(crate) async fn run_waiting<T, F>(&'static self, work: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let admitted = self.admitted.acquire().await.map_err(|e| e.to_string())?;
        self.execute(admitted, work).await
    }

    async fn execute<T, F>(
        &'static self,
        admitted: tokio::sync::SemaphorePermit<'static>,
        work: F,
    ) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let running = self.running.acquire().await.map_err(|e| e.to_string())?;
        tauri::async_runtime::spawn_blocking(move || {
            // Keep both permits until the actual work ends, even if the
            // awaiting request is cancelled: blocking jobs cannot be aborted.
            let _admitted = admitted;
            let _running = running;
            work()
        })
        .await
        .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn blocked_work_does_not_block_timers_and_cancel_keeps_capacity_reserved() {
        static QUEUE: WorkQueue = WorkQueue::new(1);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let job = tokio::spawn(QUEUE.run(move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        }));
        started_rx.await.unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            tokio::time::sleep(std::time::Duration::from_millis(10)),
        )
        .await
        .unwrap();
        job.abort();
        let _ = job.await;
        assert!(QUEUE.run(|| ()).await.is_err());
        let periodic = tokio::spawn(QUEUE.run_waiting(|| 42));
        release_tx.send(()).unwrap();
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(1), periodic)
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            42
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn tick_queue_runs_while_details_queue_is_busy() {
        static DETAILS: WorkQueue = WorkQueue::new(16);
        static TICK: WorkQueue = WorkQueue::new(1);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let details = tokio::spawn(DETAILS.run(move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        }));
        started_rx.await.unwrap();
        let queued_details = tokio::spawn(DETAILS.run(|| 1));
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(1), TICK.run_waiting(|| 42))
                .await
                .expect("tick waited behind details work")
                .unwrap(),
            42
        );
        release_tx.send(()).unwrap();
        details.await.unwrap().unwrap();
        assert_eq!(queued_details.await.unwrap().unwrap(), 1);
    }
}
