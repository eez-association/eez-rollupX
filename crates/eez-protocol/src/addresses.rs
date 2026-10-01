//! Well-known addresses shared across the eez stack.

use alloy_primitives::{Address, address};

/// The `EEZL2` manager: an upgradeable `EEZProxy` genesis predeploy. Every
/// system transaction targets this address, which holds the manager's storage,
/// balance and events, and deploys its cross-chain proxies.
pub const EEZL2_ADDRESS: Address = address!("0x4B2760F84B83d6b3829E30A7573B0172B6d6402C");
