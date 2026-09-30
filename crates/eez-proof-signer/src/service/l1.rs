//! Canonical L1 authorization, distinct from stateless execution validity.
//! Reads are async under the existing request permit and absolute deadline.

use alloy_primitives::Address;
use eez_l1::{
    L1Reader, L1ReaderConfig,
    derivation::{
        DerivationError,
        proof::{AuthenticatedProofContext, ProofContextError},
    },
};
use eez_protocol::{
    l2::{ParentProvenance, Schedule},
    public_inputs::L1PostingContext,
};
use tonic::Status;

/// An operator-selected source. No Composer endpoint, claimed finality, or
/// production fallback can construct the test-only variant.
#[derive(Debug)]
// Production has only the canonical variant; tests add a zero-sized authority.
#[cfg_attr(test, allow(clippy::large_enum_variant))]
pub(crate) enum L1Authority {
    Canonical {
        reader: L1Reader,
        registry: Address,
        rollup_id: u64,
        schedule: Schedule,
    },
    #[cfg(test)]
    Synthetic,
}

impl L1Authority {
    pub(crate) fn new(
        rpc_url: url::Url,
        registry: Address,
        rollup_id: u64,
        schedule: Schedule,
    ) -> eyre::Result<Self> {
        eyre::ensure!(
            !registry.is_zero() && rollup_id != 0,
            "L1 registry and rollup identity must be nonzero"
        );
        let reader = L1Reader::new(L1ReaderConfig {
            rpc_url,
            eez: registry,
            rollup_id,
            deploy_block: schedule.config().l1_reference.number,
            beacon_url: None,
        });
        Ok(Self::Canonical {
            reader,
            registry,
            rollup_id,
            schedule,
        })
    }

    pub(super) async fn authenticate(
        &self,
        target: L1PostingContext,
        supplied: ParentProvenance,
    ) -> Result<CandidateContext, Status> {
        match self {
            Self::Canonical {
                reader,
                registry,
                rollup_id,
                schedule,
            } => {
                // Lazy authentication lets a signer start before the embedded
                // L1 is ready. Every Prove still requires finalized genesis and
                // fresh canonical facts; Capabilities grants no authority.
                let source = reader
                    .derivation_source(*schedule)
                    .await
                    .map_err(source_status)?;
                let context = source
                    .proof_context(*registry, *rollup_id, target, supplied)
                    .await
                    .map_err(proof_status)?;
                Ok(CandidateContext::Canonical(Box::new(context)))
            }
            #[cfg(test)]
            Self::Synthetic => Ok(CandidateContext::Synthetic),
        }
    }
}

pub(super) enum CandidateContext {
    Canonical(Box<AuthenticatedProofContext>),
    #[cfg(test)]
    Synthetic,
}

impl CandidateContext {
    /// Bind all independently replayed blocks (including cache hits) to the
    /// authenticated parent and descendant seed. Settlement later binds every
    /// root-update checkpoint before allowing a partially consumed prefix.
    pub(super) fn check_execution(
        &self,
        window: &crate::validate::ValidatedWindow,
    ) -> Result<(), &'static str> {
        let context = match self {
            Self::Canonical(context) => context,
            #[cfg(test)]
            Self::Synthetic => return Ok(()),
        };
        let count = u32::try_from(window.preceding_blocks().len() + 1)
            .map_err(|_| "candidate block count exceeds V0")?;
        context
            .check_candidate(count, false)
            .map_err(|error| error.0)?;
        check_execution(window, context.parent(), context.descendant_seed())
    }

    /// Run only after every root update is bound to independent execution and
    /// authorized actions. Registry equality with an unchecked claimed root is
    /// deliberately insufficient.
    pub(super) fn check_settlement(
        &self,
        effects: &crate::settlement::BoundEffectSequence<'_, '_>,
        block_count: u32,
        has_actions: bool,
    ) -> Result<(), &'static str> {
        let context = match self {
            Self::Canonical(context) => context,
            #[cfg(test)]
            Self::Synthetic => return Ok(()),
        };
        context
            .check_candidate(block_count, has_actions)
            .map_err(|error| error.0)?;
        if context.registry_root() == context.parent().hash_slow()
            || effects.contains_checkpoint(context.registry_root())
        {
            Ok(())
        } else {
            Err("canonical registry root is not the parent or a validated terminal prefix")
        }
    }
}

