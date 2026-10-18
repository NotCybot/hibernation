use tokio::task::{JoinError, JoinSet};

pub struct FuturePool<O, F>
where
  F: Future<Output = O> + Send + 'static,
  O: Send + 'static,
{
  futures: Vec<F>,
}

impl<O, F> FuturePool<O, F>
where
  F: Future<Output = O> + Send + 'static,
  O: Send + 'static,
{
  pub fn new(futures: Vec<F>) -> Self {
    FuturePool { futures }
  }

  pub async fn run(self) -> Vec<Result<O, JoinError>> {
    let mut left = self.futures;
    let mut running = JoinSet::new();

    for _ in 0..20 {
      if let Some(future) = left.pop() {
        running.spawn(future);
      }
    }

    let mut results = Vec::new();
    while let Some(result) = running.join_next().await {
      results.push(result);
      if let Some(future) = left.pop() {
        running.spawn(future);
      }
    }

    results
  }
}

#[cfg(test)]
mod test {
  use std::{
    sync::{
      Arc,
      atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
  };

  use super::FuturePool;

  #[tokio::test]
  async fn empty_pool_returns_nothing() {
    let pool: FuturePool<(), std::future::Ready<()>> = FuturePool::new(Vec::new());
    assert!(pool.run().await.is_empty());
  }

  #[tokio::test]
  async fn returns_every_result() {
    let futures = (0..50).map(|i| async move { i }).collect::<Vec<_>>();
    let mut results = FuturePool::new(futures)
      .run()
      .await
      .into_iter()
      .map(Result::unwrap)
      .collect::<Vec<_>>();
    // Completion order is not guaranteed
    results.sort();
    assert_eq!(results, (0..50).collect::<Vec<_>>());
  }

  #[tokio::test]
  async fn panics_are_reported_not_propagated() {
    let futures: Vec<std::pin::Pin<Box<dyn Future<Output = u8> + Send>>> =
      vec![Box::pin(async { 1 }), Box::pin(async { panic!("boom") })];
    let results = FuturePool::new(futures).run().await;
    assert_eq!(results.len(), 2);
    assert_eq!(results.iter().filter(|r| r.is_err()).count(), 1);
  }

  #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
  async fn at_most_20_run_concurrently() {
    let running = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let futures = (0..60)
      .map(|_| {
        let running = running.clone();
        let peak = peak.clone();
        async move {
          let now = running.fetch_add(1, Ordering::SeqCst) + 1;
          peak.fetch_max(now, Ordering::SeqCst);
          tokio::time::sleep(Duration::from_millis(20)).await;
          running.fetch_sub(1, Ordering::SeqCst);
        }
      })
      .collect::<Vec<_>>();
    assert_eq!(FuturePool::new(futures).run().await.len(), 60);
    assert_eq!(peak.load(Ordering::SeqCst), 20);
  }
}
