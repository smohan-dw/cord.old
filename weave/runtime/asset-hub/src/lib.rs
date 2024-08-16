// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! # Asset Hub Polkadot Runtime
//!
//! Asset Hub Polkadot is a parachain that provides an interface to create, manage, and use assets.
//! Assets may be fungible or non-fungible.
//!
//! ## Renaming
//!
//! This chain was originally known as "Statemint". You may see references to Statemint, Statemine,
//! and Westmint throughout the codebase. These are synonymous with "Asset Hub Polkadot, Kusama, and
//! Westend", respectively.
//!
//! ## Assets
//!
//! - Fungibles: Configuration of `pallet-assets`.
//! - Non-Fungibles (NFTs): Configuration of `pallet-uniques`.
//!
//! ## Other Functionality
//!
//! ### Native Balances
//!
//! Asset Hub Polkadot uses its parent DOT token as its native asset.
//!
//! ### Governance
//!
//! As a system parachain, Asset Hub defers its governance (namely, its `Root` origin), to its
//! Relay Chain parent, Polkadot.
//!
//! ### Collator Selection
//!
//! Asset Hub uses `pallet-collator-selection`, a simple first-come-first-served registration
//! system where collators can reserve a small bond to join the block producer set. There is no
//! slashing.
//!
//! ### XCM
//!
//! Because Asset Hub is fully under the control of the Relay Chain, it is meant to be a
//! `TrustedTeleporter`. It can also serve as a reserve location to other parachains for DOT as well
//! as other local assets.

#![cfg_attr(not(feature = "std"), no_std)]
#![recursion_limit = "256"]

// Make the WASM binary available.
#[cfg(feature = "std")]
include!(concat!(env!("OUT_DIR"), "/wasm_binary.rs"));

// Genesis preset configurations.
pub mod genesis_config_presets;
mod impls;
mod weights;
pub mod xcm_config;

use assets_common::{
	foreign_creators::ForeignCreators,
	local_and_foreign_assets::{LocalFromLeft, TargetFromLeft},
	matching::FromSiblingParachain,
	AssetIdForTrustBackedAssetsConvert,
};
use cumulus_pallet_parachain_system::RelayNumberMonotonicallyIncreases;
use cumulus_primitives_core::{AggregateMessageOrigin, ParaId};
use sp_api::impl_runtime_apis;
use sp_core::{crypto::KeyTypeId, ConstU128, OpaqueMetadata};
use sp_runtime::{
	create_runtime_str, generic, impl_opaque_keys,
	traits::{AccountIdLookup, BlakeTwo256, Block as BlockT, Verify},
	transaction_validity::{TransactionSource, TransactionValidity},
	ApplyExtrinsicResult, Perbill, Permill,
};
use xcm_config::TrustBackedAssetsPalletLocationV3;
use xcm_runtime_apis::{
	dry_run::{CallDryRunEffects, Error as XcmDryRunApiError, XcmDryRunEffects},
	fees::Error as XcmPaymentApiError,
};

use sp_std::prelude::*;
#[cfg(feature = "std")]
use sp_version::NativeVersion;
use sp_version::RuntimeVersion;

use codec::{Decode, Encode, MaxEncodedLen};
use frame_support::{
	construct_runtime,
	dispatch::DispatchClass,
	genesis_builder_helper::{build_state, get_preset},
	parameter_types,
	traits::{
		fungible, fungibles, tokens::imbalance::ResolveAssetTo, AsEnsureOriginWithArg, ConstBool,
		ConstU32, ConstU64, ConstU8, EitherOfDiverse, InstanceFilter, NeverEnsureOrigin,
		TransformOrigin,
	},
	weights::{ConstantMultiplier, Weight, WeightToFee as _},
	PalletId,
};
use frame_system::{
	limits::{BlockLength, BlockWeights},
	EnsureRoot, EnsureSigned,
};
use pallet_nfts::PalletFeatures;
use parachains_common::{
	message_queue::*, AccountId, AssetIdForTrustBackedAssets, AuraId, Balance, BlockNumber, Hash,
	Header, Nonce, Signature,
};

pub use cord_weave_system_parachains_constants::SLOT_DURATION;
use cord_weave_system_parachains_constants::{
	loom::{consensus::*, currency::*, fee::WeightToFee},
	AVERAGE_ON_INITIALIZE_RATIO, DAYS, HOURS, MAXIMUM_BLOCK_WEIGHT, NORMAL_DISPATCH_RATIO,
};
use sp_runtime::RuntimeDebug;
use xcm::{
	latest::prelude::{AssetId, BodyId},
	VersionedAssetId, VersionedAssets, VersionedLocation, VersionedXcm,
};
use xcm_config::{
	FellowshipLocation, ForeignAssetsConvertedConcreteId, ForeignCreatorsSovereignAccountOf,
	GovernanceLocation, PoolAssetsConvertedConcreteId, TrustBackedAssetsConvertedConcreteId,
	UntLocation, UntLocationV3, XcmOriginToTransactDispatchOrigin,
};

#[cfg(any(feature = "std", test))]
pub use sp_runtime::BuildStorage;

// Polkadot imports
use pallet_xcm::{EnsureXcm, IsVoiceOfBody};
use polkadot_runtime_common::{BlockHashCount, SlowAdjustingFeeUpdate};

use weights::{BlockExecutionWeight, ExtrinsicBaseWeight, RocksDbWeight};

impl_opaque_keys! {
	pub struct SessionKeys {
		pub aura: Aura,
	}
}

#[sp_version::runtime_version]
pub const VERSION: RuntimeVersion = RuntimeVersion {
	spec_name: create_runtime_str!("asset-hub-loom"),
	impl_name: create_runtime_str!("dw-asset-hub-loom"),
	authoring_version: 1,
	spec_version: 1_000_000,
	impl_version: 0,
	apis: RUNTIME_API_VERSIONS,
	transaction_version: 15,
	state_version: 0,
};

/// The version information used to identify this runtime when compiled natively.
#[cfg(feature = "std")]
pub fn native_version() -> NativeVersion {
	NativeVersion { runtime_version: VERSION, can_author_with: Default::default() }
}

parameter_types! {
	pub const Version: RuntimeVersion = VERSION;
	pub RuntimeBlockLength: BlockLength =
		BlockLength::max_with_normal_ratio(5 * 1024 * 1024, NORMAL_DISPATCH_RATIO);
	pub RuntimeBlockWeights: BlockWeights = BlockWeights::builder()
		.base_block(BlockExecutionWeight::get())
		.for_class(DispatchClass::all(), |weights| {
			weights.base_extrinsic = ExtrinsicBaseWeight::get();
		})
		.for_class(DispatchClass::Normal, |weights| {
			weights.max_total = Some(NORMAL_DISPATCH_RATIO * MAXIMUM_BLOCK_WEIGHT);
		})
		.for_class(DispatchClass::Operational, |weights| {
			weights.max_total = Some(MAXIMUM_BLOCK_WEIGHT);
			// Operational transactions have some extra reserved space, so that they
			// are included even if block reached `MAXIMUM_BLOCK_WEIGHT`.
			weights.reserved = Some(
				MAXIMUM_BLOCK_WEIGHT - NORMAL_DISPATCH_RATIO * MAXIMUM_BLOCK_WEIGHT
			);
		})
		.avg_block_initialization(AVERAGE_ON_INITIALIZE_RATIO)
		.build_or_panic();
	pub const SS58Prefix: u8 = 0;
}

// Configure FRAME pallets to include in runtime.
impl frame_system::Config for Runtime {
	type BaseCallFilter = frame_support::traits::Everything;
	type BlockWeights = RuntimeBlockWeights;
	type BlockLength = RuntimeBlockLength;
	type AccountId = AccountId;
	type RuntimeCall = RuntimeCall;
	type Lookup = AccountIdLookup<AccountId, ()>;
	type Nonce = Nonce;
	type Hash = Hash;
	type Hashing = BlakeTwo256;
	type Block = Block;
	type RuntimeEvent = RuntimeEvent;
	type RuntimeTask = RuntimeTask;
	type RuntimeOrigin = RuntimeOrigin;
	type BlockHashCount = BlockHashCount;
	type DbWeight = RocksDbWeight;
	type Version = Version;
	type PalletInfo = PalletInfo;
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type AccountData = pallet_balances::AccountData<Balance>;
	type SystemWeightInfo = weights::frame_system::WeightInfo<Runtime>;
	type SS58Prefix = SS58Prefix;
	type OnSetCode = cumulus_pallet_parachain_system::ParachainSetCode<Self>;
	type MaxConsumers = frame_support::traits::ConstU32<64>;
	type SingleBlockMigrations = ();
	type MultiBlockMigrator = ();
	type PreInherents = ();
	type PostInherents = ();
	type PostTransactions = ();
}

impl pallet_timestamp::Config for Runtime {
	/// A timestamp: milliseconds since the unix epoch.
	type Moment = u64;
	type OnTimestampSet = Aura;
	type MinimumPeriod = ConstU64<{ SLOT_DURATION / 2 }>;
	type WeightInfo = weights::pallet_timestamp::WeightInfo<Runtime>;
}

impl pallet_authorship::Config for Runtime {
	type FindAuthor = pallet_session::FindAccountFromAuthorIndex<Self, Aura>;
	type EventHandler = (CollatorSelection,);
}

parameter_types! {
	// This comes from cord_weave_system_parachains_constants::polkadot::currency and is the ED for all system
	// parachains. For Asset Hub in particular, we set it to 1/10th of the amount.
	pub const ExistentialDeposit: Balance = SYSTEM_PARA_EXISTENTIAL_DEPOSIT / 10;
}

