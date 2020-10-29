#!/bin/sh
#
# run-dev: A quick and dirty script to run a testing setup of local nodes.

set -eu

# Build the contracts
# make build-contracts-rs

# Build the node first, so that `sleep` in the loop has an effect.
# cargo build -p casper-node

BASEDIR=$(readlink -f $(dirname $0))
ACCOUNTS_CSV=/tmp/run-dev-accounts.csv
CHAINSPEC=$(mktemp -t chainspec_XXXXXXXX --suffix .toml)
TRUSTED_HASH="${TRUSTED_HASH:-}"

# Generate a genesis timestamp 30 seconds into the future, unless explicity given a different one.
TIMESTAMP=$(python3 -c 'from datetime import datetime, timedelta; print((datetime.utcnow() + timedelta(seconds=30)).isoformat("T") + "Z")')
TIMESTAMP=${GENESIS_TIMESTAMP:-$TIMESTAMP}

echo "GENESIS_TIMESTAMP=${TIMESTAMP}"

# Update the chainspec to use the current time as the genesis timestamp.
cp ${BASEDIR}/resources/local/chainspec.toml ${CHAINSPEC}
sed -i "s/^\([[:alnum:]_]*timestamp\) = .*/\1 = \"${TIMESTAMP}\"/" ${CHAINSPEC}
sed -i 's|\.\./\.\.|'"$BASEDIR"'|' ${CHAINSPEC}
sed -i 's|accounts\.csv|'"${ACCOUNTS_CSV}"'|' ${CHAINSPEC}

# If no nodes defined, start all.
NODES="${@:-1 2 3 4 5}"

# Setup a node's keys.
setup_node() {
    ID=$1
    KEY_DIR=/tmp/node-${ID}-keys

    if [ -e ${KEY_DIR} ]; then
        echo "already got a key for node ${ID}"
        return
    fi;

    mkdir -p ${KEY_DIR}/
    cargo run --quiet --manifest-path=client/Cargo.toml -- keygen ${KEY_DIR}
}

run_node() {
    ID=$1
    STORAGE_DIR=/tmp/node-${ID}-storage
    LOGFILE=/tmp/node-${ID}.log
    KEY_DIR=/tmp/node-${ID}-keys

    rm -rf ${STORAGE_DIR}
    rm -f ${LOGFILE}
    rm -f ${LOGFILE}.stderr
    mkdir -p ${STORAGE_DIR}

    if [ $1 -ne 1 ]
    then
        BIND_ADDRESS_ARG=--config-ext=network.bind_address='0.0.0.0:0'
        DEPS="--property=After=node-1.service --property=Requires=node-1.service"
    else
        BIND_ADDRESS_ARG=
        DEPS=
    fi

    if ! [ -z "$TRUSTED_HASH" ]
    then
        TRUSTED_HASH_ARG=--config-ext=node.trusted_hash="${TRUSTED_HASH}"
    else
        TRUSTED_HASH_ARG=
    fi

    echo "$TRUSTED_HASH_ARG"

    # We compile in a seperate step, since we use resource limits for the actual process.
    # cargo build --release

    # We run with a 10 minute timeout, to allow for compilation and loading.
    cargo build --release

    systemctl --user reset-failed node-$ID || true

    systemd-run \
        --user \
        --unit node-$ID \
        --description "Casper Dev Node ${ID}" \
        --no-block \
        --property=Type=notify \
        --property=TimeoutSec=600 \
        --property=WorkingDirectory=${BASEDIR} \
        $DEPS \
        --setenv=RUST_LOG=info \
        --property=StandardOutput=file:${LOGFILE} \
        --property=StandardError=file:${LOGFILE}.stderr \
        --property=LimitDATA=infinity \  # FIXME
        -- \
        cargo run --release -p casper-node \
        validator \
        resources/local/config.toml \
        --config-ext=network.systemd_support=true \
        --config-ext=consensus.secret_key_path=${KEY_DIR}/secret_key.pem \
        --config-ext=storage.path=${STORAGE_DIR} \
        --config-ext=network.gossip_interval=1000 \
        --config-ext=node.chainspec_config_path=${CHAINSPEC} \
        ${BIND_ADDRESS_ARG} \
        ${TRUSTED_HASH_ARG}

    echo "Started node $ID, logfile: ${LOGFILE}"
}

# Generates keys for all nodes
for i in $NODES; do
    setup_node $i
done;

# Regenerate accounts CSV
rm -f ${ACCOUNTS_CSV}
for ID in $NODES; do
   KEY_DIR=/tmp/node-${ID}-keys
   HEX_KEY=$(cat ${KEY_DIR}/public_key_hex)
   WEIGHT=10000000000000
   MOTES=1000000000000000
   echo "${HEX_KEY},${MOTES},${WEIGHT}" >> ${ACCOUNTS_CSV}
done;

# Setup config

for i in $NODES; do
    run_node $i
done;

echo "Test network starting."
echo
echo 'To stop all nodes, run `systemctl --user stop node-\*`'
