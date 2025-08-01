//! Transaction pool errors

use std::any::Any;

use alloy_eips::eip4844::BlobTransactionValidationError;
use alloy_primitives::{Address, TxHash, U256};
use reth_primitives_traits::transaction::error::InvalidTransactionError;

/// Transaction pool result type.
pub type PoolResult<T> = Result<T, PoolError>;

/// A trait for additional errors that can be thrown by the transaction pool.
///
/// For example during validation
/// [`TransactionValidator::validate_transaction`](crate::validate::TransactionValidator::validate_transaction)
pub trait PoolTransactionError: core::error::Error + Send + Sync {
    /// Returns `true` if the error was caused by a transaction that is considered bad in the
    /// context of the transaction pool and warrants peer penalization.
    ///
    /// See [`PoolError::is_bad_transaction`].
    fn is_bad_transaction(&self) -> bool;

    /// Returns a reference to `self` as a `&dyn Any`, enabling downcasting.
    fn as_any(&self) -> &dyn Any;
}

// Needed for `#[error(transparent)]`
impl core::error::Error for Box<dyn PoolTransactionError> {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        (**self).source()
    }
}

/// Transaction pool error.
#[derive(Debug, thiserror::Error)]
#[error("[{hash}]: {kind}")]
pub struct PoolError {
    /// The transaction hash that caused the error.
    pub hash: TxHash,
    /// The error kind.
    pub kind: PoolErrorKind,
}