impl pallet_balances::Config for Runtime {
	type MaxLocks = ConstU32<50>;
	/// The type for recording an account's balance.
	type Balance = Balance;
	/// The ubiquitous event type.
	type RuntimeEvent = RuntimeEvent;
	type DustRemoval = ();
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
	type WeightInfo = weights::pallet_balances::WeightInfo<Runtime>;
	type MaxReserves = ConstU32<50>;
	type ReserveIdentifier = [u8; 8];
	type RuntimeHoldReason = RuntimeHoldReason;
	type RuntimeFreezeReason = RuntimeFreezeReason;
	type FreezeIdentifier = ();
	type MaxFreezes = ConstU32<0>;
}

parameter_types! {
	/// Relay Chain `TransactionByteFee` / 10
	pub const TransactionByteFee: Balance = cord_weave_system_parachains_constants::loom::fee::TRANSACTION_BYTE_FEE;
	pub StakingPot: AccountId = CollatorSelection::account_id();
}

impl pallet_transaction_payment::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type OnChargeTransaction = impls::tx_payment::FungiblesAdapter<
		NativeAndAssets,
		UntLocationV3,
		ResolveAssetTo<StakingPot, NativeAndAssets>,
	>;
	type WeightToFee = WeightToFee;
	type LengthToFee = ConstantMultiplier<Balance, TransactionByteFee>;
	type FeeMultiplierUpdate = SlowAdjustingFeeUpdate<Self>;
	type OperationalFeeMultiplier = ConstU8<5>;
}

parameter_types! {
	pub const AssetDeposit: Balance = system_para_deposit(1, 190);
	pub const AssetAccountDeposit: Balance = system_para_deposit(1, 16);
	pub const AssetsStringLimit: u32 = 50;
	/// Key = 32 bytes, Value = 36 bytes (32+1+1+1+1)
	// https://github.com/paritytech/substrate/blob/069917b/frame/assets/src/lib.rs#L257L271
	pub const MetadataDepositBase: Balance = system_para_deposit(1, 68);
	pub const MetadataDepositPerByte: Balance = system_para_deposit(0, 1);
}

/// We allow root to execute privileged asset operations.
pub type AssetsForceOrigin = EnsureRoot<AccountId>;

// Called "Trust Backed" assets because these are generally registered by some account, and users of
// the asset assume it has some claimed backing. The pallet is called `Assets` in
// `construct_runtime` to avoid breaking changes on storage reads.
pub type TrustBackedAssetsInstance = pallet_assets::Instance1;
type TrustBackedAssetsCall = pallet_assets::Call<Runtime, TrustBackedAssetsInstance>;
impl pallet_assets::Config<TrustBackedAssetsInstance> for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Balance = Balance;
	type AssetId = AssetIdForTrustBackedAssets;
	type AssetIdParameter = codec::Compact<AssetIdForTrustBackedAssets>;
	type Currency = Balances;
	type CreateOrigin = AsEnsureOriginWithArg<EnsureSigned<AccountId>>;
	type ForceOrigin = AssetsForceOrigin;
	type AssetDeposit = AssetDeposit;
	type MetadataDepositBase = MetadataDepositBase;
	type MetadataDepositPerByte = MetadataDepositPerByte;
	type ApprovalDeposit = ExistentialDeposit;
	type StringLimit = AssetsStringLimit;
	type Freezer = ();
	type Extra = ();
	type WeightInfo = weights::pallet_assets_local::WeightInfo<Runtime>;
	type CallbackHandle = pallet_assets::AutoIncAssetId<Runtime, TrustBackedAssetsInstance>;
	type AssetAccountDeposit = AssetAccountDeposit;
	type RemoveItemsLimit = frame_support::traits::ConstU32<1000>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = ();
}

parameter_types! {
	// we just reuse the same deposits
	pub const ForeignAssetsAssetDeposit: Balance = AssetDeposit::get();
	pub const ForeignAssetsAssetAccountDeposit: Balance = AssetAccountDeposit::get();
	pub const ForeignAssetsAssetsStringLimit: u32 = AssetsStringLimit::get();
	pub const ForeignAssetsMetadataDepositBase: Balance = MetadataDepositBase::get();
	pub const ForeignAssetsMetadataDepositPerByte: Balance = MetadataDepositPerByte::get();
}

/// Assets managed by some foreign location. Note: we do not declare a `ForeignAssetsCall` type, as
/// this type is used in proxy definitions. We assume that a foreign location would not want to set
/// an individual, local account as a proxy for the issuance of their assets. This issuance should
/// be managed by the foreign location's governance.
pub type ForeignAssetsInstance = pallet_assets::Instance2;
impl pallet_assets::Config<ForeignAssetsInstance> for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Balance = Balance;
	type AssetId = xcm::v3::Location;
	type AssetIdParameter = xcm::v3::Location;
	type Currency = Balances;
	type CreateOrigin = ForeignCreators<
		(FromSiblingParachain<parachain_info::Pallet<Runtime>, xcm::v3::Location>,),
		ForeignCreatorsSovereignAccountOf,
		AccountId,
		xcm::v3::Location,
	>;
	type ForceOrigin = AssetsForceOrigin;
	type AssetDeposit = ForeignAssetsAssetDeposit;
	type MetadataDepositBase = ForeignAssetsMetadataDepositBase;
	type MetadataDepositPerByte = ForeignAssetsMetadataDepositPerByte;
	type ApprovalDeposit = ExistentialDeposit;
	type StringLimit = ForeignAssetsAssetsStringLimit;
	type Freezer = ();
	type Extra = ();
	type WeightInfo = weights::pallet_assets_foreign::WeightInfo<Runtime>;
	type CallbackHandle = ();
	type AssetAccountDeposit = ForeignAssetsAssetAccountDeposit;
	type RemoveItemsLimit = frame_support::traits::ConstU32<1000>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = xcm_config::XcmBenchmarkHelper;
}

parameter_types! {
	// One storage item; key size is 32; value is size 4+4+16+32 bytes = 56 bytes.
	pub const DepositBase: Balance = system_para_deposit(1, 88);
	// Additional storage item size of 32 bytes.
	pub const DepositFactor: Balance = system_para_deposit(0, 32);
	pub const MaxSignatories: u32 = 100;
}

impl pallet_multisig::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeCall = RuntimeCall;
	type Currency = Balances;
	type DepositBase = DepositBase;
	type DepositFactor = DepositFactor;
	type MaxSignatories = MaxSignatories;
	type WeightInfo = weights::pallet_multisig::WeightInfo<Runtime>;
}

impl pallet_utility::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeCall = RuntimeCall;
	type PalletsOrigin = OriginCaller;
	type WeightInfo = weights::pallet_utility::WeightInfo<Runtime>;
}

parameter_types! {
	// One storage item; key size 32, value size 8; .
	pub const ProxyDepositBase: Balance = system_para_deposit(1, 40);
	// Additional storage item size of 33 bytes.
	pub const ProxyDepositFactor: Balance = system_para_deposit(0, 33);
	pub const MaxProxies: u16 = 32;
	// One storage item; key size 32, value size 16
	pub const AnnouncementDepositBase: Balance = system_para_deposit(1, 48);
	pub const AnnouncementDepositFactor: Balance = system_para_deposit(0, 66);
	pub const MaxPending: u16 = 32;
}

/// The type used to represent the kinds of proxying allowed.
#[derive(
	Copy,
	Clone,
	Eq,
	PartialEq,
	Ord,
	PartialOrd,
	Encode,
	Decode,
	RuntimeDebug,
	MaxEncodedLen,
	scale_info::TypeInfo,
)]
pub enum ProxyType {
	/// Fully permissioned proxy. Can execute any call on behalf of _proxied_.
	Any,
	/// Can execute any call that does not transfer funds or assets.
	NonTransfer,
	/// Proxy with the ability to reject time-delay proxy announcements.
	CancelProxy,
	/// Assets proxy. Can execute any call from `assets`, **including asset transfers**.
	Assets,
	/// Owner proxy. Can execute calls related to asset ownership.
	AssetOwner,
	/// Asset manager. Can execute calls related to asset management.
	AssetManager,
	/// Collator selection proxy. Can execute calls related to collator selection mechanism.
	Collator,
}
impl Default for ProxyType {
	fn default() -> Self {
		Self::Any
	}
}

