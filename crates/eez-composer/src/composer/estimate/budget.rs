//! Apply RPC work limits to both seeded and lazily opened target sessions.
use super::{Arc, CompositionBuilder, Instant, LocalChainClient};
use eez_protocol::{
    ChainClient, ExecutionOutcome, ExecutionRequest, ExecutorErrorKind, ExecutorResult,
    SessionSnapshot, TargetExecutionSession,
};

pub(super) struct Client {
    pub inner: Arc<LocalChainClient>,
    pub deadline: Instant,
}

impl ChainClient for Client {
    fn begin_execution_session(&self) -> ExecutorResult<Box<dyn TargetExecutionSession + Send>> {
        Ok(wrap(self.inner.begin_execution_session()?, self.deadline))
    }
}

struct Session {
    inner: Box<dyn TargetExecutionSession + Send>,
    deadline: Instant,
}

pub(super) fn wrap(
    inner: Box<dyn TargetExecutionSession + Send>,
    deadline: Instant,
) -> Box<dyn TargetExecutionSession + Send> {
    Box::new(Session { inner, deadline })
}

impl TargetExecutionSession for Session {
    fn execute(
        &mut self,
        req: ExecutionRequest,
        dispatcher: &mut CompositionBuilder,
    ) -> ExecutorResult<ExecutionOutcome> {
        // Each individual native frame also has its existing execution gas cap.
        // This bounds repeated/nested dispatch, including calls later reverted.
        if Instant::now() >= self.deadline || dispatcher.recorded_count() > 64 {
            return Err(ExecutorErrorKind::Unavailable(
                "wallet estimation execution budget exhausted".into(),
            )
            .into());
        }
        self.inner.execute(req, dispatcher)
    }
    fn checkpoint(&mut self) -> ExecutorResult<SessionSnapshot> {
        self.inner.checkpoint()
    }
    fn rollback(&mut self, snapshot: &SessionSnapshot) -> ExecutorResult<()> {
        self.inner.rollback(snapshot)
    }
}
