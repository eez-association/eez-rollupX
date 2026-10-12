//! Keep the embedded L1's independent worker pools inside the CLI lifecycle.

use std::time::Duration;

use reth_tasks::{Runtime, RuntimeBuilder, RuntimeConfig, TokioConfig};

// Match the pinned Reth CLI's graceful-shutdown budget. The L1 and L2 drain
// concurrently; this is not an extension of the process shutdown deadline.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn build(parent: &Runtime) -> eyre::Result<Runtime> {
    let runtime = RuntimeBuilder::new(
        RuntimeConfig::default().with_tokio(TokioConfig::existing_handle(parent.handle().clone())),
    )
    .build()?;
    let manager = runtime
        .take_task_manager_handle()
        .ok_or_else(|| eyre::eyre!("embedded L1 task manager is missing"))?;
    // Reth reports a critical task failure through its panic monitor. Forward
    // that failure to the CLI instead of leaving only the embedded L1 stopped.
    parent.spawn_critical_task("eez-embedded-l1-tasks", async move {
        match manager.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => panic!("embedded L1 critical task failed: {error}"),
            Err(error) => panic!("embedded L1 task manager failed: {error}"),
        }
    });

    let child = runtime.clone();
    let worker = parent.clone();
    parent.spawn_critical_with_graceful_shutdown_signal(
        "eez-embedded-l1-shutdown",
        async move |shutdown| {
            let guard = shutdown.await;
            // Sharing Tokio threads does not share Reth's shutdown signal or
            // graceful-task counter. Drain the L1 on a blocking worker because
            // graceful_shutdown_with_timeout spins while its async tasks exit.
            // The worker retains the CLI guard even if its awaiter is cancelled.
            let result = worker
                .spawn_blocking(move || {
                    let _guard = guard;
                    child.graceful_shutdown_with_timeout(SHUTDOWN_TIMEOUT)
                })
                .await;
            match result {
                Ok(true) => tracing::debug!("embedded L1 graceful shutdown completed"),
                Ok(false) => tracing::warn!("embedded L1 graceful shutdown timed out"),
                Err(error) => tracing::error!(%error, "embedded L1 shutdown worker failed"),
            }
        },
    );
    Ok(runtime)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn cli_shutdown_waits_for_embedded_l1_to_finish() {
        let parent = RuntimeBuilder::new(RuntimeConfig::default())
            .build()
            .unwrap();
        let child = build(&parent).unwrap();
        let (started, observed) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        let (finished, completion) = mpsc::channel();
        let worker = child.clone();
        child.spawn_with_graceful_shutdown_signal(async move |shutdown| {
            let guard = shutdown.await;
            worker
                .spawn_blocking(move || {
                    let _guard = guard;
                    started.send(()).unwrap();
                    resume.recv().unwrap();
                })
                .await
                .unwrap();
        });
        let shutdown = parent.clone();
        let thread = std::thread::spawn(move || {
            finished
                .send(shutdown.graceful_shutdown_with_timeout(SHUTDOWN_TIMEOUT))
                .unwrap();
        });

        let received_shutdown = observed.recv_timeout(Duration::from_secs(3)).is_ok();
        let premature_completion = completion.recv_timeout(Duration::from_millis(250)).ok();
        // Release and drain the fixture before asserting, including on failure.
        let _ = release.send(());
        let completed =
            premature_completion.or_else(|| completion.recv_timeout(SHUTDOWN_TIMEOUT).ok());
        child.graceful_shutdown_with_timeout(SHUTDOWN_TIMEOUT);
        thread.join().unwrap();
        drop(child);
        parent.shutdown_timeout(SHUTDOWN_TIMEOUT);

        assert!(
            received_shutdown,
            "CLI shutdown did not reach the embedded L1"
        );
        assert!(
            premature_completion.is_none(),
            "CLI returned while the L1 was still draining"
        );
        assert_eq!(completed, Some(true));
    }

    #[test]
    fn embedded_l1_critical_failure_reaches_cli_monitor() {
        let parent = RuntimeBuilder::new(RuntimeConfig::default())
            .build()
            .unwrap();
        let child = build(&parent).unwrap();
        let manager = parent.take_task_manager_handle().unwrap();
        let (finished, completion) = mpsc::channel();
        parent.handle().spawn(async move {
            let _ = finished.send(manager.await);
        });
        child.spawn_critical_task("l1-fixture-failure", async {
            panic!("controlled embedded L1 failure");
        });
        let result = completion.recv_timeout(Duration::from_secs(3));
        child.graceful_shutdown_with_timeout(SHUTDOWN_TIMEOUT);
        parent.graceful_shutdown_with_timeout(SHUTDOWN_TIMEOUT);
        drop(child);
        parent.shutdown_timeout(SHUTDOWN_TIMEOUT);

        let error = result
            .expect("CLI did not observe the L1 failure")
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains("l1-fixture-failure"), "{error}");
    }
}
