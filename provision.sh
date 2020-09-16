#!/bin/sh

set -e

ACTION=$1

if [ -z $ACTION ]; then
  echo "usage: $0 [setup|provision|start|status|logs|ssh] NODE_ADDRS"
  exit 1
fi;
shift

CURRENT_KEY_INDEX=1
BOOTSTRAP_NODE=
while true; do
  if [ -z $1 ]; then
    break;
  fi;

  DEST=$1
  DESTS+="${DEST} "
  KEY_SOURCE=resources/local/secret_keys/node-${CURRENT_KEY_INDEX}.pem
  CURRENT_KEY_INDEX=$((${CURRENT_KEY_INDEX}+1))

  # Create secret key that we need.
  echo "Creating secret key for ${DEST} from ${KEY_SOURCE}"
  cp ${KEY_SOURCE} /tmp/${DEST}.pem
  shift;

  if [ -z ${BOOTSTRAP_NODE} ]; then
    BOOTSTRAP_NODE=${DEST}
    echo "Bootstrap node: ${BOOTSTRAP_NODE}"
  fi
done;

case $ACTION in
  setup)
    CMD="scp payload.sh root@{}:/tmp/payload.sh; ssh root@{} 'sh /tmp/payload.sh'"
    ;;
  provision)
    ACCOUNTS_CSV=$(pwd)/resources/local/accounts.csv
    CHAINSPEC_TOML=/tmp/chainspec.toml
    CONFIG_TOML=/tmp/config.toml

    echo "setting genesis timestamp to NOW + 5 minutes"
    TIMESTAMP=$(date --date '+5 min' '+%s000')

    # Prepare temporary chainspec and config by making a copy from the production chainspec.
    cp $(pwd)/resources/production/chainspec.toml ${CHAINSPEC_TOML}
    # There is no production config, we use local instead.
    cp $(pwd)/resources/local/config.toml ${CONFIG_TOML}

    # Lifted from `run-dev.sh`
    sed -i "s/^\([[:alnum:]_]*timestamp\) = .*/\1 = ${TIMESTAMP}/" ${CHAINSPEC_TOML}
    sed -i "s/^known_addresses = .*/known_addresses = ['${BOOTSTRAP_NODE}:34553']/" ${CONFIG_TOML}

    CMD="sed -i \"s/^public_address = .*/public_address = '{}:34553'/\" ${CONFIG_TOML}; scp ${ACCOUNTS_CSV} ${CHAINSPEC_TOML} ${CONFIG_TOML} root@{}:/etc/casper-node/; scp /tmp/{}.pem root@{}:/etc/casper-node/secret_key.pem"
    ;;
  start)
    CMD="if [ ! {} = ${BOOTSTRAP_NODE} ]; then echo not bootstrap, sleeping ...; sleep 5; fi; ssh root@{} \"systemctl start casper-node; journalctl -u casper-node -f\""
    ;;
  status)
    CMD="curl {}:7777/status | jq"
    ;;
  logs)
    CMD="ssh root@{} \"journalctl -u casper-node -f\""
    ;;
  ssh)
    CMD="ssh root@{}"
    ;;
  *)
    echo "invalid action ${ACTION}"
    exit 1
    ;;
esac

xpanes -c "${CMD}; echo -e exited, connect with 'ssh root@{}'" $DESTS
