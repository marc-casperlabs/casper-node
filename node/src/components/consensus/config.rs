use std::path::PathBuf;

use datasize::DataSize;
use serde::{Deserialize, Serialize};

use casper_types::SecretKey;

use crate::{
    components::chainspec_loader::{HighwayConfig, UpgradePoint},
    types::{TimeDiff, Timestamp},
    utils::External,
    Chainspec,
};

/// Consensus configuration.
#[derive(DataSize, Debug, Deserialize, Serialize, Clone)]
// Disallow unknown fields to ensure config files and command-line overrides contain valid keys.
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Path to secret key file.
    pub secret_key_path: External<SecretKey>,
    /// Path to the folder where unit hash files will be stored.
    pub unit_hashes_folder: PathBuf,
    /// The duration for which incoming vertices with missing dependencies are kept in a queue.
    pub pending_vertex_timeout: TimeDiff,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            secret_key_path: External::Missing,
            unit_hashes_folder: Default::default(),
            pending_vertex_timeout: "10sec".parse().unwrap(),
        }
    }
}

/// Consensus protocol configuration.
#[derive(DataSize, Debug)]
pub(crate) struct ProtocolConfig {
    pub(crate) highway_config: HighwayConfig,
    /// Number of eras before an auction actually defines the set of validators.
    /// If you bond with a sufficient bid in era N, you will be a validator in era N +
    /// auction_delay + 1
    pub(crate) auction_delay: u64,
    pub(crate) unbonding_delay: u64,
    /// Name of the network.
    pub(crate) name: String,
    /// Genesis timestamp.
    pub(crate) timestamp: Timestamp,
    pub(crate) upgrades: Vec<UpgradePoint>,
}

impl From<&Chainspec> for ProtocolConfig {
    fn from(c: &Chainspec) -> Self {
        ProtocolConfig {
            highway_config: c.genesis.highway_config.clone(),
            auction_delay: c.genesis.auction_delay,
            unbonding_delay: c.genesis.unbonding_delay,
            name: c.genesis.name.clone(),
            timestamp: c.genesis.timestamp,
            upgrades: c.upgrades.clone(),
        }
    }
}
