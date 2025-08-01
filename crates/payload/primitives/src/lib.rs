//! Abstractions for working with execution payloads.
//!
//! This crate provides types and traits for execution and building payloads.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use crate::alloc::string::ToString;
use alloy_primitives::Bytes;
use reth_chainspec::EthereumHardforks;
use reth_primitives_traits::{NodePrimitives, SealedBlock};

mod error;
pub use error::{
    EngineObjectValidationError, InvalidPayloadAttributesError, NewPayloadError,
    PayloadBuilderError, VersionSpecificValidationError,
};

mod traits;
pub use traits::{
    BuildNextEnv, BuiltPayload, PayloadAttributes, PayloadAttributesBuilder,
    PayloadBuilderAttributes,
};

mod payload;
pub use payload::{ExecutionPayload, PayloadOrAttributes};

/// Core trait that defines the associated types for working with execution payloads.
pub trait PayloadTypes: Send + Sync + Unpin + core::fmt::Debug + Clone + 'static {
    /// The format for execution payload data that can be processed and validated.
    ///
    /// This type represents the canonical format for block data that includes
    /// all necessary information for execution and validation.
    type ExecutionData: ExecutionPayload;
    /// The type representing a successfully built payload/block.
    type BuiltPayload: BuiltPayload + Clone + Unpin;

    /// Attributes that specify how a payload should be constructed.
    ///
    /// These attributes typically come from external sources (e.g., consensus layer over RPC such
    /// as the Engine API) and contain parameters like timestamp, fee recipient, and randomness.
    type PayloadAttributes: PayloadAttributes + Unpin;

    /// Extended attributes used internally during payload building.
    ///
    /// This type augments the basic payload attributes with additional information
    /// needed during the building process, such as unique identifiers and parent
    /// block references.
    type PayloadBuilderAttributes: PayloadBuilderAttributes<RpcPayloadAttributes = Self::PayloadAttributes>
        + Clone
        + Unpin;

    /// Converts a sealed block into the execution payload format.
    fn block_to_payload(
        block: SealedBlock<
            <<Self::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block,
        >,
    ) -> Self::ExecutionData;
}