impl InstanceFilter<RuntimeCall> for ProxyType {
	fn filter(&self, c: &RuntimeCall) -> bool {
		match self {
			ProxyType::Any => true,
			ProxyType::NonTransfer => !matches!(
				c,
				RuntimeCall::Balances { .. }
					| RuntimeCall::Assets { .. }
					| RuntimeCall::Nfts { .. }
					| RuntimeCall::Uniques { .. }
			),
			ProxyType::CancelProxy => matches!(
				c,
				RuntimeCall::Proxy(pallet_proxy::Call::reject_announcement { .. })
					| RuntimeCall::Utility { .. }
					| RuntimeCall::Multisig { .. }
			),
			ProxyType::Assets => {
				matches!(
					c,
					RuntimeCall::Assets { .. }
						| RuntimeCall::Utility { .. }
						| RuntimeCall::Multisig { .. }
						| RuntimeCall::Nfts { .. }
						| RuntimeCall::Uniques { .. }
				)
			},
			ProxyType::AssetOwner => matches!(
				c,
				RuntimeCall::Assets(TrustBackedAssetsCall::create { .. })
					| RuntimeCall::Assets(TrustBackedAssetsCall::start_destroy { .. })
					| RuntimeCall::Assets(TrustBackedAssetsCall::destroy_accounts { .. })
					| RuntimeCall::Assets(TrustBackedAssetsCall::destroy_approvals { .. })
					| RuntimeCall::Assets(TrustBackedAssetsCall::finish_destroy { .. })
					| RuntimeCall::Assets(TrustBackedAssetsCall::transfer_ownership { .. })
					| RuntimeCall::Assets(TrustBackedAssetsCall::set_team { .. })
					| RuntimeCall::Assets(TrustBackedAssetsCall::set_metadata { .. })
					| RuntimeCall::Assets(TrustBackedAssetsCall::clear_metadata { .. })
					| RuntimeCall::Assets(TrustBackedAssetsCall::set_min_balance { .. })
					| RuntimeCall::Nfts(pallet_nfts::Call::create { .. })
					| RuntimeCall::Nfts(pallet_nfts::Call::destroy { .. })
					| RuntimeCall::Nfts(pallet_nfts::Call::redeposit { .. })
					| RuntimeCall::Nfts(pallet_nfts::Call::transfer_ownership { .. })
					| RuntimeCall::Nfts(pallet_nfts::Call::set_team { .. })
					| RuntimeCall::Nfts(pallet_nfts::Call::set_collection_max_supply { .. })
					| RuntimeCall::Nfts(pallet_nfts::Call::lock_collection { .. })
					| RuntimeCall::Uniques(pallet_uniques::Call::create { .. })
					| RuntimeCall::Uniques(pallet_uniques::Call::destroy { .. })
					| RuntimeCall::Uniques(pallet_uniques::Call::transfer_ownership { .. })
					| RuntimeCall::Uniques(pallet_uniques::Call::set_team { .. })
					| RuntimeCall::Uniques(pallet_uniques::Call::set_metadata { .. })
					| RuntimeCall::Uniques(pallet_uniques::Call::set_attribute { .. })
					| RuntimeCall::Uniques(pallet_uniques::Call::set_collection_metadata { .. })
					| RuntimeCall::Uniques(pallet_uniques::Call::clear_metadata { .. })
					| RuntimeCall::Uniques(pallet_uniques::Call::clear_attribute { .. })
					| RuntimeCall::Uniques(pallet_uniques::Call::clear_collection_metadata { .. })
					| RuntimeCall::Uniques(pallet_uniques::Call::set_collection_max_supply { .. })
					| RuntimeCall::Utility { .. }
					| RuntimeCall::Multisig { .. }
			),
			ProxyType::AssetManager => matches!(
				c,
				RuntimeCall::Assets(TrustBackedAssetsCall::mint { .. })
					| RuntimeCall::Assets(TrustBackedAssetsCall::burn { .. })
					| RuntimeCall::Assets(TrustBackedAssetsCall::freeze { .. })
					| RuntimeCall::Assets(TrustBackedAssetsCall::block { .. })
					| RuntimeCall::Assets(TrustBackedAssetsCall::thaw { .. })
					| RuntimeCall::Assets(TrustBackedAssetsCall::freeze_asset { .. })
					| RuntimeCall::Assets(TrustBackedAssetsCall::thaw_asset { .. })
					| RuntimeCall::Assets(TrustBackedAssetsCall::touch_other { .. })
					| RuntimeCall::Assets(TrustBackedAssetsCall::refund_other { .. })
					| RuntimeCall::Nfts(pallet_nfts::Call::force_mint { .. })
					| RuntimeCall::Nfts(pallet_nfts::Call::update_mint_settings { .. })
					| RuntimeCall::Nfts(pallet_nfts::Call::mint_pre_signed { .. })
					| RuntimeCall::Nfts(pallet_nfts::Call::set_attributes_pre_signed { .. })
					| RuntimeCall::Nfts(pallet_nfts::Call::lock_item_transfer { .. })
					| RuntimeCall::Nfts(pallet_nfts::Call::unlock_item_transfer { .. })
					| RuntimeCall::Nfts(pallet_nfts::Call::lock_item_properties { .. })
					| RuntimeCall::Nfts(pallet_nfts::Call::set_metadata { .. })
					| RuntimeCall::Nfts(pallet_nfts::Call::clear_metadata { .. })
					| RuntimeCall::Nfts(pallet_nfts::Call::set_collection_metadata { .. })
					| RuntimeCall::Nfts(pallet_nfts::Call::clear_collection_metadata { .. })
					| RuntimeCall::Uniques(pallet_uniques::Call::mint { .. })
					| RuntimeCall::Uniques(pallet_uniques::Call::burn { .. })
					| RuntimeCall::Uniques(pallet_uniques::Call::freeze { .. })
					| RuntimeCall::Uniques(pallet_uniques::Call::thaw { .. })
					| RuntimeCall::Uniques(pallet_uniques::Call::freeze_collection { .. })
					| RuntimeCall::Uniques(pallet_uniques::Call::thaw_collection { .. })
					| RuntimeCall::Utility { .. }
					| RuntimeCall::Multisig { .. }
			),
			ProxyType::Collator => matches!(
				c,
				RuntimeCall::CollatorSelection { .. }
					| RuntimeCall::Utility { .. }
					| RuntimeCall::Multisig { .. }
			),
		}
	}

	fn is_superset(&self, o: &Self) -> bool {
		match (self, o) {
			(x, y) if x == y => true,
			(ProxyType::Any, _) => true,
			(_, ProxyType::Any) => false,
			(ProxyType::Assets, ProxyType::AssetOwner) => true,
			(ProxyType::Assets, ProxyType::AssetManager) => true,
			(ProxyType::NonTransfer, ProxyType::Collator) => true,
			_ => false,
		}
	}
}

impl pallet_proxy::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeCall = RuntimeCall;
	type Currency = Balances;
	type ProxyType = ProxyType;
	type ProxyDepositBase = ProxyDepositBase;
	type ProxyDepositFactor = ProxyDepositFactor;
	type MaxProxies = MaxProxies;
	type WeightInfo = weights::pallet_proxy::WeightInfo<Runtime>;
	type MaxPending = MaxPending;
	type CallHasher = BlakeTwo256;
	type AnnouncementDepositBase = AnnouncementDepositBase;
	type AnnouncementDepositFactor = AnnouncementDepositFactor;
}

parameter_types! {
	pub const ReservedXcmpWeight: Weight = MAXIMUM_BLOCK_WEIGHT.saturating_div(4);
	pub const ReservedDmpWeight: Weight = MAXIMUM_BLOCK_WEIGHT.saturating_div(4);
	pub const RelayOrigin: AggregateMessageOrigin = AggregateMessageOrigin::Parent;
}

impl cumulus_pallet_parachain_system::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type OnSystemEvent = ();
	type SelfParaId = parachain_info::Pallet<Runtime>;
	type DmpQueue = frame_support::traits::EnqueueWithOrigin<MessageQueue, RelayOrigin>;
	type ReservedDmpWeight = ReservedDmpWeight;
	type OutboundXcmpMessageSource = XcmpQueue;
	type XcmpMessageHandler = XcmpQueue;
	type ReservedXcmpWeight = ReservedXcmpWeight;
	type CheckAssociatedRelayNumber = RelayNumberMonotonicallyIncreases;
	type ConsensusHook = ConsensusHook;
	type WeightInfo = weights::cumulus_pallet_parachain_system::WeightInfo<Runtime>;
}

type ConsensusHook = cumulus_pallet_aura_ext::FixedVelocityConsensusHook<
	Runtime,
	RELAY_CHAIN_SLOT_DURATION_MILLIS,
	BLOCK_PROCESSING_VELOCITY,
	UNINCLUDED_SEGMENT_CAPACITY,
>;

impl parachain_info::Config for Runtime {}

parameter_types! {
	pub MessageQueueServiceWeight: Weight = Perbill::from_percent(35) * RuntimeBlockWeights::get().max_block;
	pub MessageQueueIdleServiceWeight: Weight = Perbill::from_percent(20) * RuntimeBlockWeights::get().max_block;
}

impl pallet_message_queue::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type WeightInfo = weights::pallet_message_queue::WeightInfo<Runtime>;
	#[cfg(feature = "runtime-benchmarks")]
	type MessageProcessor = pallet_message_queue::mock_helpers::NoopMessageProcessor<
		cumulus_primitives_core::AggregateMessageOrigin,
	>;
	#[cfg(not(feature = "runtime-benchmarks"))]
	type MessageProcessor = xcm_builder::ProcessXcmMessage<
		AggregateMessageOrigin,
		xcm_executor::XcmExecutor<xcm_config::XcmConfig>,
		RuntimeCall,
	>;
	type Size = u32;
	// The XCMP queue pallet is only ever able to handle the `Sibling(ParaId)` origin:
	type QueueChangeHandler = NarrowOriginToSibling<XcmpQueue>;
	type QueuePausedQuery = NarrowOriginToSibling<XcmpQueue>;
	type HeapSize = sp_core::ConstU32<{ 64 * 1024 }>;
	type MaxStale = sp_core::ConstU32<8>;
	type ServiceWeight = MessageQueueServiceWeight;
	type IdleMaxServiceWeight = MessageQueueIdleServiceWeight;
}

impl cumulus_pallet_aura_ext::Config for Runtime {}

