//! The existing preparation future owns this guard; no new task or queue owner
//! is introduced. Dropping an unpolled wrapper cannot invent a started operation.
use super::{HeldPool, PreparationFacts};
use alloy_primitives::Bytes;
use eez_l1::L2Endpoint;
use eez_telemetry::{
    Publisher,
    schema::{Event, Hash32, PreparationId, PreparationOutcome},
};
use std::time::Instant;

struct Preparation {
    publisher: Publisher,
    id: PreparationId,
    started: Instant,
    finished: bool,
}

impl Preparation {
    fn finish(&mut self, outcome: PreparationOutcome) {
        // Set before publication so unwinding cannot report this operation twice.
        self.finished = true;
        if let Some(ticket) = self.publisher.capture() {
            self.publisher.publish(
                ticket,
                Event::PreparationFinished {
                    preparation: self.id,
                    outcome,
                    elapsed_ns: self
                        .started
                        .elapsed()
                        .as_nanos()
                        .try_into()
                        .unwrap_or(u64::MAX),
                },
                None,
            );
        }
    }
}

impl Drop for Preparation {
    fn drop(&mut self) {
        if !self.finished {
            self.finish(if std::thread::panicking() {
                PreparationOutcome::Panicked
            } else {
                PreparationOutcome::Cancelled
            });
        }
    }
}

impl HeldPool {
    fn start_preparation(
        &self,
        sync: L2Endpoint,
        members: &[crate::HeldTx],
    ) -> Option<Preparation> {
        let state = self.state.lock().expect("held_pool mutex poisoned");
        let lifecycle = state.lifecycle.as_ref()?;
        let ticket = lifecycle.publisher.capture()?;
        let preparation = Preparation {
            publisher: lifecycle.publisher.clone(),
            id: ticket.preparation_id(),
            started: Instant::now(),
            finished: false,
        };
        let event = lifecycle
            .members(members)
            .map(|members| Event::PreparationStarted {
                l2_chain_id: lifecycle.context.l2.chain_id,
                l2_genesis_hash: Hash32(lifecycle.context.l2.genesis_hash.0),
                rollup_id: lifecycle.context.rollup_id,
                sync: eez_telemetry::schema::BlockRef {
                    number: sync.number,
                    hash: Hash32(sync.hash.0),
                },
                members,
            });
        drop(state);
        match event {
            Ok(event) => {
                preparation.publisher.publish(ticket, event, None);
            }
            Err(failure) => preparation.publisher.discard(ticket, failure),
        }
        Some(preparation)
    }

    /// Observe the original future without changing its error, bytes or scheduling.
    /// Starts on first poll, finishes once on return/drop, and links successful
    /// facts to that preparation even if later ledger registration fails.
    pub(crate) fn observe_preparation<E>(
        &self,
        sync: L2Endpoint,
        members: &[crate::HeldTx],
        future: impl Future<Output = Result<(Bytes, PreparationFacts), E>>,
        failure: impl FnOnce(&E) -> PreparationOutcome,
    ) -> impl Future<Output = Result<(Bytes, PreparationFacts), E>> {
        // Box before constructing the wrapper so its unpolled state also stays
        // small. Otherwise the large builder future plus capture state inflates
        // every enclosing minimal-dispatch future above the repository limit.
        let future = Box::pin(future);
        async move {
            let mut preparation = self.start_preparation(sync, members);
            let mut result = future.await;
            if let Some(preparation) = &mut preparation {
                let outcome = match &mut result {
                    Ok((_, facts)) => {
                        facts.preparation = Some(preparation.id);
                        PreparationOutcome::Prepared
                    }
                    Err(error) => failure(error),
                };
                preparation.finish(outcome);
            }
            result
        }
    }
}