/// Validates the timestamp depending on the version called:
///
/// * If V2, this ensures that the payload timestamp is pre-Cancun.
/// * If V3, this ensures that the payload timestamp is within the Cancun timestamp.
/// * If V4, this ensures that the payload timestamp is within the Prague timestamp.
///
/// Otherwise, this will return [`EngineObjectValidationError::UnsupportedFork`].
pub fn validate_payload_timestamp(
    chain_spec: impl EthereumHardforks,
    version: EngineApiMessageVersion,
    timestamp: u64,
) -> Result<(), EngineObjectValidationError> {
    let is_cancun = chain_spec.is_cancun_active_at_timestamp(timestamp);
    if version.is_v2() && is_cancun {
        // From the Engine API spec:
        //
        // ### Update the methods of previous forks
        //
        // This document defines how Cancun payload should be handled by the [`Shanghai
        // API`](https://github.com/ethereum/execution-apis/blob/ff43500e653abde45aec0f545564abfb648317af/src/engine/shanghai.md).
        //
        // For the following methods:
        //
        // - [`engine_forkchoiceUpdatedV2`](https://github.com/ethereum/execution-apis/blob/ff43500e653abde45aec0f545564abfb648317af/src/engine/shanghai.md#engine_forkchoiceupdatedv2)
        // - [`engine_newPayloadV2`](https://github.com/ethereum/execution-apis/blob/ff43500e653abde45aec0f545564abfb648317af/src/engine/shanghai.md#engine_newpayloadV2)
        // - [`engine_getPayloadV2`](https://github.com/ethereum/execution-apis/blob/ff43500e653abde45aec0f545564abfb648317af/src/engine/shanghai.md#engine_getpayloadv2)
        //
        // a validation **MUST** be added:
        //
        // 1. Client software **MUST** return `-38005: Unsupported fork` error if the `timestamp` of
        //    payload or payloadAttributes is greater or equal to the Cancun activation timestamp.
        return Err(EngineObjectValidationError::UnsupportedFork)
    }

    if version.is_v3() && !is_cancun {
        // From the Engine API spec:
        // <https://github.com/ethereum/execution-apis/blob/ff43500e653abde45aec0f545564abfb648317af/src/engine/cancun.md#specification-2>
        //
        // For `engine_getPayloadV3`:
        //
        // 1. Client software **MUST** return `-38005: Unsupported fork` error if the `timestamp` of
        //    the built payload does not fall within the time frame of the Cancun fork.
        //
        // For `engine_forkchoiceUpdatedV3`:
        //
        // 2. Client software **MUST** return `-38005: Unsupported fork` error if the
        //    `payloadAttributes` is set and the `payloadAttributes.timestamp` does not fall within
        //    the time frame of the Cancun fork.
        //
        // For `engine_newPayloadV3`:
        //
        // 2. Client software **MUST** return `-38005: Unsupported fork` error if the `timestamp` of
        //    the payload does not fall within the time frame of the Cancun fork.
        return Err(EngineObjectValidationError::UnsupportedFork)
    }

    let is_prague = chain_spec.is_prague_active_at_timestamp(timestamp);
    if version.is_v4() && !is_prague {
        // From the Engine API spec:
        // <https://github.com/ethereum/execution-apis/blob/7907424db935b93c2fe6a3c0faab943adebe8557/src/engine/prague.md#specification-1>
        //
        // For `engine_getPayloadV4`:
        //
        // 1. Client software **MUST** return `-38005: Unsupported fork` error if the `timestamp` of
        //    the built payload does not fall within the time frame of the Prague fork.
        //
        // For `engine_forkchoiceUpdatedV4`:
        //
        // 2. Client software **MUST** return `-38005: Unsupported fork` error if the
        //    `payloadAttributes` is set and the `payloadAttributes.timestamp` does not fall within
        //    the time frame of the Prague fork.
        //
        // For `engine_newPayloadV4`:
        //
        // 2. Client software **MUST** return `-38005: Unsupported fork` error if the `timestamp` of
        //    the payload does not fall within the time frame of the Prague fork.
        return Err(EngineObjectValidationError::UnsupportedFork)
    }

    let is_osaka = chain_spec.is_osaka_active_at_timestamp(timestamp);
    if version.is_v5() && !is_osaka {
        // From the Engine API spec:
        // <https://github.com/ethereum/execution-apis/blob/15399c2e2f16a5f800bf3f285640357e2c245ad9/src/engine/osaka.md#specification>
        //
        // For `engine_getPayloadV5`
        //
        // 1. Client software MUST return -38005: Unsupported fork error if the timestamp of the
        //    built payload does not fall within the time frame of the Osaka fork.
        return Err(EngineObjectValidationError::UnsupportedFork)
    }

    Ok(())
}

/// Validates the presence of the `withdrawals` field according to the payload timestamp.
/// After Shanghai, withdrawals field must be [Some].
/// Before Shanghai, withdrawals field must be [None];
pub fn validate_withdrawals_presence<T: EthereumHardforks>(
    chain_spec: &T,
    version: EngineApiMessageVersion,
    message_validation_kind: MessageValidationKind,
    timestamp: u64,
    has_withdrawals: bool,
) -> Result<(), EngineObjectValidationError> {
    let is_shanghai_active = chain_spec.is_shanghai_active_at_timestamp(timestamp);

    match version {
        EngineApiMessageVersion::V1 => {
            if has_withdrawals {
                return Err(message_validation_kind
                    .to_error(VersionSpecificValidationError::WithdrawalsNotSupportedInV1))
            }
        }
        EngineApiMessageVersion::V2 |
        EngineApiMessageVersion::V3 |
        EngineApiMessageVersion::V4 |
        EngineApiMessageVersion::V5 => {
            if is_shanghai_active && !has_withdrawals {
                return Err(message_validation_kind
                    .to_error(VersionSpecificValidationError::NoWithdrawalsPostShanghai))
            }
            if !is_shanghai_active && has_withdrawals {
                return Err(message_validation_kind
                    .to_error(VersionSpecificValidationError::HasWithdrawalsPreShanghai))
            }
        }
    };

    Ok(())
}