parameter_types! {
	// Fellows pluralistic body.
	pub const FellowsBodyId: BodyId = BodyId::Technical;
	/// The asset ID for the asset that we use to pay for message delivery fees.
	pub FeeAssetId: AssetId = AssetId(xcm_config::UntLocation::get());
	/// The base fee for the message delivery fees.
	pub const ToSiblingBaseDeliveryFee: u128 = MILLI.saturating_mul(3);
	pub const ToParentBaseDeliveryFee: u128 = MILLI.saturating_mul(3);
}

pub type PriceForSiblingParachainDelivery = polkadot_runtime_common::xcm_sender::ExponentialPrice<
	FeeAssetId,
	ToSiblingBaseDeliveryFee,
	TransactionByteFee,
	XcmpQueue,
>;
pub type PriceForParentDelivery = polkadot_runtime_common::xcm_sender::ExponentialPrice<
	FeeAssetId,
	ToParentBaseDeliveryFee,
	TransactionByteFee,
	ParachainSystem,
>;

impl cumulus_pallet_xcmp_queue::Config for Runtime {
	type WeightInfo = weights::cumulus_pallet_xcmp_queue::WeightInfo<Runtime>;
	type RuntimeEvent = RuntimeEvent;
	type ChannelInfo = ParachainSystem;
	type VersionWrapper = PolkadotXcm;
	// Enqueue XCMP messages from siblings for later processing.
	type XcmpQueue = TransformOrigin<MessageQueue, AggregateMessageOrigin, ParaId, ParaIdToSibling>;
	type MaxActiveOutboundChannels = ConstU32<128>;
	// Most on-chain HRMP channels are configured to use 102400 bytes of max message size, so we
	// need to set the page size larger than that until we reduce the channel size on-chain.
	type MaxPageSize = ConstU32<{ 103 * 1024 }>;
	type MaxInboundSuspended = sp_core::ConstU32<1_000>;
	type ControllerOrigin = EitherOfDiverse<
		EnsureRoot<AccountId>,
		EnsureXcm<IsVoiceOfBody<FellowshipLocation, FellowsBodyId>>,
	>;
	type ControllerOriginConverter = XcmOriginToTransactDispatchOrigin;
	type PriceForSiblingDelivery = PriceForSiblingParachainDelivery;
}

impl cumulus_pallet_xcmp_queue::migration::v5::V5Config for Runtime {
	// This must be the same as the `ChannelInfo` from the `Config`:
	type ChannelList = ParachainSystem;
}

parameter_types! {
	pub const Period: u32 = 6 * HOURS;
	pub const Offset: u32 = 0;
}

impl pallet_session::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type ValidatorId = <Self as frame_system::Config>::AccountId;
	// we don't have stash and controller, thus we don't need the convert as well.
	type ValidatorIdOf = pallet_collator_selection::IdentityCollator;
	type ShouldEndSession = pallet_session::PeriodicSessions<Period, Offset>;
	type NextSessionRotation = pallet_session::PeriodicSessions<Period, Offset>;
	type SessionManager = CollatorSelection;
	// Essentially just Aura, but let's be pedantic.
	type SessionHandler = <SessionKeys as sp_runtime::traits::OpaqueKeys>::KeyTypeIdProviders;
	type Keys = SessionKeys;
	type WeightInfo = weights::pallet_session::WeightInfo<Runtime>;
}

impl pallet_aura::Config for Runtime {
	type AuthorityId = AuraId;
	type DisabledValidators = ();
	type MaxAuthorities = ConstU32<100_000>;
	type AllowMultipleBlocksPerSlot = ConstBool<true>;
	type SlotDuration = ConstU64<SLOT_DURATION>;
}

parameter_types! {
	pub const PotId: PalletId = PalletId(*b"PotStake");
	pub const SessionLength: BlockNumber = 6 * HOURS;
	// `StakingAdmin` pluralistic body.
	pub const StakingAdminBodyId: BodyId = BodyId::Defense;
}

/// We allow root and the `StakingAdmin` to execute privileged collator selection operations.
pub type CollatorSelectionUpdateOrigin = EitherOfDiverse<
	EnsureRoot<AccountId>,
	EnsureXcm<IsVoiceOfBody<GovernanceLocation, StakingAdminBodyId>>,
>;

impl pallet_collator_selection::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Currency = Balances;
	type UpdateOrigin = CollatorSelectionUpdateOrigin;
	type PotId = PotId;
	type MaxCandidates = ConstU32<100>;
	type MinEligibleCollators = ConstU32<4>;
	type MaxInvulnerables = ConstU32<20>;
	// should be a multiple of session or things will get inconsistent
	type KickThreshold = Period;
	type ValidatorId = <Self as frame_system::Config>::AccountId;
	type ValidatorIdOf = pallet_collator_selection::IdentityCollator;
	type ValidatorRegistration = Session;
	type WeightInfo = weights::pallet_collator_selection::WeightInfo<Runtime>;
}

impl pallet_asset_conversion_tx_payment::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Fungibles = LocalAndForeignAssets;
	type OnChargeAssetTransaction =
		impls::tx_payment::SwapCreditAdapter<UntLocationV3, AssetConversion>;
}

parameter_types! {
	pub const UniquesCollectionDeposit: Balance = 10 * UNITS; // 10 UNIT deposit to create uniques class
	pub const UniquesItemDeposit: Balance = UNITS / 100; // 1 / 100 UNIT deposit to create uniques instance
	pub const UniquesMetadataDepositBase: Balance = system_para_deposit(1, 129);
	pub const UniquesAttributeDepositBase: Balance = system_para_deposit(1, 0);
	pub const UniquesDepositPerByte: Balance = system_para_deposit(0, 1);
}

impl pallet_uniques::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type CollectionId = u32;
	type ItemId = u32;
	type Currency = Balances;
	type ForceOrigin = AssetsForceOrigin;
	type CollectionDeposit = UniquesCollectionDeposit;
	type ItemDeposit = UniquesItemDeposit;
	type MetadataDepositBase = UniquesMetadataDepositBase;
	type AttributeDepositBase = UniquesAttributeDepositBase;
	type DepositPerByte = UniquesDepositPerByte;
	type StringLimit = ConstU32<128>;
	type KeyLimit = ConstU32<32>; // Max 32 bytes per key
	type ValueLimit = ConstU32<64>; // Max 64 bytes per value
	type WeightInfo = weights::pallet_uniques::WeightInfo<Runtime>;
	#[cfg(feature = "runtime-benchmarks")]
	type Helper = ();
	type CreateOrigin = AsEnsureOriginWithArg<EnsureSigned<AccountId>>;
	type Locker = ();
}

parameter_types! {
	pub NftsPalletFeatures: PalletFeatures = PalletFeatures::all_enabled();
	pub const NftsMaxDeadlineDuration: BlockNumber = 12 * 30 * DAYS;
	// re-use the Uniques deposits
	pub const NftsCollectionDeposit: Balance = system_para_deposit(1, 130);
	pub const NftsItemDeposit: Balance = system_para_deposit(1, 164) / 40;
	pub const NftsMetadataDepositBase: Balance = system_para_deposit(1, 129) / 10;
	pub const NftsAttributeDepositBase: Balance = system_para_deposit(1, 0) / 10;
	pub const NftsDepositPerByte: Balance = system_para_deposit(0, 1);
}

impl pallet_nfts::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type CollectionId = u32;
	type ItemId = u32;
	type Currency = Balances;
	type CreateOrigin = AsEnsureOriginWithArg<EnsureSigned<AccountId>>;
	type ForceOrigin = AssetsForceOrigin;
	type Locker = ();
	type CollectionDeposit = NftsCollectionDeposit;
	type ItemDeposit = NftsItemDeposit;
	type MetadataDepositBase = NftsMetadataDepositBase;
	type AttributeDepositBase = NftsAttributeDepositBase;
	type DepositPerByte = NftsDepositPerByte;
	type StringLimit = ConstU32<256>;
	type KeyLimit = ConstU32<64>;
	type ValueLimit = ConstU32<256>;
	type ApprovalsLimit = ConstU32<20>;
	type ItemAttributesApprovalsLimit = ConstU32<30>;
	type MaxTips = ConstU32<10>;
	type MaxDeadlineDuration = NftsMaxDeadlineDuration;
	type MaxAttributesPerCall = ConstU32<10>;
	type Features = NftsPalletFeatures;
	type OffchainSignature = Signature;
	type OffchainPublic = <Signature as Verify>::Signer;
	type WeightInfo = weights::pallet_nfts::WeightInfo<Runtime>;
	#[cfg(feature = "runtime-benchmarks")]
	type Helper = ();
}

pub type PoolAssetsInstance = pallet_assets::Instance3;
impl pallet_assets::Config<PoolAssetsInstance> for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Balance = Balance;
	type RemoveItemsLimit = ConstU32<1000>;
	type AssetId = u32;
	type AssetIdParameter = u32;
	type Currency = Balances;
	type CreateOrigin = NeverEnsureOrigin<AccountId>;
	type ForceOrigin = AssetsForceOrigin;
	type AssetDeposit = ConstU128<0>;
	type AssetAccountDeposit = AssetAccountDeposit;
	type MetadataDepositBase = ConstU128<0>;
	type MetadataDepositPerByte = ConstU128<0>;
	type ApprovalDeposit = ExistentialDeposit;
	type StringLimit = ConstU32<50>;
	type Freezer = ();
	type Extra = ();
	type CallbackHandle = ();
	type WeightInfo = weights::pallet_assets_pool::WeightInfo<Runtime>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = ();
}

