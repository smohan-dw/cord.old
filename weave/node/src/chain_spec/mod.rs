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

use sc_chain_spec::{ChainSpecExtension, ChainSpecGroup};
use serde::{Deserialize, Serialize};

pub mod asset_hub;
pub mod coretime;

/// Generic extensions for Parachain ChainSpecs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ChainSpecGroup, ChainSpecExtension)]
pub struct Extensions {
	/// The relay chain of the Parachain.
	#[serde(alias = "relayChain", alias = "RelayChain")]
	pub relay_chain: String,
	/// The id of the Parachain.
	#[serde(alias = "paraId", alias = "ParaId")]
	pub para_id: u32,
}

impl Extensions {
	/// Try to get the extension from the given `ChainSpec`.
	pub fn try_get(chain_spec: &dyn sc_service::ChainSpec) -> Option<&Self> {
		sc_chain_spec::get_extension(chain_spec.extensions())
	}
}

/// Generic chain spec for all polkadot-parachain runtimes
pub type GenericChainSpec = sc_service::GenericChainSpec<Extensions>;

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn can_decode_extension_camel_and_snake_case() {
		let camel_case = r#"{"relayChain":"relay","paraId":1}"#;
		let snake_case = r#"{"relay_chain":"relay","para_id":1}"#;
		let pascal_case = r#"{"RelayChain":"relay","ParaId":1}"#;

		let camel_case_extension: Extensions = serde_json::from_str(camel_case).unwrap();
		let snake_case_extension: Extensions = serde_json::from_str(snake_case).unwrap();
		let pascal_case_extension: Extensions = serde_json::from_str(pascal_case).unwrap();

		assert_eq!(camel_case_extension, snake_case_extension);
		assert_eq!(snake_case_extension, pascal_case_extension);
	}
}