/// Validate the presence of the `parentBeaconBlockRoot` field according to the given timestamp.
/// This method is meant to be used with either a `payloadAttributes` field or a full payload, with
/// the `engine_forkchoiceUpdated` and `engine_newPayload` methods respectively.
///
/// After Cancun, the `parentBeaconBlockRoot` field must be [Some].
/// Before Cancun, the `parentBeaconBlockRoot` field must be [None].
///
/// If the engine API message version is V1 or V2, and the timestamp is post-Cancun, then this will
/// return [`EngineObjectValidationError::UnsupportedFork`].
///
/// If the timestamp is before the Cancun fork and the engine API message version is V3, then this
/// will return [`EngineObjectValidationError::UnsupportedFork`].
///
/// If the engine API message version is V3, but the `parentBeaconBlockRoot` is [None], then
/// this will return [`VersionSpecificValidationError::NoParentBeaconBlockRootPostCancun`].
///
/// This implements the following Engine API spec rules:
///
/// 1. Client software **MUST** check that provided set of parameters and their fields strictly
///    matches the expected one and return `-32602: Invalid params` error if this check fails. Any
///    field having `null` value **MUST** be considered as not provided.
///
/// For `engine_forkchoiceUpdatedV3`:
///
/// 1. Client software **MUST** check that provided set of parameters and their fields strictly
///    matches the expected one and return `-32602: Invalid params` error if this check fails. Any
///    field having `null` value **MUST** be considered as not provided.
///
/// 2. Extend point (7) of the `engine_forkchoiceUpdatedV1` specification by defining the following
///    sequence of checks that **MUST** be run over `payloadAttributes`:
///     1. `payloadAttributes` matches the `PayloadAttributesV3` structure, return `-38003: Invalid
///        payload attributes` on failure.
///     2. `payloadAttributes.timestamp` falls within the time frame of the Cancun fork, return
///        `-38005: Unsupported fork` on failure.
///     3. `payloadAttributes.timestamp` is greater than `timestamp` of a block referenced by
///        `forkchoiceState.headBlockHash`, return `-38003: Invalid payload attributes` on failure.
///     4. If any of the above checks fails, the `forkchoiceState` update **MUST NOT** be rolled
///        back.
///
/// For `engine_newPayloadV3`:
///
/// 2. Client software **MUST** return `-38005: Unsupported fork` error if the `timestamp` of the
///    payload does not fall within the time frame of the Cancun fork.
///
/// For `engine_newPayloadV4`:
///
/// 2. Client software **MUST** return `-38005: Unsupported fork` error if the `timestamp` of the
///    payload does not fall within the time frame of the Prague fork.
///
/// Returning the right error code (ie, if the client should return `-38003: Invalid payload
/// attributes` is handled by the `message_validation_kind` parameter. If the parameter is
/// `MessageValidationKind::Payload`, then the error code will be `-32602: Invalid params`. If the
/// parameter is `MessageValidationKind::PayloadAttributes`, then the error code will be `-38003:
/// Invalid payload attributes`.
pub fn validate_parent_beacon_block_root_presence<T: EthereumHardforks>(
    chain_spec: &T,
    version: EngineApiMessageVersion,
    validation_kind: MessageValidationKind,
    timestamp: u64,
    has_parent_beacon_block_root: bool,
) -> Result<(), EngineObjectValidationError> {
    // 1. Client software **MUST** check that provided set of parameters and their fields strictly
    //    matches the expected one and return `-32602: Invalid params` error if this check fails.
    //    Any field having `null` value **MUST** be considered as not provided.
    //
    // For `engine_forkchoiceUpdatedV3`:
    //
    // 2. Extend point (7) of the `engine_forkchoiceUpdatedV1` specification by defining the
    //    following sequence of checks that **MUST** be run over `payloadAttributes`:
    //     1. `payloadAttributes` matches the `PayloadAttributesV3` structure, return `-38003:
    //        Invalid payload attributes` on failure.
    //     2. `payloadAttributes.timestamp` falls within the time frame of the Cancun fork, return
    //        `-38005: Unsupported fork` on failure.
    //     3. `payloadAttributes.timestamp` is greater than `timestamp` of a block referenced by
    //        `forkchoiceState.headBlockHash`, return `-38003: Invalid payload attributes` on
    //        failure.
    //     4. If any of the above checks fails, the `forkchoiceState` update **MUST NOT** be rolled
    //        back.
    match version {
        EngineApiMessageVersion::V1 | EngineApiMessageVersion::V2 => {
            if has_parent_beacon_block_root {
                return Err(validation_kind.to_error(
                    VersionSpecificValidationError::ParentBeaconBlockRootNotSupportedBeforeV3,
                ))
            }
        }
        EngineApiMessageVersion::V3 | EngineApiMessageVersion::V4 | EngineApiMessageVersion::V5 => {
            if !has_parent_beacon_block_root {
                return Err(validation_kind
                    .to_error(VersionSpecificValidationError::NoParentBeaconBlockRootPostCancun))
            }
        }
    };

    // For `engine_forkchoiceUpdatedV3`:
    //
    // 2. Client software **MUST** return `-38005: Unsupported fork` error if the
    //    `payloadAttributes` is set and the `payloadAttributes.timestamp` does not fall within the
    //    time frame of the Cancun fork.
    //
    // For `engine_newPayloadV3`:
    //
    // 2. Client software **MUST** return `-38005: Unsupported fork` error if the `timestamp` of the
    //    payload does not fall within the time frame of the Cancun fork.
    validate_payload_timestamp(chain_spec, version, timestamp)?;

    Ok(())
}