/// Union fungibles implementation for `Assets`` and `ForeignAssets`.
pub type LocalAndForeignAssets = fungibles::UnionOf<
	Assets,
	ForeignAssets,
	LocalFromLeft<
		AssetIdForTrustBackedAssetsConvert<TrustBackedAssetsPalletLocationV3, xcm::v3::Location>,
		AssetIdForTrustBackedAssets,
		xcm::v3::Location,
	>,
	xcm::v3::Location,
	AccountId,
>;

/// Union fungibles implementation for [`LocalAndForeignAssets`] and `Balances`.
pub type NativeAndAssets = fungible::UnionOf<
	Balances,
	LocalAndForeignAssets,
	TargetFromLeft<UntLocationV3, xcm::v3::Location>,
	xcm::v3::Location,
	AccountId,
>;

parameter_types! {
	pub const AssetConversionPalletId: PalletId = PalletId(*b"py/ascon");
	pub const LiquidityWithdrawalFee: Permill = Permill::from_percent(0);
	// Storage deposit for pool setup within asset conversion pallet
	// and pool's lp token creation within assets pallet.
	pub const PoolSetupFee: Balance = system_para_deposit(1, 4) + AssetDeposit::get();
}

pub type PoolIdToAccountId = pallet_asset_conversion::AccountIdConverter<
	AssetConversionPalletId,
	(xcm::v3::Location, xcm::v3::Location),
>;

impl pallet_asset_conversion::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Balance = Balance;
	type HigherPrecisionBalance = sp_core::U256;
	type AssetKind = xcm::v3::Location;
	type Assets = NativeAndAssets;
	type PoolId = (Self::AssetKind, Self::AssetKind);
	type PoolLocator = pallet_asset_conversion::WithFirstAsset<
		UntLocationV3,
		AccountId,
		Self::AssetKind,
		PoolIdToAccountId,
	>;
	type PoolAssetId = u32;
	type PoolAssets = PoolAssets;
	type PoolSetupFee = PoolSetupFee;
	type PoolSetupFeeAsset = UntLocationV3;
	type PoolSetupFeeTarget = ResolveAssetTo<xcm_config::RelayTreasuryPalletAccount, Self::Assets>;
	type LiquidityWithdrawalFee = LiquidityWithdrawalFee;
	type LPFee = ConstU32<3>;
	type PalletId = AssetConversionPalletId;
	type MaxSwapPathLength = ConstU32<3>;
	type MintMinLiquidity = ConstU128<100>;
	type WeightInfo = weights::pallet_asset_conversion::WeightInfo<Runtime>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = assets_common::benchmarks::AssetPairFactory<
		UntLocationV3,
		parachain_info::Pallet<Runtime>,
		xcm_config::TrustBackedAssetsPalletIndex,
		Self::AssetKind,
	>;
}

impl pallet_sudo::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeCall = RuntimeCall;
	type WeightInfo = ();
}

// Create the runtime by composing the FRAME pallets that were previously configured.
construct_runtime!(
	pub enum Runtime
	{
		// System support stuff.
		System: frame_system = 0,
		ParachainSystem: cumulus_pallet_parachain_system = 1,
		// RandomnessCollectiveFlip = 2 removed
		Timestamp: pallet_timestamp = 3,
		ParachainInfo: parachain_info = 4,

		// Monetary stuff.
		Balances: pallet_balances = 10,
		TransactionPayment: pallet_transaction_payment = 11,
		AssetTxPayment: pallet_asset_conversion_tx_payment = 13,
		// Vesting: pallet_vesting = 14 removed,

		// Collator support. the order of these 5 are important and shall not change.
		Authorship: pallet_authorship = 20,
		CollatorSelection: pallet_collator_selection = 21,
		Session: pallet_session = 22,
		Aura: pallet_aura = 23,
		AuraExt: cumulus_pallet_aura_ext = 24,

		// XCM helpers.
		XcmpQueue: cumulus_pallet_xcmp_queue = 30,
		PolkadotXcm: pallet_xcm = 31,
		CumulusXcm: cumulus_pallet_xcm = 32,
		// DmpQueue: cumulus_pallet_dmp_queue = 33, removed
		MessageQueue: pallet_message_queue = 35,

		// Handy utilities.
		Utility: pallet_utility = 40,
		Multisig: pallet_multisig = 41,
		Proxy: pallet_proxy = 42,

		// The main stage.
		Assets: pallet_assets::<Instance1> = 50,
		Uniques: pallet_uniques = 51,
		Nfts: pallet_nfts = 52,
		ForeignAssets: pallet_assets::<Instance2> = 53,
		PoolAssets: pallet_assets::<Instance3> = 54,
		AssetConversion: pallet_asset_conversion = 55,

		Sudo: pallet_sudo = 100,
	}
);

/// The address format for describing accounts.
pub type Address = sp_runtime::MultiAddress<AccountId, ()>;
/// Block type as expected by this runtime.
pub type Block = generic::Block<Header, UncheckedExtrinsic>;
/// A Block signed with a Justification
pub type SignedBlock = generic::SignedBlock<Block>;
/// BlockId type as expected by this runtime.
pub type BlockId = generic::BlockId<Block>;
/// The SignedExtension to the basic transaction logic.
pub type SignedExtra = (
	frame_system::CheckNonZeroSender<Runtime>,
	frame_system::CheckSpecVersion<Runtime>,
	frame_system::CheckTxVersion<Runtime>,
	frame_system::CheckGenesis<Runtime>,
	frame_system::CheckEra<Runtime>,
	frame_system::CheckNonce<Runtime>,
	frame_system::CheckWeight<Runtime>,
	pallet_asset_conversion_tx_payment::ChargeAssetTxPayment<Runtime>,
	frame_metadata_hash_extension::CheckMetadataHash<Runtime>,
);
/// Unchecked extrinsic type as expected by this runtime.
pub type UncheckedExtrinsic =
	generic::UncheckedExtrinsic<Address, RuntimeCall, Signature, SignedExtra>;

parameter_types! {
	pub DmpQueueName: &'static str = "DmpQueue";
}

/// Migrations to apply on runtime upgrade.
pub type Migrations = (
	// unreleased
	frame_support::migrations::RemovePallet<DmpQueueName, RocksDbWeight>,
	cumulus_pallet_xcmp_queue::migration::v4::MigrationToV4<Runtime>,
	pallet_collator_selection::migration::v2::MigrationToV2<Runtime>,
	cumulus_pallet_xcmp_queue::migration::v5::MigrateV4ToV5<Runtime>,
	pallet_assets::migration::next_asset_id::SetNextAssetId<
		ConstU32<50_000_000>,
		Runtime,
		TrustBackedAssetsInstance,
	>,
	// permanent
	pallet_xcm::migration::MigrateToLatestXcmVersion<Runtime>,
);

/// Executive: handles dispatch to the various modules.
pub type Executive = frame_executive::Executive<
	Runtime,
	Block,
	frame_system::ChainContext<Runtime>,
	Runtime,
	AllPalletsWithSystem,
	Migrations,
>;

#[cfg(feature = "runtime-benchmarks")]
mod benches {
	frame_benchmarking::define_benchmarks!(
		[frame_system, SystemBench::<Runtime>]
		[pallet_assets, Local]
		[pallet_assets, Foreign]
		[pallet_assets, Pool]
		[pallet_asset_conversion, AssetConversion]
		[pallet_balances, Balances]
		[pallet_message_queue, MessageQueue]
		[pallet_multisig, Multisig]
		[pallet_nfts, Nfts]
		[pallet_proxy, Proxy]
		[pallet_session, SessionBench::<Runtime>]
		[pallet_uniques, Uniques]
		[pallet_utility, Utility]
		[pallet_timestamp, Timestamp]
		[pallet_collator_selection, CollatorSelection]
		[cumulus_pallet_parachain_system, ParachainSystem]
		[cumulus_pallet_xcmp_queue, XcmpQueue]
		// XCM
		[pallet_xcm, PalletXcmExtrinsiscsBenchmark::<Runtime>]
		// Bridges
		// [pallet_xcm_bridge_hub_router, ToKusama]
		// NOTE: Make sure you point to the individual modules below.
		[pallet_xcm_benchmarks::fungible, XcmBalances]
		[pallet_xcm_benchmarks::generic, XcmGeneric]
	);
}

