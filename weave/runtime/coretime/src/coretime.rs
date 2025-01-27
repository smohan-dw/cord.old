// This file is part of CORD – https://cord.network

// Copyright (C) Dhiway Networks Pvt. Ltd.
// SPDX-License-Identifier: GPL-3.0-or-later

// CORD is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// CORD is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with CORD. If not, see <https://www.gnu.org/licenses/>.

use crate::*;
use codec::{Decode, Encode};
use cord_loom_runtime_constants::{system_parachain::coretime, time::DAYS as RELAY_DAYS};
use cumulus_pallet_parachain_system::RelaychainDataProvider;
use cumulus_primitives_core::relay_chain;
use frame_support::{
	parameter_types,
	traits::{
		fungible::{Balanced, Credit, Inspect},
		tokens::{Fortitude, Preservation},
		DefensiveResult, OnUnbalanced,
	},
};
use frame_system::Pallet as System;
use pallet_broker::{CoreAssignment, CoreIndex, CoretimeInterface, PartsOf57600, RCBlockNumberOf};
use parachains_common::{AccountId, Balance};
use sp_runtime::traits::AccountIdConversion;
use xcm::latest::prelude::*;
use xcm_executor::traits::TransactAsset;

/// A type containing the encoding of the coretime pallet in the Relay chain runtime. Used to
/// construct any remote calls. The codec index must correspond to the index of `Coretime` in the
/// `construct_runtime` of the Relay chain.
#[derive(Encode, Decode)]
enum RelayRuntimePallets {
	#[codec(index = 74)]
	Coretime(CoretimeProviderCalls),
}

/// Call encoding for the calls needed from the relay coretime pallet.
#[derive(Encode, Decode)]
enum CoretimeProviderCalls {
	#[codec(index = 1)]
	RequestCoreCount(CoreIndex),
	#[codec(index = 2)]
	RequestRevenueInfoAt(relay_chain::BlockNumber),
	#[codec(index = 3)]
	CreditAccount(AccountId, Balance),
	#[codec(index = 4)]
	AssignCore(
		CoreIndex,
		relay_chain::BlockNumber,
		Vec<(CoreAssignment, PartsOf57600)>,
		Option<relay_chain::BlockNumber>,
	),
}

parameter_types! {
	/// The holding account into which burnt funds will be moved at the point of sale. This will be
	/// burnt periodically.
	pub CoretimeBurnAccount: AccountId = PalletId(*b"py/ctbrn").into_account_truncating();
}

/// Burn revenue from coretime sales. See
/// [RFC-010](https://polkadot-fellows.github.io/RFCs/approved/0010-burn-coretime-revenue.html).
pub struct BurnCoretimeRevenue;
impl OnUnbalanced<Credit<AccountId, Balances>> for BurnCoretimeRevenue {
	fn on_nonzero_unbalanced(amount: Credit<AccountId, Balances>) {
		let acc = CoretimeBurnAccount::get();
		if !System::<Runtime>::account_exists(&acc) {
			// The account doesn't require ED to survive.
			System::<Runtime>::inc_providers(&acc);
		}
		Balances::resolve(&acc, amount).defensive_ok();
	}
}

type AssetTransactor = <xcm_config::XcmConfig as xcm_executor::Config>::AssetTransactor;

fn burn_at_relay(stash: &AccountId, value: Balance) -> Result<(), XcmError> {
	let dest = Location::parent();
	let stash_location =
		Junction::AccountId32 { network: None, id: stash.clone().into() }.into_location();
	let asset = Asset { id: AssetId(Location::parent()), fun: Fungible(value) };
	let dummy_xcm_context = XcmContext { origin: None, message_id: [0; 32], topic: None };

	let withdrawn = AssetTransactor::withdraw_asset(&asset, &stash_location, None)?;

	// TODO https://github.com/polkadot-fellows/runtimes/issues/404
	AssetTransactor::can_check_out(&dest, &asset, &dummy_xcm_context)?;

	let parent_assets = Into::<Assets>::into(withdrawn)
		.reanchored(&dest, &Here)
		.defensive_map_err(|_| XcmError::ReanchorFailed)?;

	PolkadotXcm::send_xcm(
		Here,
		Location::parent(),
		Xcm(vec![
			Instruction::UnpaidExecution {
				weight_limit: WeightLimit::Unlimited,
				check_origin: None,
			},
			ReceiveTeleportedAsset(parent_assets.clone()),
			BurnAsset(parent_assets),
		]),
	)?;

	AssetTransactor::check_out(&dest, &asset, &dummy_xcm_context);

	Ok(())
}

