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

use crate::{
	coretime::{BrokerPalletId, CoretimeBurnAccount},
	*,
};
use cord_loom_runtime_constants::system_parachain::coretime::TIMESLICE_PERIOD;
use coretime::CoretimeAllocator;
use frame_support::{
	assert_ok,
	traits::{
		fungible::{Inspect, Mutate},
		Get, OnInitialize,
	},
};
use pallet_broker::{ConfigRecordOf, RCBlockNumberOf, SaleInfo};
use parachains_runtimes_test_utils::ExtBuilder;
use sp_runtime::traits::AccountIdConversion;

fn advance_to(b: BlockNumber) {
	while System::block_number() < b {
		let block_number = System::block_number() + 1;
		System::set_block_number(block_number);
		Broker::on_initialize(block_number);
	}
}

#[test]
fn bulk_revenue_is_burnt() {
	const ALICE: [u8; 32] = [1u8; 32];

	ExtBuilder::<Runtime>::default()
		.with_collators(vec![AccountId::from(ALICE)])
		.with_session_keys(vec![(
			AccountId::from(ALICE),
			AccountId::from(ALICE),
			SessionKeys { aura: AuraId::from(sp_core::sr25519::Public::from_raw(ALICE)) },
		)])
		.build()
		.execute_with(|| {
			// Configure broker and start sales
			let config = ConfigRecordOf::<Runtime> {
				advance_notice: 1,
				interlude_length: 1,
				leadin_length: 2,
				region_length: 1,
				ideal_bulk_proportion: Perbill::from_percent(100),
				limit_cores_offered: None,
				renewal_bump: Perbill::from_percent(3),
				contribution_timeout: 1,
			};
			let ed = ExistentialDeposit::get();
			assert_ok!(Broker::configure(RuntimeOrigin::root(), config.clone()));
			assert_ok!(Broker::start_sales(RuntimeOrigin::root(), ed, 1));

			let sale_start = SaleInfo::<Runtime>::get().unwrap().sale_start;
			advance_to(sale_start + config.interlude_length);

			// Check and set initial balances.
			let broker_account = BrokerPalletId::get().into_account_truncating();
			let coretime_burn_account = CoretimeBurnAccount::get();
			let treasury_account = xcm_config::RelayTreasuryPalletAccount::get();
			assert_ok!(Balances::mint_into(&AccountId::from(ALICE), 200 * ed));
			let alice_balance_before = Balances::balance(&AccountId::from(ALICE));
			let treasury_balance_before = Balances::balance(&treasury_account);
			let broker_balance_before = Balances::balance(&broker_account);
			let burn_balance_before = Balances::balance(&coretime_burn_account);

			// Purchase coretime.
			assert_ok!(Broker::purchase(RuntimeOrigin::signed(AccountId::from(ALICE)), 100 * ed));

			// Alice decreases.
			assert!(Balances::balance(&AccountId::from(ALICE)) < alice_balance_before);
			// Treasury balance does not increase.
			assert_eq!(Balances::balance(&treasury_account), treasury_balance_before);
			// Broker pallet account does not increase.
			assert_eq!(Balances::balance(&broker_account), broker_balance_before);
			// Coretime burn pot gets the funds.
			assert!(Balances::balance(&coretime_burn_account) > burn_balance_before);

			// They're burnt when a day has passed on chain.
			// This needs to be asserted in an emulated test.
		});
}

#[test]
fn timeslice_period_is_sane() {
	// Config TimeslicePeriod is set to this constant - assumption in burning logic.
	let timeslice_period_config: RCBlockNumberOf<CoretimeAllocator> =
		<Runtime as pallet_broker::Config>::TimeslicePeriod::get();
	assert_eq!(timeslice_period_config, TIMESLICE_PERIOD);

	// Timeslice period constant non-zero - assumption in burning logic.
	#[cfg(feature = "fast-runtime")]
	assert_eq!(TIMESLICE_PERIOD, 20);
	#[cfg(not(feature = "fast-runtime"))]
	assert_eq!(TIMESLICE_PERIOD, 80);
}