impl_runtime_apis! {
	impl sp_consensus_aura::AuraApi<Block, AuraId> for Runtime {
		fn slot_duration() -> sp_consensus_aura::SlotDuration {
			sp_consensus_aura::SlotDuration::from_millis(SLOT_DURATION)
		}

		fn authorities() -> Vec<AuraId> {
			pallet_aura::Authorities::<Runtime>::get().into_inner()
		}
	}

	impl cumulus_primitives_aura::AuraUnincludedSegmentApi<Block> for Runtime {
		fn can_build_upon(
			included_hash: <Block as BlockT>::Hash,
			slot: cumulus_primitives_aura::Slot,
		) -> bool {
			ConsensusHook::can_build_upon(included_hash, slot)
		}
	}

	impl sp_api::Core<Block> for Runtime {
		fn version() -> RuntimeVersion {
			VERSION
		}

		fn execute_block(block: Block) {
			Executive::execute_block(block)
		}

		fn initialize_block(header: &<Block as BlockT>::Header) -> sp_runtime::ExtrinsicInclusionMode {
			Executive::initialize_block(header)
		}
	}

	impl sp_api::Metadata<Block> for Runtime {
		fn metadata() -> OpaqueMetadata {
			OpaqueMetadata::new(Runtime::metadata().into())
		}

		fn metadata_at_version(version: u32) -> Option<OpaqueMetadata> {
			Runtime::metadata_at_version(version)
		}

		fn metadata_versions() -> sp_std::vec::Vec<u32> {
			Runtime::metadata_versions()
		}
	}

	impl sp_block_builder::BlockBuilder<Block> for Runtime {
		fn apply_extrinsic(extrinsic: <Block as BlockT>::Extrinsic) -> ApplyExtrinsicResult {
			Executive::apply_extrinsic(extrinsic)
		}

		fn finalize_block() -> <Block as BlockT>::Header {
			Executive::finalize_block()
		}

		fn inherent_extrinsics(data: sp_inherents::InherentData) -> Vec<<Block as BlockT>::Extrinsic> {
			data.create_extrinsics()
		}

		fn check_inherents(
			block: Block,
			data: sp_inherents::InherentData,
		) -> sp_inherents::CheckInherentsResult {
			data.check_extrinsics(&block)
		}
	}

	impl sp_transaction_pool::runtime_api::TaggedTransactionQueue<Block> for Runtime {
		fn validate_transaction(
			source: TransactionSource,
			tx: <Block as BlockT>::Extrinsic,
			block_hash: <Block as BlockT>::Hash,
		) -> TransactionValidity {
			Executive::validate_transaction(source, tx, block_hash)
		}
	}

	impl sp_offchain::OffchainWorkerApi<Block> for Runtime {
		fn offchain_worker(header: &<Block as BlockT>::Header) {
			Executive::offchain_worker(header)
		}
	}

	impl sp_session::SessionKeys<Block> for Runtime {
		fn generate_session_keys(seed: Option<Vec<u8>>) -> Vec<u8> {
			SessionKeys::generate(seed)
		}

		fn decode_session_keys(
			encoded: Vec<u8>,
		) -> Option<Vec<(Vec<u8>, KeyTypeId)>> {
			SessionKeys::decode_into_raw_public_keys(&encoded)
		}
	}

	impl frame_system_rpc_runtime_api::AccountNonceApi<Block, AccountId, Nonce> for Runtime {
		fn account_nonce(account: AccountId) -> Nonce {
			System::account_nonce(account)
		}
	}

	impl pallet_transaction_payment_rpc_runtime_api::TransactionPaymentApi<Block, Balance> for Runtime {
		fn query_info(
			uxt: <Block as BlockT>::Extrinsic,
			len: u32,
		) -> pallet_transaction_payment_rpc_runtime_api::RuntimeDispatchInfo<Balance> {
			TransactionPayment::query_info(uxt, len)
		}
		fn query_fee_details(
			uxt: <Block as BlockT>::Extrinsic,
			len: u32,
		) -> pallet_transaction_payment::FeeDetails<Balance> {
			TransactionPayment::query_fee_details(uxt, len)
		}
		fn query_weight_to_fee(weight: Weight) -> Balance {
			TransactionPayment::weight_to_fee(weight)
		}
		fn query_length_to_fee(length: u32) -> Balance {
			TransactionPayment::length_to_fee(length)
		}
	}

	impl pallet_transaction_payment_rpc_runtime_api::TransactionPaymentCallApi<Block, Balance, RuntimeCall>
		for Runtime
	{
		fn query_call_info(
			call: RuntimeCall,
			len: u32,
		) -> pallet_transaction_payment::RuntimeDispatchInfo<Balance> {
			TransactionPayment::query_call_info(call, len)
		}
		fn query_call_fee_details(
			call: RuntimeCall,
			len: u32,
		) -> pallet_transaction_payment::FeeDetails<Balance> {
			TransactionPayment::query_call_fee_details(call, len)
		}
		fn query_weight_to_fee(weight: Weight) -> Balance {
			TransactionPayment::weight_to_fee(weight)
		}
		fn query_length_to_fee(length: u32) -> Balance {
			TransactionPayment::length_to_fee(length)
		}
	}

	impl xcm_runtime_apis::fees::XcmPaymentApi<Block> for Runtime {
		fn query_acceptable_payment_assets(xcm_version: xcm::Version) -> Result<Vec<VersionedAssetId>, XcmPaymentApiError> {
			let acceptable_assets = vec![AssetId(xcm_config::UntLocation::get())];
			PolkadotXcm::query_acceptable_payment_assets(xcm_version, acceptable_assets)
		}

		fn query_weight_to_asset_fee(weight: Weight, asset: VersionedAssetId) -> Result<u128, XcmPaymentApiError> {
			match asset.try_as::<AssetId>() {
				Ok(asset_id) if asset_id.0 == xcm_config::UntLocation::get() => {
					// for native token
					Ok(WeightToFee::weight_to_fee(&weight))
				},
				Ok(asset_id) => {
					log::trace!(target: "xcm::xcm_runtime_apis", "query_weight_to_asset_fee - unhandled asset_id: {asset_id:?}!");
					Err(XcmPaymentApiError::AssetNotFound)
				},
				Err(_) => {
					log::trace!(target: "xcm::xcm_runtime_apis", "query_weight_to_asset_fee - failed to convert asset: {asset:?}!");
					Err(XcmPaymentApiError::VersionedConversionFailed)
				}
			}
		}

		fn query_xcm_weight(message: VersionedXcm<()>) -> Result<Weight, XcmPaymentApiError> {
			PolkadotXcm::query_xcm_weight(message)
		}

		fn query_delivery_fees(destination: VersionedLocation, message: VersionedXcm<()>) -> Result<VersionedAssets, XcmPaymentApiError> {
			PolkadotXcm::query_delivery_fees(destination, message)
		}
	}

	impl xcm_runtime_apis::dry_run::DryRunApi<Block, RuntimeCall, RuntimeEvent, OriginCaller> for Runtime {
		fn dry_run_call(origin: OriginCaller, call: RuntimeCall) -> Result<CallDryRunEffects<RuntimeEvent>, XcmDryRunApiError> {
			PolkadotXcm::dry_run_call::<Runtime, xcm_config::XcmRouter, OriginCaller, RuntimeCall>(origin, call)
		}

		fn dry_run_xcm(origin_location: VersionedLocation, xcm: VersionedXcm<RuntimeCall>) -> Result<XcmDryRunEffects<RuntimeEvent>, XcmDryRunApiError> {
			PolkadotXcm::dry_run_xcm::<Runtime, xcm_config::XcmRouter, RuntimeCall, xcm_config::XcmConfig>(origin_location, xcm)
		}
	}

	impl xcm_runtime_apis::conversions::LocationToAccountApi<Block, AccountId> for Runtime {
		fn convert_location(location: VersionedLocation) -> Result<
			AccountId,
			xcm_runtime_apis::conversions::Error
		> {
			xcm_runtime_apis::conversions::LocationToAccountHelper::<
				AccountId,
				xcm_config::LocationToAccountId,
			>::convert_location(location)
		}
	}

	impl assets_common::runtime_api::FungiblesApi<
		Block,
		AccountId,
	> for Runtime
	{
		fn query_account_balances(account: AccountId) -> Result<xcm::VersionedAssets, assets_common::runtime_api::FungiblesAccessError> {
			use assets_common::fungible_conversion::{convert, convert_balance};
			Ok([
				// collect pallet_balance
				{
					let balance = Balances::free_balance(account.clone());
					if balance > 0 {
						vec![convert_balance::<UntLocation, Balance>(balance)?]
					} else {
						vec![]
					}
				},
				// collect pallet_assets (TrustBackedAssets)
				convert::<_, _, _, _, TrustBackedAssetsConvertedConcreteId>(
					Assets::account_balances(account.clone())
						.iter()
						.filter(|(_, balance)| balance > &0)
				)?,
				// collect pallet_assets (ForeignAssets)
				convert::<_, _, _, _, ForeignAssetsConvertedConcreteId>(
					ForeignAssets::account_balances(account.clone())
						.iter()
						.filter(|(_, balance)| balance > &0)
				)?,
				// collect pallet_assets (PoolAssets)
				convert::<_, _, _, _, PoolAssetsConvertedConcreteId>(
					PoolAssets::account_balances(account)
					.iter()
					.filter(|(_, balance)| balance > &0)
				)?,
				// collect ... e.g. other tokens
			].concat().into())
		}
	}

	impl cumulus_primitives_core::CollectCollationInfo<Block> for Runtime {
		fn collect_collation_info(header: &<Block as BlockT>::Header) -> cumulus_primitives_core::CollationInfo {
			ParachainSystem::collect_collation_info(header)
		}
	}

	impl sp_genesis_builder::GenesisBuilder<Block> for Runtime {
		fn build_state(config: Vec<u8>) -> sp_genesis_builder::Result {
			build_state::<RuntimeGenesisConfig>(config)
		}

		fn get_preset(id: &Option<sp_genesis_builder::PresetId>) -> Option<Vec<u8>> {
			get_preset::<RuntimeGenesisConfig>(id, &genesis_config_presets::get_preset)
		}

		fn preset_names() -> Vec<sp_genesis_builder::PresetId> {
			vec![
				sp_genesis_builder::PresetId::from("local_testnet"),
				sp_genesis_builder::PresetId::from("development"),
			]
		}
	}

	impl pallet_asset_conversion::AssetConversionApi<Block, Balance, xcm::v3::Location> for Runtime {
		fn quote_price_exact_tokens_for_tokens(
			asset1: xcm::v3::Location,
			asset2: xcm::v3::Location,
			amount: Balance,
			include_fee: bool,
		) -> Option<Balance> {
			AssetConversion::quote_price_exact_tokens_for_tokens(
				asset1,
				asset2,
				amount,
				include_fee,
			)
		}

		fn quote_price_tokens_for_exact_tokens(
			asset1: xcm::v3::Location,
			asset2: xcm::v3::Location,
			amount: Balance,
			include_fee: bool,
		) -> Option<Balance> {
			AssetConversion::quote_price_tokens_for_exact_tokens(
				asset1,
				asset2,
				amount,
				include_fee,
			)
		}

		fn get_reserves(
			asset1: xcm::v3::Location,
			asset2: xcm::v3::Location,
		) -> Option<(Balance, Balance)> {
			AssetConversion::get_reserves(asset1, asset2).ok()
		}
	}

	#[cfg(feature = "try-runtime")]
	impl frame_try_runtime::TryRuntime<Block> for Runtime {
		fn on_runtime_upgrade(checks: frame_try_runtime::UpgradeCheckSelect) -> (Weight, Weight) {
			let weight = Executive::try_runtime_upgrade(checks).unwrap();
			(weight, RuntimeBlockWeights::get().max_block)
		}

		fn execute_block(
			block: Block,
			state_root_check: bool,
			signature_check: bool,
			select: frame_try_runtime::TryStateSelect,
		) -> Weight {
			// NOTE: intentional unwrap: we don't want to propagate the error backwards, and want to
			// have a backtrace here.
			Executive::try_execute_block(block, state_root_check, signature_check, select).unwrap()
		}
	}

	#[cfg(feature = "runtime-benchmarks")]
	impl frame_benchmarking::Benchmark<Block> for Runtime {
		fn benchmark_metadata(extra: bool) -> (
			Vec<frame_benchmarking::BenchmarkList>,
			Vec<frame_support::traits::StorageInfo>,
		) {
			use frame_benchmarking::{Benchmarking, BenchmarkList};
			use frame_support::traits::StorageInfoTrait;
			use pallet_xcm::benchmarking::Pallet as PalletXcmExtrinsiscsBenchmark;
			use frame_system_benchmarking::Pallet as SystemBench;
			use cumulus_pallet_session_benchmarking::Pallet as SessionBench;
			use pallet_xcm_bridge_hub_router::benchmarking::Pallet as XcmBridgeHubRouterBench;

			// This is defined once again in dispatch_benchmark, because list_benchmarks!
			// and add_benchmarks! are macros exported by define_benchmarks! macros and those types
			// are referenced in that call.
			type XcmBalances = pallet_xcm_benchmarks::fungible::Pallet::<Runtime>;
			type XcmGeneric = pallet_xcm_benchmarks::generic::Pallet::<Runtime>;

			// Benchmark files generated for `Assets/ForeignAssets` instances are by default
			// `pallet_assets_assets.rs / pallet_assets_foreign_assets`, which is not really nice,
			// so with this redefinition we can change names to nicer:
			// `pallet_assets_local.rs / pallet_assets_foreign.rs`.
			type Local = pallet_assets::Pallet::<Runtime, TrustBackedAssetsInstance>;
			type Foreign = pallet_assets::Pallet::<Runtime, ForeignAssetsInstance>;
			type Pool = pallet_assets::Pallet::<Runtime, PoolAssetsInstance>;

			type ToKusama = XcmBridgeHubRouterBench<Runtime>;

			let mut list = Vec::<BenchmarkList>::new();
			list_benchmarks!(list, extra);

			let storage_info = AllPalletsWithSystem::storage_info();
			(list, storage_info)
		}

		fn dispatch_benchmark(
			config: frame_benchmarking::BenchmarkConfig
		) -> Result<Vec<frame_benchmarking::BenchmarkBatch>, sp_runtime::RuntimeString> {
			use frame_benchmarking::{Benchmarking, BenchmarkBatch, BenchmarkError};
			use sp_storage::TrackedStorageKey;
			use xcm::latest::prelude::{
				Asset, Fungible, Here, InteriorLocation, Junction, Junction::*, Location, NetworkId,
				NonFungible, Parent, ParentThen, Response, XCM_VERSION, Assets as XcmAssets,
			};

			use frame_system_benchmarking::Pallet as SystemBench;
			impl frame_system_benchmarking::Config for Runtime {
				fn setup_set_code_requirements(code: &sp_std::vec::Vec<u8>) -> Result<(), BenchmarkError> {
					ParachainSystem::initialize_for_set_code_benchmark(code.len() as u32);
					Ok(())
				}

				fn verify_set_code() {
					System::assert_last_event(cumulus_pallet_parachain_system::Event::<Runtime>::ValidationFunctionStored.into());
				}
			}

			use cumulus_pallet_session_benchmarking::Pallet as SessionBench;
			impl cumulus_pallet_session_benchmarking::Config for Runtime {}

			use xcm_config::{UntLocation, MaxAssetsIntoHolding};
			use pallet_xcm_benchmarks::asset_instance_from;

			parameter_types! {
				pub ExistentialDepositAsset: Option<Asset> = Some((
					UntLocation::get(),
					ExistentialDeposit::get()
				).into());
				pub const RandomParaId: ParaId = ParaId::new(43211234);
			}

			use pallet_xcm::benchmarking::Pallet as PalletXcmExtrinsiscsBenchmark;
			impl pallet_xcm::benchmarking::Config for Runtime {
				type DeliveryHelper = (
					cumulus_primitives_utility::ToParentDeliveryHelper<
						xcm_config::XcmConfig,
						ExistentialDepositAsset,
						PriceForParentDelivery,
					>,
					polkadot_runtime_common::xcm_sender::ToParachainDeliveryHelper<
						xcm_config::XcmConfig,
						ExistentialDepositAsset,
						PriceForSiblingParachainDelivery,
						RandomParaId,
						ParachainSystem,
					>
				);

				fn reachable_dest() -> Option<Location> {
					Some(Parent.into())
				}

				fn teleportable_asset_and_dest() -> Option<(Asset, Location)> {
					// Relay/native token can be teleported between AH and Relay.
					Some((
						Asset {
							fun: Fungible(ExistentialDeposit::get()),
							id: AssetId(Parent.into())
						},
						Parent.into(),
					))
				}

				fn reserve_transferable_asset_and_dest() -> Option<(Asset, Location)> {
					// AH can reserve transfer native token to some random parachain.
					Some((
						Asset {
							fun: Fungible(ExistentialDeposit::get()),
							id: AssetId(Parent.into())
						},
						ParentThen(Parachain(RandomParaId::get().into()).into()).into(),
					))
				}

				fn set_up_complex_asset_transfer(
				) -> Option<(XcmAssets, u32, Location, Box<dyn FnOnce()>)> {
					// Transfer to Relay some local AH asset (local-reserve-transfer) while paying
					// fees using teleported native token.
					// (We don't care that Relay doesn't accept incoming unknown AH local asset)
					let dest = Parent.into();

					let fee_amount = ExistentialDeposit::get();
					let fee_asset: Asset = (Location::parent(), fee_amount).into();

					let who = frame_benchmarking::whitelisted_caller();
					// Give some multiple of the existential deposit
					let balance = fee_amount + ExistentialDeposit::get() * 1000;
					let _ = <Balances as frame_support::traits::Currency<_>>::make_free_balance_be(
						&who, balance,
					);
					// verify initial balance
					assert_eq!(Balances::free_balance(&who), balance);

					// set up local asset
					let asset_amount = 10u128;
					let initial_asset_amount = asset_amount * 10;
					let (asset_id, _, _) = pallet_assets::benchmarking::create_default_minted_asset::<
						Runtime,
						pallet_assets::Instance1
					>(true, initial_asset_amount);
					let asset_location = Location::new(
						0,
						[PalletInstance(50), GeneralIndex(u32::from(asset_id).into())]
					);
					let transfer_asset: Asset = (asset_location, asset_amount).into();

					let assets: XcmAssets = vec![fee_asset.clone(), transfer_asset].into();
					let fee_index = if assets.get(0).unwrap().eq(&fee_asset) { 0 } else { 1 };

					// verify transferred successfully
					let verify = Box::new(move || {
						// verify native balance after transfer, decreased by transferred fee amount
						// (plus transport fees)
						assert!(Balances::free_balance(&who) <= balance - fee_amount);
						// verify asset balance decreased by exactly transferred amount
						assert_eq!(
							Assets::balance(asset_id.into(), &who),
							initial_asset_amount - asset_amount,
						);
					});
					Some((assets, fee_index as u32, dest, verify))
				}

				fn get_asset() -> Asset {
					Asset {
						id: AssetId(Location::parent()),
						fun: Fungible(ExistentialDeposit::get()),
					}
				}
			}

			impl pallet_xcm_benchmarks::Config for Runtime {
				type XcmConfig = xcm_config::XcmConfig;
				type AccountIdConverter = xcm_config::LocationToAccountId;
				type DeliveryHelper = cumulus_primitives_utility::ToParentDeliveryHelper<
					xcm_config::XcmConfig,
					ExistentialDepositAsset,
					PriceForParentDelivery,
				>;
				fn valid_destination() -> Result<Location, BenchmarkError> {
					Ok(UntLocation::get())
				}
				fn worst_case_holding(depositable_count: u32) -> XcmAssets {
					// A mix of fungible, non-fungible, and concrete assets.
					let holding_non_fungibles = MaxAssetsIntoHolding::get() / 2 - depositable_count;
					let holding_fungibles = holding_non_fungibles.saturating_sub(2); // -2 for two `iter::once` bellow
					let fungibles_amount: u128 = 100;
					(0..holding_fungibles)
						.map(|i| {
							Asset {
								id: AssetId(GeneralIndex(i as u128).into()),
								fun: Fungible(fungibles_amount * (i + 1) as u128), // non-zero amount
							}
						})
						.chain(core::iter::once(Asset { id: AssetId(Here.into()), fun: Fungible(u128::MAX) }))
						.chain(core::iter::once(Asset { id: AssetId(UntLocation::get()), fun: Fungible(1_000_000 * UNITS) }))
						.chain((0..holding_non_fungibles).map(|i| Asset {
							id: AssetId(GeneralIndex(i as u128).into()),
							fun: NonFungible(asset_instance_from(i)),
						}))
						.collect::<Vec<_>>()
						.into()
				}
			}

			parameter_types! {
				pub const TrustedTeleporter: Option<(Location, Asset)> = Some((
					UntLocation::get(),
					Asset { fun: Fungible(UNITS), id: AssetId(UntLocation::get()) },
				));
				pub const CheckedAccount: Option<(AccountId, xcm_builder::MintLocation)> = None;
				// AssetHubPolkadot trusts AssetHubKusama as reserve for KSMs
				pub TrustedReserve: Option<(Location, Asset)> = Some(
					(
						xcm_config::bridging::to_kusama::AssetHubKusama::get(),
						Asset::from((xcm_config::bridging::to_kusama::KsmLocation::get(), 1000000000000 as u128))
					)
				);
			}

			impl pallet_xcm_benchmarks::fungible::Config for Runtime {
				type TransactAsset = Balances;

				type CheckedAccount = CheckedAccount;
				type TrustedTeleporter = TrustedTeleporter;
				type TrustedReserve = TrustedReserve;

				fn get_asset() -> Asset {
					Asset {
						id: AssetId(UntLocation::get()),
						fun: Fungible(UNITS),
					}
				}
			}

			impl pallet_xcm_benchmarks::generic::Config for Runtime {
				type TransactAsset = Balances;
				type RuntimeCall = RuntimeCall;

				fn worst_case_response() -> (u64, Response) {
					(0u64, Response::Version(Default::default()))
				}

				fn worst_case_asset_exchange() -> Result<(XcmAssets, XcmAssets), BenchmarkError> {
					Err(BenchmarkError::Skip)
				}

				fn universal_alias() -> Result<(Location, Junction), BenchmarkError> {
					xcm_config::bridging::BridgingBenchmarksHelper::prepare_universal_alias()
					.ok_or(BenchmarkError::Skip)
				}

				fn transact_origin_and_runtime_call() -> Result<(Location, RuntimeCall), BenchmarkError> {
					Ok((UntLocation::get(), frame_system::Call::remark_with_event { remark: vec![] }.into()))
				}

				fn subscribe_origin() -> Result<Location, BenchmarkError> {
					Ok(UntLocation::get())
				}

				fn claimable_asset() -> Result<(Location, Location, XcmAssets), BenchmarkError> {
					let origin = UntLocation::get();
					let assets: XcmAssets = (AssetId(UntLocation::get()), 1_000 * UNITS).into();
					let ticket = Location { parents: 0, interior: Here };
					Ok((origin, ticket, assets))
				}

				fn fee_asset() -> Result<Asset, BenchmarkError> {
					Ok(Asset {
						id: AssetId(UntLocation::get()),
						fun: Fungible(1_000_000 * UNITS),
					})
				}

				fn unlockable_asset() -> Result<(Location, Location, Asset), BenchmarkError> {
					Err(BenchmarkError::Skip)
				}

				fn export_message_origin_and_destination(
				) -> Result<(Location, NetworkId, InteriorLocation), BenchmarkError> {
					Err(BenchmarkError::Skip)
				}

				fn alias_origin() -> Result<(Location, Location), BenchmarkError> {
					Err(BenchmarkError::Skip)
				}
			}

			use pallet_xcm_bridge_hub_router::benchmarking::{
				Pallet as XcmBridgeHubRouterBench,
				Config as XcmBridgeHubRouterConfig,
			};


			type XcmBalances = pallet_xcm_benchmarks::fungible::Pallet::<Runtime>;
			type XcmGeneric = pallet_xcm_benchmarks::generic::Pallet::<Runtime>;

			type Local = pallet_assets::Pallet::<Runtime, TrustBackedAssetsInstance>;
			type Foreign = pallet_assets::Pallet::<Runtime, ForeignAssetsInstance>;
			type Pool = pallet_assets::Pallet::<Runtime, PoolAssetsInstance>;

			type ToKusama = XcmBridgeHubRouterBench<Runtime, ToKusamaXcmRouterInstance>;

			let whitelist: Vec<TrackedStorageKey> = vec![
				// Block Number
				hex_literal::hex!("26aa394eea5630e07c48ae0c9558cef702a5c1b19ab7a04f536c519aca4983ac").to_vec().into(),
				// Total Issuance
				hex_literal::hex!("c2261276cc9d1f8598ea4b6a74b15c2f57c875e4cff74148e4628f264b974c80").to_vec().into(),
				// Execution Phase
				hex_literal::hex!("26aa394eea5630e07c48ae0c9558cef7ff553b5a9862a516939d82b3d3d8661a").to_vec().into(),
				// Event Count
				hex_literal::hex!("26aa394eea5630e07c48ae0c9558cef70a98fdbe9ce6c55837576c60c7af3850").to_vec().into(),
				// System Events
				hex_literal::hex!("26aa394eea5630e07c48ae0c9558cef780d41e5e16056765bc8461851072c9d7").to_vec().into(),
				//TODO: use from relay_well_known_keys::ACTIVE_CONFIG
				hex_literal::hex!("06de3d8a54d27e44a9d5ce189618f22db4b49d95320d9021994c850f25b8e385").to_vec().into(),
			];

			let mut batches = Vec::<BenchmarkBatch>::new();
			let params = (&config, &whitelist);
			add_benchmarks!(params, batches);

			Ok(batches)
		}
	}
}