/// Transaction pool error kind.
#[derive(Debug, thiserror::Error)]
pub enum PoolErrorKind {
    /// Same transaction already imported
    #[error("already imported")]
    AlreadyImported,
    /// Thrown if a replacement transaction's gas price is below the already imported transaction
    #[error("insufficient gas price to replace existing transaction")]
    ReplacementUnderpriced,
    /// The fee cap of the transaction is below the minimum fee cap determined by the protocol
    #[error("transaction feeCap {0} below chain minimum")]
    FeeCapBelowMinimumProtocolFeeCap(u128),
    /// Thrown when the number of unique transactions of a sender exceeded the slot capacity.
    #[error("rejected due to {0} being identified as a spammer")]
    SpammerExceededCapacity(Address),
    /// Thrown when a new transaction is added to the pool, but then immediately discarded to
    /// respect the size limits of the pool.
    #[error("transaction discarded outright due to pool size constraints")]
    DiscardedOnInsert,
    /// Thrown when the transaction is considered invalid.
    #[error(transparent)]
    InvalidTransaction(#[from] InvalidPoolTransactionError),
    /// Thrown if the mutual exclusivity constraint (blob vs normal transaction) is violated.
    #[error("transaction type {1} conflicts with existing transaction for {0}")]
    ExistingConflictingTransactionType(Address, u8),
    /// Any other error that occurred while inserting/validating a transaction. e.g. IO database
    /// error
    #[error(transparent)]
    Other(#[from] Box<dyn core::error::Error + Send + Sync>),
}

// === impl PoolError ===

impl PoolError {
    /// Creates a new pool error.
    pub fn new(hash: TxHash, kind: impl Into<PoolErrorKind>) -> Self {
        Self { hash, kind: kind.into() }
    }

    /// Creates a new pool error with the `Other` kind.
    pub fn other(
        hash: TxHash,
        error: impl Into<Box<dyn core::error::Error + Send + Sync>>,
    ) -> Self {
        Self { hash, kind: PoolErrorKind::Other(error.into()) }
    }

    /// Returns `true` if the error was caused by a transaction that is considered bad in the
    /// context of the transaction pool and warrants peer penalization.
    ///
    /// Not all error variants are caused by the incorrect composition of the transaction (See also
    /// [`InvalidPoolTransactionError`]) and can be caused by the current state of the transaction
    /// pool. For example the transaction pool is already full or the error was caused my an
    /// internal error, such as database errors.
    ///
    /// This function returns true only if the transaction will never make it into the pool because
    /// its composition is invalid and the original sender should have detected this as well. This
    /// is used to determine whether the original sender should be penalized for sending an
    /// erroneous transaction.
    #[inline]
    pub fn is_bad_transaction(&self) -> bool {
        #[expect(clippy::match_same_arms)]
        match &self.kind {
            PoolErrorKind::AlreadyImported => {
                // already imported but not bad
                false
            }
            PoolErrorKind::ReplacementUnderpriced => {
                // already imported but not bad
                false
            }
            PoolErrorKind::FeeCapBelowMinimumProtocolFeeCap(_) => {
                // fee cap of the tx below the technical minimum determined by the protocol, see
                // [MINIMUM_PROTOCOL_FEE_CAP](alloy_primitives::constants::MIN_PROTOCOL_BASE_FEE)
                // although this transaction will always be invalid, we do not want to penalize the
                // sender because this check simply could not be implemented by the client
                false
            }
            PoolErrorKind::SpammerExceededCapacity(_) => {
                // the sender exceeded the slot capacity, we should not penalize the peer for
                // sending the tx because we don't know if all the transactions are sent from the
                // same peer, there's also a chance that old transactions haven't been cleared yet
                // (pool lags behind) and old transaction still occupy a slot in the pool
                false
            }
            PoolErrorKind::DiscardedOnInsert => {
                // valid tx but dropped due to size constraints
                false
            }
            PoolErrorKind::InvalidTransaction(err) => {
                // transaction rejected because it violates constraints
                err.is_bad_transaction()
            }
            PoolErrorKind::Other(_) => {
                // internal error unrelated to the transaction
                false
            }
            PoolErrorKind::ExistingConflictingTransactionType(_, _) => {
                // this is not a protocol error but an implementation error since the pool enforces
                // exclusivity (blob vs normal tx) for all senders
                false
            }
        }
    }
}

/// Represents all errors that can happen when validating transactions for the pool for EIP-4844
/// transactions
#[derive(Debug, thiserror::Error)]
pub enum Eip4844PoolTransactionError {
    /// Thrown if we're unable to find the blob for a transaction that was previously extracted
    #[error("blob sidecar not found for EIP4844 transaction")]
    MissingEip4844BlobSidecar,
    /// Thrown if an EIP-4844 transaction without any blobs arrives
    #[error("blobless blob transaction")]
    NoEip4844Blobs,
    /// Thrown if an EIP-4844 transaction without any blobs arrives
    #[error("too many blobs in transaction: have {have}, permitted {permitted}")]
    TooManyEip4844Blobs {
        /// Number of blobs the transaction has
        have: u64,
        /// Number of maximum blobs the transaction can have
        permitted: u64,
    },
    /// Thrown if validating the blob sidecar for the transaction failed.
    #[error(transparent)]
    InvalidEip4844Blob(BlobTransactionValidationError),
    /// EIP-4844 transactions are only accepted if they're gapless, meaning the previous nonce of
    /// the transaction (`tx.nonce -1`) must either be in the pool or match the on chain nonce of
    /// the sender.
    ///
    /// This error is thrown on validation if a valid blob transaction arrives with a nonce that
    /// would introduce gap in the nonce sequence.
    #[error("nonce too high")]
    Eip4844NonceGap,
    /// Thrown if blob transaction has an EIP-7594 style sidecar before Osaka.
    #[error("unexpected eip-7594 sidecar before osaka")]
    UnexpectedEip7594SidecarBeforeOsaka,
    /// Thrown if blob transaction has an EIP-4844 style sidecar after Osaka.
    #[error("unexpected eip-4844 sidecar after osaka")]
    UnexpectedEip4844SidecarAfterOsaka,
}

/// Represents all errors that can happen when validating transactions for the pool for EIP-7702
/// transactions
#[derive(Debug, thiserror::Error)]
pub enum Eip7702PoolTransactionError {
    /// Thrown if the transaction has no items in its authorization list
    #[error("no items in authorization list for EIP7702 transaction")]
    MissingEip7702AuthorizationList,
    /// Returned when a transaction with a nonce
    /// gap is received from accounts with a deployed delegation or pending delegation.
    #[error("gapped-nonce tx from delegated accounts")]
    OutOfOrderTxFromDelegated,
    /// Returned when the maximum number of in-flight
    /// transactions is reached for specific accounts.
    #[error("in-flight transaction limit reached for delegated accounts")]
    InflightTxLimitReached,
    /// Returned if a transaction has an authorization
    /// signed by an address which already has in-flight transactions known to the
    /// pool.
    #[error("authority already reserved")]
    AuthorityReserved,
}

/// Represents errors that can happen when validating transactions for the pool
///
/// See [`TransactionValidator`](crate::TransactionValidator).
#[derive(Debug, thiserror::Error)]
pub enum InvalidPoolTransactionError {
    /// Hard consensus errors
    #[error(transparent)]
    Consensus(#[from] InvalidTransactionError),
    /// Thrown when a new transaction is added to the pool, but then immediately discarded to
    /// respect the size limits of the pool.
    #[error("transaction's gas limit {0} exceeds block's gas limit {1}")]
    ExceedsGasLimit(u64, u64),
    /// Thrown when a transaction's gas limit exceeds the configured maximum per-transaction limit.
    #[error("transaction's gas limit {0} exceeds maximum per-transaction gas limit {1}")]
    MaxTxGasLimitExceeded(u64, u64),
    /// Thrown when a new transaction is added to the pool, but then immediately discarded to
    /// respect the tx fee exceeds the configured cap
    #[error("tx fee ({max_tx_fee_wei} wei) exceeds the configured cap ({tx_fee_cap_wei} wei)")]
    ExceedsFeeCap {
        /// max fee in wei of new tx submitted to the pull (e.g. 0.11534 ETH)
        max_tx_fee_wei: u128,
        /// configured tx fee cap in wei (e.g. 1.0 ETH)
        tx_fee_cap_wei: u128,
    },
    /// Thrown when a new transaction is added to the pool, but then immediately discarded to
    /// respect the `max_init_code_size`.
    #[error("transaction's input size {0} exceeds max_init_code_size {1}")]
    ExceedsMaxInitCodeSize(usize, usize),
    /// Thrown if the input data of a transaction is greater
    /// than some meaningful limit a user might use. This is not a consensus error
    /// making the transaction invalid, rather a DOS protection.
    #[error("input data too large")]
    OversizedData(usize, usize),
    /// Thrown if the transaction's fee is below the minimum fee
    #[error("transaction underpriced")]
    Underpriced,
    /// Thrown if the transaction's would require an account to be overdrawn
    #[error("transaction overdraws from account, balance: {balance}, cost: {cost}")]
    Overdraft {
        /// Cost transaction is allowed to consume. See `reth_transaction_pool::PoolTransaction`.
        cost: U256,
        /// Balance of account.
        balance: U256,
    },
    /// EIP-2681 error thrown if the nonce is higher or equal than `U64::max`
    /// `<https://eips.ethereum.org/EIPS/eip-2681>`
    #[error("nonce exceeds u64 limit")]
    Eip2681,
    /// EIP-4844 related errors
    #[error(transparent)]
    Eip4844(#[from] Eip4844PoolTransactionError),
    /// EIP-7702 related errors
    #[error(transparent)]
    Eip7702(#[from] Eip7702PoolTransactionError),
    /// Any other error that occurred while inserting/validating that is transaction specific
    #[error(transparent)]
    Other(Box<dyn PoolTransactionError>),
    /// The transaction is specified to use less gas than required to start the
    /// invocation.
    #[error("intrinsic gas too low")]
    IntrinsicGasTooLow,
    /// The transaction priority fee is below the minimum required priority fee.
    #[error("transaction priority fee below minimum required priority fee {minimum_priority_fee}")]
    PriorityFeeBelowMinimum {
        /// Minimum required priority fee.
        minimum_priority_fee: u128,
    },
}

// === impl InvalidPoolTransactionError ===

impl InvalidPoolTransactionError {
    /// Returns a new [`InvalidPoolTransactionError::Other`] instance with the given
    /// [`PoolTransactionError`].
    pub fn other<E: PoolTransactionError + 'static>(err: E) -> Self {
        Self::Other(Box::new(err))
    }

    /// Returns `true` if the error was caused by a transaction that is considered bad in the
    /// context of the transaction pool and warrants peer penalization.
    ///
    /// See [`PoolError::is_bad_transaction`].
    #[expect(clippy::match_same_arms)]
    #[inline]
    fn is_bad_transaction(&self) -> bool {
        match self {
            Self::Consensus(err) => {
                // transaction considered invalid by the consensus rules
                // We do not consider the following errors to be erroneous transactions, since they
                // depend on dynamic environmental conditions and should not be assumed to have been
                // intentionally caused by the sender
                match err {
                    InvalidTransactionError::InsufficientFunds { .. } |
                    InvalidTransactionError::NonceNotConsistent { .. } => {
                        // transaction could just have arrived late/early
                        false
                    }
                    InvalidTransactionError::GasTooLow |
                    InvalidTransactionError::GasTooHigh |
                    InvalidTransactionError::TipAboveFeeCap => {
                        // these are technically not invalid
                        false
                    }
                    InvalidTransactionError::FeeCapTooLow => {
                        // dynamic, but not used during validation
                        false
                    }
                    InvalidTransactionError::Eip2930Disabled |
                    InvalidTransactionError::Eip1559Disabled |
                    InvalidTransactionError::Eip4844Disabled |
                    InvalidTransactionError::Eip7702Disabled => {
                        // settings
                        false
                    }
                    InvalidTransactionError::OldLegacyChainId |
                    InvalidTransactionError::ChainIdMismatch |
                    InvalidTransactionError::GasUintOverflow |
                    InvalidTransactionError::TxTypeNotSupported |
                    InvalidTransactionError::SignerAccountHasBytecode |
                    InvalidTransactionError::GasLimitTooHigh => true,
                }
            }
            Self::ExceedsGasLimit(_, _) => true,
            Self::MaxTxGasLimitExceeded(_, _) => {
                // local setting
                false
            }
            Self::ExceedsFeeCap { max_tx_fee_wei: _, tx_fee_cap_wei: _ } => true,
            Self::ExceedsMaxInitCodeSize(_, _) => true,
            Self::OversizedData(_, _) => true,
            Self::Underpriced => {
                // local setting
                false
            }
            Self::IntrinsicGasTooLow => true,
            Self::Overdraft { .. } => false,
            Self::Other(err) => err.is_bad_transaction(),
            Self::Eip2681 => true,
            Self::Eip4844(eip4844_err) => {
                match eip4844_err {
                    Eip4844PoolTransactionError::MissingEip4844BlobSidecar => {
                        // this is only reachable when blob transactions are reinjected and we're
                        // unable to find the previously extracted blob
                        false
                    }
                    Eip4844PoolTransactionError::InvalidEip4844Blob(_) => {
                        // This is only reachable when the blob is invalid
                        true
                    }
                    Eip4844PoolTransactionError::Eip4844NonceGap => {
                        // it is possible that the pool sees `nonce n` before `nonce n-1` and this
                        // is only thrown for valid(good) blob transactions
                        false
                    }
                    Eip4844PoolTransactionError::NoEip4844Blobs => {
                        // this is a malformed transaction and should not be sent over the network
                        true
                    }
                    Eip4844PoolTransactionError::TooManyEip4844Blobs { .. } => {
                        // this is a malformed transaction and should not be sent over the network
                        true
                    }
                    Eip4844PoolTransactionError::UnexpectedEip4844SidecarAfterOsaka |
                    Eip4844PoolTransactionError::UnexpectedEip7594SidecarBeforeOsaka => {
                        // for now we do not want to penalize peers for broadcasting different
                        // sidecars
                        false
                    }
                }
            }
            Self::Eip7702(eip7702_err) => match eip7702_err {
                Eip7702PoolTransactionError::MissingEip7702AuthorizationList => {
                    // as EIP-7702 specifies, 7702 transactions must have an non-empty authorization
                    // list so this is a malformed transaction and should not be
                    // sent over the network
                    true
                }
                Eip7702PoolTransactionError::OutOfOrderTxFromDelegated => false,
                Eip7702PoolTransactionError::InflightTxLimitReached => false,
                Eip7702PoolTransactionError::AuthorityReserved => false,
            },
            Self::PriorityFeeBelowMinimum { .. } => false,
        }
    }

    /// Returns `true` if an import failed due to an oversized transaction
    pub const fn is_oversized(&self) -> bool {
        matches!(self, Self::OversizedData(_, _))
    }

    /// Returns `true` if an import failed due to nonce gap.
    pub const fn is_nonce_gap(&self) -> bool {
        matches!(self, Self::Consensus(InvalidTransactionError::NonceNotConsistent { .. })) ||
            matches!(self, Self::Eip4844(Eip4844PoolTransactionError::Eip4844NonceGap))
    }

    /// Returns the arbitrary error if it is [`InvalidPoolTransactionError::Other`]
    pub fn as_other(&self) -> Option<&dyn PoolTransactionError> {
        match self {
            Self::Other(err) => Some(&**err),
            _ => None,
        }
    }

    /// Returns a reference to the [`InvalidPoolTransactionError::Other`] value if this type is a
    /// [`InvalidPoolTransactionError::Other`] of that type. Returns None otherwise.
    pub fn downcast_other_ref<T: core::error::Error + 'static>(&self) -> Option<&T> {
        let other = self.as_other()?;
        other.as_any().downcast_ref()
    }

    /// Returns true if the this type is a [`InvalidPoolTransactionError::Other`] of that error
    /// type. Returns false otherwise.
    pub fn is_other<T: core::error::Error + 'static>(&self) -> bool {
        self.as_other().map(|err| err.as_any().is::<T>()).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(thiserror::Error, Debug)]
    #[error("err")]
    struct E;

    impl PoolTransactionError for E {
        fn is_bad_transaction(&self) -> bool {
            false
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    #[test]
    fn other_downcast() {
        let err = InvalidPoolTransactionError::Other(Box::new(E));
        assert!(err.is_other::<E>());

        assert!(err.downcast_other_ref::<E>().is_some());
    }
}
