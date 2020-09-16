#!/bin/sh

set -eu

HOSTNAME=$(hostname)
IP=$(hostname -I)
SOURCE=https://github.com/CasperLabs/casper-node

echo "* Running on ${HOSTNAME} ($IP)"

echo "* Installing prerequisites via apt"
apt-get -qq update
apt-get -qq dist-upgrade
apt-get -qq install git make curl gcc build-essential libssl-dev pkg-config file snapd
# Just to be sure add snaps to path.
export PATH=$PATH:/snap/bin

echo "* Installing more recent CMake version via snap"
snap install cmake --classic

echo "* Installing rustup"
if [ ! -e $HOME/.rustup ]; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs > /tmp/rustup
  sh /tmp/rustup -y --default-toolchain none
fi
export PATH=$HOME/.cargo/bin:$PATH

if [ ! -e /src ]; then
echo "* Cloning source"
  rm -rf /src
  mkdir /src
  git clone --recursive --shallow-submodules --depth 1 ${SOURCE} /src/casper-node
fi

cd /src/casper-node

echo "* Setting up"
make setup-rs

echo "* Building system contracts"
make build-system-contracts -j

echo "* Compiling a release node"
cargo build -p casper-node --release

echo "* Copying built node to /usr/local/bin"
# stop service in case it is running
systemctl stop casper-node || true
sleep 0.5
cp -v target/release/casper-node /usr/local/bin

echo "* Copying over system contracts"
mkdir -p /etc/casper-node
cp -v target/wasm32-unknown-unknown/release/mint_install.wasm /etc/casper-node/
cp -v target/wasm32-unknown-unknown/release/pos_install.wasm /etc/casper-node/
cp -v target/wasm32-unknown-unknown/release/standard_payment_install.wasm /etc/casper-node/
cp -v target/wasm32-unknown-unknown/release/auction_install.wasm /etc/casper-node/

echo "* Setting up the casper user account"
adduser --disabled-login --group --no-create-home --system casper
mkdir -p /var/lib/casper-node/storage
chown casper:casper -R /var/lib/casper-node
chown casper:casper -R /etc/casper-node
chmod -R g=,o= /var/lib/casper-node /etc/casper-node

echo "* Creating systemd unit file"
cat > /etc/systemd/system/casper-node.service <<EOF
[Unit]
Description=CasperLabs blockchain node
Documentation=https://github.com/casperlabs/casper-node
After=network-online.target
Requires=network-online.target

[Service]
Environment=RUST_LOG=debug
ExecStart=/usr/local/bin/casper-node validator /etc/casper-node/config.toml
# RestartSec=5
# Restart=on-failure
Restart=never
# FIXME: Once #218 is merged, use notify.
Type=simple
User=casper
Group=casper


[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload

echo "Node set up successfully. You can now provision it."