cumulus_pallet_parachain_system::register_validate_block! {
	Runtime = Runtime,
	BlockExecutor = cumulus_pallet_aura_ext::BlockExecutor::<Runtime, Executive>,
}

#[cfg(test)]
mod tests {
	use super::*;
	use cord_weave_system_parachains_constants::polkadot::fee;
	use sp_runtime::traits::Zero;
	use sp_weights::WeightToFee;

	/// We can fit at least 1000 transfers in a block.
	#[test]
	fn sane_block_weight() {
		use pallet_balances::WeightInfo;
		let block = RuntimeBlockWeights::get().max_block;
		let base = RuntimeBlockWeights::get().get(DispatchClass::Normal).base_extrinsic;
		let transfer =
			base + weights::pallet_balances::WeightInfo::<Runtime>::transfer_allow_death();

		let fit = block.checked_div_per_component(&transfer).unwrap_or_default();
		assert!(fit >= 1000, "{} should be at least 1000", fit);
	}

	/// The fee for one transfer is at most 1 CENT.
	#[test]
	fn sane_transfer_fee() {
		use pallet_balances::WeightInfo;
		let base = RuntimeBlockWeights::get().get(DispatchClass::Normal).base_extrinsic;
		let transfer =
			base + weights::pallet_balances::WeightInfo::<Runtime>::transfer_allow_death();

		let fee: Balance = fee::WeightToFee::weight_to_fee(&transfer);
		assert!(fee <= MILLI, "{} MILLIMILLI should be at most 1000", fee / MILLIMILLI);
	}

