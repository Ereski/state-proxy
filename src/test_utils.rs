use futures::{select, FutureExt};
use std::future::Future;
use tokio::time::{self, Duration};

const FUTURE_TIMEOUT: Duration = Duration::from_secs(30);

pub async fn panic_on_timeout<F, O>(future: F) -> O
where
    F: Future<Output = O>,
{
    select!(
        res = future.fuse() => Some(res),
        _ = time::sleep(FUTURE_TIMEOUT).fuse() => None
    )
    .expect("Future timed out")
}