parameter_types! {
	/// The revenue from on-demand coretime sales. This is distributed amonst those who contributed
	/// regions to the pool.
	pub storage CoretimeRevenue: Option<(BlockNumber, Balance)> = None;
}

/// Type that implements the [`CoretimeInterface`] for the allocation of Coretime. Meant to operate
/// from the parachain context. That is, the parachain provides a market (broker) for the sale of
/// coretime, but assumes a `CoretimeProvider` (i.e. a Relay Chain) to actually provide cores.
pub struct CoretimeAllocator;
impl CoretimeInterface for CoretimeAllocator {
	type AccountId = AccountId;
	type Balance = Balance;
	type RelayChainBlockNumberProvider = RelaychainDataProvider<Runtime>;

	fn request_core_count(count: CoreIndex) {
		use crate::coretime::CoretimeProviderCalls::RequestCoreCount;
		let request_core_count_call = RelayRuntimePallets::Coretime(RequestCoreCount(count));

		// Weight for `request_core_count` from Kusama runtime benchmarks:
		// `ref_time` = 7889000 + (3 * 25000000) + (1 * 100000000) = 182889000
		// `proof_size` = 1636
		// Add 5% to each component and round to 2 significant figures.
		let call_weight = Weight::from_parts(190_000_000, 1700);

		let message = Xcm(vec![
			Instruction::UnpaidExecution {
				weight_limit: WeightLimit::Unlimited,
				check_origin: None,
			},
			Instruction::Transact {
				origin_kind: OriginKind::Native,
				require_weight_at_most: call_weight,
				call: request_core_count_call.encode().into(),
			},
		]);

		match PolkadotXcm::send_xcm(Here, Location::parent(), message) {
			Ok(_) => log::debug!(
				target: "runtime::coretime",
				"Request to update schedulable cores sent successfully."
			),
			Err(e) => log::error!(
				target: "runtime::coretime",
				"Failed to send request to update schedulable cores: {:?}",
				e
			),
		}
	}

	fn request_revenue_info_at(when: RCBlockNumberOf<Self>) {
		use crate::coretime::CoretimeProviderCalls::RequestRevenueInfoAt;
		let request_revenue_info_at_call =
			RelayRuntimePallets::Coretime(RequestRevenueInfoAt(when));

		// Weight for `request_revenue_at` from Kusama runtime benchmarks:
		// `ref_time` = 37_637_000 + (3 * 25000000) + (6 * 100000000) = 712637000
		// `proof_size` = 6428
		// Add 5% to each component and round to 2 significant figures.
		//
		// This benchmark has been transplanted from a testnet and not rerun, so adding a healthy
		// buffer. TODO refine when benchmarks are run: https://github.com/polkadot-fellows/runtimes/issues/404
		let call_weight = Weight::from_parts(1_000_000_000, 20_000);

		let message = Xcm(vec![
			Instruction::UnpaidExecution {
				weight_limit: WeightLimit::Unlimited,
				check_origin: None,
			},
			Instruction::Transact {
				origin_kind: OriginKind::Native,
				require_weight_at_most: call_weight,
				call: request_revenue_info_at_call.encode().into(),
			},
		]);

		match PolkadotXcm::send_xcm(Here, Location::parent(), message) {
			Ok(_) => log::debug!(
				target: "runtime::coretime",
				"Revenue info request sent successfully."
			),
			Err(e) => log::error!(
				target: "runtime::coretime",
				"Request for revenue info failed to send: {:?}",
				e
			),
		}
	}