	/// Weight is being charged for both dimensions.
	#[test]
	fn weight_charged_for_both_components() {
		let fee: Balance = fee::WeightToFee::weight_to_fee(&Weight::from_parts(20_000, 0));
		assert!(!fee.is_zero(), "Charges for ref time");

		let fee: Balance = fee::WeightToFee::weight_to_fee(&Weight::from_parts(0, 20_000));
		assert_eq!(fee, MILLI, "20kb maps to CENT");
	}

	/// Filling up a block by proof size is at most 30 times more expensive than ref time.
	///
	/// This is just a sanity check.
	#[test]
	fn full_block_fee_ratio() {
		let block = RuntimeBlockWeights::get().max_block;
		let time_fee: Balance =
			fee::WeightToFee::weight_to_fee(&Weight::from_parts(block.ref_time(), 0));
		let proof_fee: Balance =
			fee::WeightToFee::weight_to_fee(&Weight::from_parts(0, block.proof_size()));

		let proof_o_time = proof_fee.checked_div(time_fee).unwrap_or_default();
		assert!(proof_o_time <= 30, "{} should be at most 30", proof_o_time);
		let time_o_proof = time_fee.checked_div(proof_fee).unwrap_or_default();
		assert!(time_o_proof <= 30, "{} should be at most 30", time_o_proof);
	}

	#[test]
	fn test_transasction_byte_fee_is_one_twentieth_of_relay() {
		let relay_tbf = polkadot_runtime_constants::fee::TRANSACTION_BYTE_FEE;
		let parachain_tbf = TransactionByteFee::get();
		assert_eq!(relay_tbf / 20, parachain_tbf);
	}

	#[test]
	fn create_foreign_asset_deposit_is_equal_to_asset_hub_foreign_asset_pallet_deposit() {
		assert_eq!(
			bp_asset_hub_polkadot::CreateForeignAssetDeposit::get(),
			ForeignAssetsAssetDeposit::get()
		);
	}
}