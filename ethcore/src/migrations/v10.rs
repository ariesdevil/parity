// Copyright 2015-2017 Parity Technologies (UK) Ltd.
// This file is part of Parity.

// Parity is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity.  If not, see <http://www.gnu.org/licenses/>.

//! Bloom upgrade

use std::sync::Arc;
use db::{COL_EXTRA, COL_HEADERS, COL_STATE};
use state_db::{ACCOUNT_BLOOM_SPACE, DEFAULT_ACCOUNT_PRESET, StateDB};
use util::trie::TrieDB;
use views::HeaderView;
use bloom_journal::Bloom;
use util::migration::{Error, Migration, Progress, Batch, Config};
use util::journaldb;
use util::{H256, Trie};
use util::{Database, DBTransaction};

/// Account bloom upgrade routine. If bloom already present, does nothing.
/// If database empty (no best block), does nothing.
/// Can be called on upgraded database with no issues (will do nothing).
pub fn generate_bloom(source: Arc<Database>, dest: &mut Database) -> Result<(), Error> {
	trace!(target: "migration", "Account bloom upgrade started");
	let best_block_hash = match source.get(COL_EXTRA, b"best")? {
		// no migration needed
		None => {
			trace!(target: "migration", "No best block hash, skipping");
			return Ok(());
		},
		Some(hash) => hash,
	};
	let best_block_header = match source.get(COL_HEADERS, &best_block_hash)? {
		// no best block, nothing to do
		None => {
			trace!(target: "migration", "No best block header, skipping");
			return Ok(())
		},
		Some(x) => x,
	};
	let state_root = HeaderView::new(&best_block_header).state_root();

	trace!("Adding accounts bloom (one-time upgrade)");
	let bloom_journal = {
		let mut bloom = Bloom::new(ACCOUNT_BLOOM_SPACE, DEFAULT_ACCOUNT_PRESET);
		// no difference what algorithm is passed, since there will be no writes
		let state_db = journaldb::new(
			source.clone(),
			journaldb::Algorithm::OverlayRecent,
			COL_STATE);
		let account_trie = TrieDB::new(state_db.as_hashdb(), &state_root).map_err(|e| Error::Custom(format!("Cannot open trie: {:?}", e)))?;
		for item in account_trie.iter().map_err(|_| Error::MigrationImpossible)? {
			let (ref account_key, _) = item.map_err(|_| Error::MigrationImpossible)?;
			let account_key_hash = H256::from_slice(account_key);
			bloom.set(&*account_key_hash);
		}

		bloom.drain_journal()
	};

	trace!(target: "migration", "Generated {} bloom updates", bloom_journal.entries.len());

	let mut batch = DBTransaction::new();
	StateDB::commit_bloom(&mut batch, bloom_journal).map_err(|_| Error::Custom("Failed to commit bloom".to_owned()))?;
	dest.write(batch)?;

	trace!(target: "migration", "Finished bloom update");


	Ok(())
}

/// Account bloom migration.
#[derive(Default)]
pub struct ToV10 {
	progress: Progress,
}

impl ToV10 {
	/// New v10 migration
	pub fn new() -> ToV10 { ToV10 { progress: Progress::default() } }
}

impl Migration for ToV10 {
	fn version(&self) -> u32 {
		10
	}

	fn pre_columns(&self) -> Option<u32> { Some(5) }

	fn columns(&self) -> Option<u32> { Some(6) }

	fn migrate(&mut self, source: Arc<Database>, config: &Config, dest: &mut Database, col: Option<u32>) -> Result<(), Error> {
		let mut batch = Batch::new(config, col);
		for (key, value) in source.iter(col).into_iter().flat_map(|inner| inner) {
			self.progress.tick();
			batch.insert(key.into_vec(), value.into_vec(), dest)?;
		}
		batch.commit(dest)?;

		if col == COL_STATE {
			generate_bloom(source, dest)?;
		}

		Ok(())
	}
}