/// A type that represents whether or not we are validating a payload or payload attributes.
///
/// This is used to ensure that the correct error code is returned when validating the payload or
/// payload attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageValidationKind {
    /// We are validating fields of a payload attributes.
    PayloadAttributes,
    /// We are validating fields of a payload.
    Payload,
}

impl MessageValidationKind {
    /// Returns an `EngineObjectValidationError` based on the given
    /// `VersionSpecificValidationError` and the current validation kind.
    pub const fn to_error(
        self,
        error: VersionSpecificValidationError,
    ) -> EngineObjectValidationError {
        match self {
            Self::Payload => EngineObjectValidationError::Payload(error),
            Self::PayloadAttributes => EngineObjectValidationError::PayloadAttributes(error),
        }
    }
}

/// Validates the presence or exclusion of fork-specific fields based on the ethereum execution
/// payload, or payload attributes, and the message version.
///
/// The object being validated is provided by the [`PayloadOrAttributes`] argument, which can be
/// either an execution payload, or payload attributes.
///
/// The version is provided by the [`EngineApiMessageVersion`] argument.
pub fn validate_version_specific_fields<Payload, Type, T>(
    chain_spec: &T,
    version: EngineApiMessageVersion,
    payload_or_attrs: PayloadOrAttributes<'_, Payload, Type>,
) -> Result<(), EngineObjectValidationError>
where
    Payload: ExecutionPayload,
    Type: PayloadAttributes,
    T: EthereumHardforks,
{
    validate_withdrawals_presence(
        chain_spec,
        version,
        payload_or_attrs.message_validation_kind(),
        payload_or_attrs.timestamp(),
        payload_or_attrs.withdrawals().is_some(),
    )?;
    validate_parent_beacon_block_root_presence(
        chain_spec,
        version,
        payload_or_attrs.message_validation_kind(),
        payload_or_attrs.timestamp(),
        payload_or_attrs.parent_beacon_block_root().is_some(),
    )
}

/// The version of Engine API message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum EngineApiMessageVersion {
    /// Version 1
    V1 = 1,
    /// Version 2
    ///
    /// Added in the Shanghai hardfork.
    V2 = 2,
    /// Version 3
    ///
    /// Added in the Cancun hardfork.
    V3 = 3,
    /// Version 4
    ///
    /// Added in the Prague hardfork.
    #[default]
    V4 = 4,
    /// Version 5
    ///
    /// Added in the Osaka hardfork.
    V5 = 5,
}

impl EngineApiMessageVersion {
    /// Returns true if the version is V1.
    pub const fn is_v1(&self) -> bool {
        matches!(self, Self::V1)
    }

    /// Returns true if the version is V2.
    pub const fn is_v2(&self) -> bool {
        matches!(self, Self::V2)
    }

    /// Returns true if the version is V3.
    pub const fn is_v3(&self) -> bool {
        matches!(self, Self::V3)
    }

    /// Returns true if the version is V4.
    pub const fn is_v4(&self) -> bool {
        matches!(self, Self::V4)
    }

    /// Returns true if the version is V5.
    pub const fn is_v5(&self) -> bool {
        matches!(self, Self::V5)
    }

    /// Returns the method name for the given version.
    pub const fn method_name(&self) -> &'static str {
        match self {
            Self::V1 => "engine_newPayloadV1",
            Self::V2 => "engine_newPayloadV2",
            Self::V3 => "engine_newPayloadV3",
            Self::V4 => "engine_newPayloadV4",
            Self::V5 => "engine_newPayloadV5",
        }
    }
}

