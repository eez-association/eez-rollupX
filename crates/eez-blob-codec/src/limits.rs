//! Network blob bounds shared by outgoing packing and historical admission.
//! Fork selection is supplied by the configured execution-chain adapter.

use alloy_eips::eip7840::BlobParams;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlobLimits {
    max_per_transaction: u64,
    max_per_block: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BlobLimitError {
    #[error("active network or transport blob allowance is zero or unknown")]
    Unavailable,
    #[error("required companion blobs exhaust the L1 block allowance")]
    CompanionsExhaustCapacity,
    #[error("blob count {actual} exceeds the containing fork transaction allowance {maximum}")]
    TooMany { actual: u64, maximum: u64 },
}

impl BlobLimits {
    /// Use maxima, never the fee-market target. There is no universal six-blob
    /// rule: the selected fork's parameters define both independent bounds.
    pub fn from_params(params: BlobParams) -> Result<Self, BlobLimitError> {
        if params.max_blobs_per_tx == 0 || params.max_blob_count == 0 {
            return Err(BlobLimitError::Unavailable);
        }
        Ok(Self {
            max_per_transaction: params.max_blobs_per_tx,
            max_per_block: params.max_blob_count,
        })
    }

    /// Outgoing allowance after reserving only our required companion blobs.
    /// Operator caps govern posting; historical validation never receives them.
    pub fn outgoing(
        self,
        companion_blobs: u64,
        transport_max: u64,
        operational_cap: Option<u64>,
    ) -> Result<u64, BlobLimitError> {
        if transport_max == 0 || operational_cap == Some(0) {
            return Err(BlobLimitError::Unavailable);
        }
        let remaining = self
            .max_per_block
            .checked_sub(companion_blobs)
            .filter(|remaining| *remaining > 0)
            .ok_or(BlobLimitError::CompanionsExhaustCapacity)?;
        Ok(self
            .max_per_transaction
            .min(remaining)
            .min(transport_max)
            .min(operational_cap.unwrap_or(u64::MAX)))
    }

    /// Validate an independently supplied transaction against its containing
    /// L1 fork, regardless of the local composer's preferred posting cap.
    pub fn validate_historical(self, count: u64) -> Result<(), BlobLimitError> {
        let maximum = self.max_per_transaction.min(self.max_per_block);
        if count == 0 {
            return Err(BlobLimitError::Unavailable);
        }
        if count > maximum {
            return Err(BlobLimitError::TooMany {
                actual: count,
                maximum,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_actual_bounds_contribute_without_using_fee_target() {
        let params = BlobParams {
            target_blob_count: 1,
            max_blob_count: 12,
            max_blobs_per_tx: 10,
            ..BlobParams::prague()
        };
        let limits = BlobLimits::from_params(params).unwrap();
        assert_eq!(limits.outgoing(0, 20, None).unwrap(), 10);
        assert_eq!(limits.outgoing(4, 20, None).unwrap(), 8);
        assert_eq!(limits.outgoing(0, 7, None).unwrap(), 7);
        assert_eq!(limits.outgoing(0, 20, Some(2)).unwrap(), 2);
        assert!(limits.validate_historical(10).is_ok());
        assert!(limits.validate_historical(11).is_err());
        for companions in [12, 13, u64::MAX] {
            assert!(limits.outgoing(companions, 20, None).is_err());
        }
        assert!(limits.outgoing(0, 0, None).is_err());
        assert!(limits.outgoing(0, 20, Some(0)).is_err());
    }

    #[test]
    fn fork_limits_are_distinct_and_unknown_rules_fail_closed() {
        let before = BlobLimits::from_params(BlobParams::prague()).unwrap();
        let after = BlobLimits::from_params(BlobParams::osaka()).unwrap();
        assert!(before.validate_historical(9).is_ok());
        assert!(after.validate_historical(9).is_err());
        assert!(after.validate_historical(6).is_ok());
        assert!(
            BlobLimits::from_params(BlobParams {
                max_blob_count: 0,
                ..BlobParams::prague()
            })
            .is_err()
        );
        assert!(
            BlobLimits::from_params(BlobParams {
                max_blobs_per_tx: 0,
                ..BlobParams::prague()
            })
            .is_err()
        );
    }
}