	fn credit_account(who: Self::AccountId, amount: Self::Balance) {
		use crate::coretime::CoretimeProviderCalls::CreditAccount;
		let _credit_account_call = RelayRuntimePallets::Coretime(CreditAccount(who, amount));

		log::debug!(
			target: "runtime::coretime",
			"`credit_account` is unimplemented on the relay."
		);
	}

	fn assign_core(
		core: CoreIndex,
		begin: RCBlockNumberOf<Self>,
		assignment: Vec<(CoreAssignment, PartsOf57600)>,
		end_hint: Option<RCBlockNumberOf<Self>>,
	) {
		use crate::coretime::CoretimeProviderCalls::AssignCore;
		let assign_core_call =
			RelayRuntimePallets::Coretime(AssignCore(core, begin, assignment, end_hint));

		// Weight for `assign_core` from Kusama runtime benchmarks:
		// `ref_time` = 10177115 + (1 * 25000000) + (2 * 100000000) + (80 * 13932) = 236291675
		// `proof_size` = 3612
		// Add 5% to each component and round to 2 significant figures.
		let call_weight = Weight::from_parts(248_000_000, 3800);

		let message = Xcm(vec![
			Instruction::UnpaidExecution {
				weight_limit: WeightLimit::Unlimited,
				check_origin: None,
			},
			Instruction::Transact {
				origin_kind: OriginKind::Native,
				require_weight_at_most: call_weight,
				call: assign_core_call.encode().into(),
			},
		]);

		match PolkadotXcm::send_xcm(Here, Location::parent(), message) {
			Ok(_) => log::debug!(
				target: "runtime::coretime",
				"Core assignment sent successfully."
			),
			Err(e) => log::error!(
				target: "runtime::coretime",
				"Core assignment failed to send: {:?}",
				e
			),
		}
	}

	fn on_new_timeslice(t: pallet_broker::Timeslice) {
		// Burn roughly once per day. TIMESLICE_PERIOD tested to be != 0.
		const BURN_PERIOD: pallet_broker::Timeslice =
			RELAY_DAYS.saturating_div(coretime::TIMESLICE_PERIOD);
		// If checked_rem returns `None`, `TIMESLICE_PERIOD` is misconfigured for some reason. We
		// have bigger issues with the chain, but we still want to burn.
		if t.checked_rem(BURN_PERIOD).map_or(false, |r| r != 0) {
			return;
		}

		let stash = CoretimeBurnAccount::get();
		let value =
			Balances::reducible_balance(&stash, Preservation::Expendable, Fortitude::Polite);

		if value > 0 {
			log::debug!(target: "runtime::coretime", "Going to burn {value} stashed tokens at RC");
			match burn_at_relay(&stash, value) {
				Ok(()) => {
					log::debug!(target: "runtime::coretime", "Succesfully burnt {value} tokens");
				},
				Err(err) => {
					log::error!(target: "runtime::coretime", "burn_at_relay failed: {err:?}");
				},
			}
		}
	}
}

parameter_types! {
	pub const BrokerPalletId: PalletId = PalletId(*b"py/broke");
}

impl pallet_broker::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Currency = Balances;
	type OnRevenue = BurnCoretimeRevenue;
	type TimeslicePeriod = ConstU32<{ coretime::TIMESLICE_PERIOD }>;
	type MaxLeasedCores = ConstU32<50>;
	type MaxReservedCores = ConstU32<10>;
	type Coretime = CoretimeAllocator;
	type ConvertBalance = sp_runtime::traits::Identity;
	type WeightInfo = weights::pallet_broker::WeightInfo<Runtime>;
	type PalletId = BrokerPalletId;
	type AdminOrigin = EnsureRoot<AccountId>;
	type PriceAdapter = pallet_broker::CenterTargetPrice<Balance>;
}
