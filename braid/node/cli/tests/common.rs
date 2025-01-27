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
#[allow(unused_imports)]
use cord_primitives::{Hash, Header};
use std::{
	io::{BufRead, BufReader, Read},
	ops::{Deref, DerefMut},
	path::{Path, PathBuf},
	process::{self, Child, Command},
	time::Duration,
};
use substrate_rpc_client::{ws_client, ChainApi};

use assert_cmd::cargo::cargo_bin;
use nix::{
	sys::signal::{kill, Signal, Signal::SIGINT},
	unistd::Pid,
};
use regex::Regex;

/// Read the WS address from the output.
///
/// This is hack to get the actual bound sockaddr because
/// polkadot assigns a random port if the specified port was already bound.
///
/// You must call
/// `Command::new("cmd").stdout(process::Stdio::piped()).stderr(process::Stdio::piped())`
/// for this to work.
#[allow(dead_code)]
pub fn find_ws_url_from_output(read: impl Read + Send) -> (String, String) {
	let mut data = String::new();

	let ws_url = BufReader::new(read)
		.lines()
		.find_map(|line| {
			let line = line.expect("failed to obtain next line from stdout for port discovery");

			data.push_str(&line);

			// does the line contain our port (we expect this specific output from substrate).
			let sock_addr = match line.split_once("Running JSON-RPC server: addr=") {
				None => return None,
				Some((_, after)) => after.split_once(',').unwrap().0,
			};

			Some(format!("ws://{}", sock_addr))
		})
		.unwrap_or_else(|| panic!("Could not find address in process output:\n{}", &data));
	(ws_url, data)
}

/// Run the given `future` and panic if the `timeout` is hit.
pub async fn run_with_timeout(timeout: Duration, future: impl futures::Future<Output = ()>) {
	tokio::time::timeout(timeout, future).await.expect("Hit timeout");
}

/// Wait for at least n blocks to be finalized from a specified node
#[allow(dead_code)]
pub async fn wait_n_finalized_blocks(n: usize, url: &str) {
	// use substrate_rpc_client::{ws_client, ChainApi};

	let mut built_blocks = std::collections::HashSet::new();
	let block_duration = Duration::from_secs(2);
	let mut interval = tokio::time::interval(block_duration);
	let rpc = ws_client(url).await.unwrap();

	loop {
		if let Ok(block) = ChainApi::<(), Hash, Header, ()>::finalized_head(&rpc).await {
			built_blocks.insert(block);
			if built_blocks.len() > n {
				break;
			}
		};
		interval.tick().await;
	}
}

/// Run the node for a while (3 blocks)
#[allow(dead_code)]
pub async fn run_node_for_a_while(base_path: &Path, args: &[&str]) {
	run_with_timeout(Duration::from_secs(60 * 10), async move {
		let mut cmd = Command::new(cargo_bin("cord"))
			.stdout(process::Stdio::piped())
			.stderr(process::Stdio::piped())
			.args(args)
			.arg("-d")
			.arg(base_path)
			.spawn()
			.unwrap();

		let stderr = cmd.stderr.take().unwrap();

		let mut child = KillChildOnDrop(cmd);

		let ws_url = extract_info_from_output(stderr).0.ws_url;

		// Let it produce some blocks.
		wait_n_finalized_blocks(3, &ws_url).await;

		child.assert_still_running();

		// Stop the process
		child.stop();
	})
	.await
}

pub struct KillChildOnDrop(pub Child);

impl KillChildOnDrop {
	/// Stop the child and wait until it is finished.
	///
	/// Asserts if the exit status isn't success.
	pub fn stop(&mut self) {
		self.stop_with_signal(SIGINT);
	}

	/// Same as [`Self::stop`] but takes the `signal` that is sent to stop the
	/// child.
	pub fn stop_with_signal(&mut self, signal: Signal) {
		kill(Pid::from_raw(self.id().try_into().unwrap()), signal).unwrap();
		assert!(self.wait().unwrap().success());
	}

	/// Asserts that the child is still running.
	pub fn assert_still_running(&mut self) {
		assert!(self.try_wait().unwrap().is_none(), "the process should still be running");
	}
}

impl Drop for KillChildOnDrop {
	fn drop(&mut self) {
		let _ = self.0.kill();
	}
}

impl Deref for KillChildOnDrop {
	type Target = Child;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

impl DerefMut for KillChildOnDrop {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.0
	}
}

/// Information extracted from a running node.
#[allow(dead_code)]
pub struct NodeInfo {
	pub ws_url: String,
	pub db_path: PathBuf,
}

/// Extract [`NodeInfo`] from a running node by parsing its output.
///
/// Returns the [`NodeInfo`] and all the read data.
#[allow(dead_code)]
pub fn extract_info_from_output(read: impl Read + Send) -> (NodeInfo, String) {
	let mut data = String::new();

	let ws_url = BufReader::new(read)
		.lines()
		.find_map(|line| {
			let line = line.expect("failed to obtain next line while extracting node info");
			data.push_str(&line);
			data.push_str("\n");

			// does the line contain our port (we expect this specific output from
			// substrate).
			let sock_addr = match line.split_once("Running JSON-RPC server: addr=") {
				None => return None,
				Some((_, after)) => after.split_once(',').unwrap().0,
			};

			Some(format!("ws://{}", sock_addr))
		})
		.unwrap_or_else(|| {
			eprintln!("Observed node output:\n{}", data);
			panic!("We should get a WebSocket address")
		});

	// Database path is printed before the ws url!
	let re = Regex::new(r"Database: .+ at (\S+)").unwrap();
	let db_path = PathBuf::from(re.captures(data.as_str()).unwrap().get(1).unwrap().as_str());

	(NodeInfo { ws_url, db_path }, data)
}

#[allow(dead_code)]
pub fn start_node() -> Child {
	Command::new(cargo_bin("cord"))
		.stdout(process::Stdio::piped())
		.stderr(process::Stdio::piped())
		.args(&["--dev", "--tmp", "--rpc-port=45789", "--no-hardware-benchmarks"])
		.spawn()
		.unwrap()
}

#[allow(dead_code)]
pub fn start_node_inline(args: Vec<&str>) -> Result<(), sc_service::error::Error> {
	use sc_cli::SubstrateCli;

	// Prepend the args with some dummy value because the first arg is skipped.
	let cli_call = std::iter::once("cord").chain(args);
	let cli = cord_braid_node_cli::Cli::from_iter(cli_call);
	let runner = cli.create_runner(&cli.run).unwrap();
	runner.run_node_until_exit(|config| async move {
		cord_braid_node_cli::service::new_full(config, cli)
	})
}
