use crate::*;
use codec::{Decode, Encode};
use sp_runtime::DispatchResult;

/// An on-chain schema details written by a controller.
#[derive(Clone, Debug, Encode, Decode, PartialEq)]
pub struct SchemaDetails<T: Config> {
	/// Schema identifier.
	pub hash: HashOf<T>,
	/// \[OPTIONAL\] Storage ID (base32/52).
	pub cid: Option<IdentifierOf>,
	/// \[OPTIONAL\] Previous Storage ID (base32/52).
	pub parent_cid: Option<IdentifierOf>,
	/// The identity of the controller.
	pub controller: CordAccountOf<T>,
	/// Transaction block number
	pub block: BlockNumberOf<T>,
	/// The flag indicating schema type.
	pub permissioned: StatusOf,
	/// The flag indicating the status of the schema.
	pub revoked: StatusOf,
}

impl<T: Config> SchemaDetails<T> {
	pub fn is_valid(incoming: &IdentifierOf) -> DispatchResult {
		let cid_str = str::from_utf8(incoming).unwrap();
		let cid_details: Cid = cid_str.parse().map_err(|_err| Error::<T>::InvalidCidEncoding)?;
		ensure!(
			(cid_details.version() == Version::V1 || cid_details.version() == Version::V0),
			Error::<T>::InvalidCidVersion
		);
		Ok(())
	}

	pub fn schema_status(tx_schema: IdOf<T>, controller: CordAccountOf<T>) -> Result<(), Error<T>> {
		let schema_details = <Schemas<T>>::get(&tx_schema).ok_or(Error::<T>::SchemaNotFound)?;
		ensure!(!schema_details.revoked, Error::<T>::SchemaRevoked);
		if schema_details.permissioned {
			let delegates = <Delegations<T>>::take(&tx_schema);
			ensure!(
				(delegates.iter().find(|&delegate| *delegate == controller) == Some(&controller)),
				Error::<T>::UnauthorizedOperation
			);
		}
		Ok(())
	}
}

/// An on-chain commit details.
#[derive(Clone, Debug, Encode, Decode, PartialEq)]
pub struct SchemaCommit<T: Config> {
	/// schema hash.
	pub hash: HashOf<T>,
	/// schema storage ID
	pub cid: Option<IdentifierOf>,
	/// schema tx block number
	pub block: BlockNumberOf<T>,
	/// schema tx request type
	pub commit: SchemaCommitOf,
}

impl<T: Config> SchemaCommit<T> {
	pub fn store_tx(identifier: &IdOf<T>, tx_commit: SchemaCommit<T>) -> DispatchResult {
		let mut commit = <Commits<T>>::get(identifier).unwrap_or_default();
		commit.push(tx_commit);
		<Commits<T>>::insert(identifier, commit);
		Ok(())
	}
}

#[derive(Clone, Debug, Encode, Decode, PartialEq, Eq)]
pub enum SchemaCommitOf {
	Genesis,
	Update,
	Delegate,
	RevokeDelegation,
	Permission,
	StatusChange,
}