/// Determines how we should choose the payload to return.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PayloadKind {
    /// Returns the next best available payload (the earliest available payload).
    /// This does not wait for a real for pending job to finish if there's no best payload yet and
    /// is allowed to race various payload jobs (empty, pending best) against each other and
    /// returns whichever job finishes faster.
    ///
    /// This should be used when it's more important to return a valid payload as fast as possible.
    /// For example, the engine API timeout for `engine_getPayload` is 1s and clients should rather
    /// return an empty payload than indefinitely waiting for the pending payload job to finish and
    /// risk missing the deadline.
    #[default]
    Earliest,
    /// Only returns once we have at least one built payload.
    ///
    /// Compared to [`PayloadKind::Earliest`] this does not race an empty payload job against the
    /// already in progress one, and returns the best available built payload or awaits the job in
    /// progress.
    WaitForPending,
}

/// Validates that execution requests are valid according to Engine API specification.
///
/// `executionRequests`: `Array of DATA` - List of execution layer triggered requests. Each list
/// element is a `requests` byte array as defined by [EIP-7685](https://eips.ethereum.org/EIPS/eip-7685).
/// The first byte of each element is the `request_type` and the remaining bytes are the
/// `request_data`. Elements of the list **MUST** be ordered by `request_type` in ascending order.
/// Elements with empty `request_data` **MUST** be excluded from the list. If any element is out of
/// order, has a length of 1-byte or shorter, or more than one element has the same type byte,
/// client software **MUST** return `-32602: Invalid params` error.
pub fn validate_execution_requests(requests: &[Bytes]) -> Result<(), EngineObjectValidationError> {
    let mut last_request_type = None;
    for request in requests {
        if request.len() <= 1 {
            return Err(EngineObjectValidationError::InvalidParams(
                "EmptyExecutionRequest".to_string().into(),
            ))
        }

        let request_type = request[0];
        if Some(request_type) < last_request_type {
            return Err(EngineObjectValidationError::InvalidParams(
                "OutOfOrderExecutionRequest".to_string().into(),
            ))
        }

        if Some(request_type) == last_request_type {
            return Err(EngineObjectValidationError::InvalidParams(
                "DuplicatedExecutionRequestType".to_string().into(),
            ))
        }

        last_request_type = Some(request_type);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;

    #[test]
    fn version_ord() {
        assert!(EngineApiMessageVersion::V4 > EngineApiMessageVersion::V3);
    }

    #[test]
    fn execution_requests_validation() {
        assert_matches!(validate_execution_requests(&[]), Ok(()));

        let valid_requests = [
            Bytes::from_iter([1, 2]),
            Bytes::from_iter([2, 3]),
            Bytes::from_iter([3, 4]),
            Bytes::from_iter([4, 5]),
        ];
        assert_matches!(validate_execution_requests(&valid_requests), Ok(()));

        let requests_with_empty = [
            Bytes::from_iter([1, 2]),
            Bytes::from_iter([2, 3]),
            Bytes::new(),
            Bytes::from_iter([3, 4]),
        ];
        assert_matches!(
            validate_execution_requests(&requests_with_empty),
            Err(EngineObjectValidationError::InvalidParams(_))
        );

        let mut requests_valid_reversed = valid_requests;
        requests_valid_reversed.reverse();
        assert_matches!(
            validate_execution_requests(&requests_with_empty),
            Err(EngineObjectValidationError::InvalidParams(_))
        );

        let requests_out_of_order = [
            Bytes::from_iter([1, 2]),
            Bytes::from_iter([2, 3]),
            Bytes::from_iter([4, 5]),
            Bytes::from_iter([3, 4]),
        ];
        assert_matches!(
            validate_execution_requests(&requests_out_of_order),
            Err(EngineObjectValidationError::InvalidParams(_))
        );

        let duplicate_request_types = [
            Bytes::from_iter([1, 2]),
            Bytes::from_iter([3, 3]),
            Bytes::from_iter([4, 5]),
            Bytes::from_iter([4, 4]),
        ];
        assert_matches!(
            validate_execution_requests(&duplicate_request_types),
            Err(EngineObjectValidationError::InvalidParams(_))
        );
    }
}