fn check_execution(
    window: &crate::validate::ValidatedWindow,
    parent: &alloy_consensus::Header,
    seed: alloy_primitives::B256,
) -> Result<(), &'static str> {
    if window.window_pre_block_hash() != parent.hash_slow() {
        return Err("window does not extend its authenticated parent");
    }
    for (offset, block) in window
        .preceding_blocks()
        .iter()
        .chain(std::iter::once(window.settling_block().block()))
        .enumerate()
    {
        let decoded: eez_primitives::Block = alloy_rlp::decode_exact(block.rlp())
            .map_err(|_| "validated block cannot be decoded")?;
        if decoded.header.mix_hash != seed
            || parent.number.checked_add(offset as u64 + 1) != Some(decoded.header.number)
        {
            return Err("candidate seed or position differs from authenticated parent");
        }
    }
    Ok(())
}

fn source_status(error: DerivationError) -> Status {
    match error {
        DerivationError::Unavailable(_) => Status::unavailable(error.to_string()),
        DerivationError::Mismatch(_) | DerivationError::Profile(_) => {
            Status::failed_precondition(error.to_string())
        }
    }
}
fn proof_status(error: ProofContextError) -> Status {
    match error {
        ProofContextError::Source(error) => source_status(error),
        ProofContextError::StaleTarget => Status::aborted("canonical L1 proof target changed"),
        ProofContextError::Invalid(_) => Status::failed_precondition(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validate::{
        SettlementBlockEvidence, ValidatedBlock, ValidatedSettlingBlock, ValidatedWindow,
    };
    use alloy_consensus::Header;
    use alloy_primitives::B256;

    // Synthetic replay facts isolate authorization from the EVM. Source tests
    // authenticate the seed; process tests exercise the combined pipeline.
    fn window(parent: &Header, seeds: [u8; 2], first_number: u64) -> ValidatedWindow {
        let mut previous = parent.hash_slow();
        let mut blocks = Vec::new();
        for (offset, seed) in seeds.into_iter().enumerate() {
            let number = first_number + offset as u64;
            let block = eez_primitives::Block::new(
                Header {
                    number,
                    parent_hash: previous,
                    mix_hash: B256::with_last_byte(seed),
                    ..Default::default()
                },
                Default::default(),
            );
            previous = block.header.hash_slow();
            blocks.push(ValidatedBlock::for_test(
                number,
                alloy_rlp::encode(block),
                SettlementBlockEvidence::for_test(vec![], vec![]),
            ));
        }
        let terminal = ValidatedSettlingBlock::for_test(blocks.pop().unwrap(), vec![], vec![]);
        ValidatedWindow::for_test(parent.hash_slow(), B256::ZERO, previous, blocks, terminal)
    }

    #[test]
    fn every_replayed_block_is_bound_to_authenticated_parent_and_seed() {
        let parent = Header {
            number: 4,
            mix_hash: B256::with_last_byte(7),
            ..Default::default()
        };
        let seed = B256::with_last_byte(8);
        check_execution(&window(&parent, [8, 8], 5), &parent, seed).unwrap();
        for seeds in [[7, 8], [8, 7], [7, 7]] {
            assert_eq!(
                check_execution(&window(&parent, seeds, 5), &parent, seed),
                Err("candidate seed or position differs from authenticated parent")
            );
        }
        assert_eq!(
            check_execution(&window(&parent, [8, 8], 6), &parent, seed),
            Err("candidate seed or position differs from authenticated parent")
        );
        let mut different = parent.clone();
        different.state_root = B256::with_last_byte(1);
        assert_eq!(
            check_execution(&window(&parent, [8, 8], 5), &different, seed),
            Err("window does not extend its authenticated parent")
        );
    }

    #[test]
    fn inconclusive_l1_and_reorg_statuses_never_authorize_eviction() {
        for (error, expected) in [
            (ProofContextError::StaleTarget, tonic::Code::Aborted),
            (
                ProofContextError::Source(DerivationError::Unavailable("header")),
                tonic::Code::Unavailable,
            ),
            (
                ProofContextError::Invalid("parent"),
                tonic::Code::FailedPrecondition,
            ),
        ] {
            let status = proof_status(error);
            assert_eq!(status.code(), expected);
            assert!(status.details().is_empty());
        }
    }
}
