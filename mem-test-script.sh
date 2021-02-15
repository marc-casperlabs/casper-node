#!/bin/sh

# http://localhost:9090/graph?g0.expr=os_mem_rss_bytes&g0.tab=0&g0.stacked=0&g0.range_input=30m&g1.expr=net_direct_message_requests&g1.tab=0&g1.stacked=0&g1.range_input=30m&g2.expr=%20sum(os_mem_rss_bytes)%20-%20sum(mem_consensus)%20&g2.tab=0&g2.stacked=1&g2.range_input=30m

set -e

NODE_COUNT=30

export CASPER_ENABLE_LIBP2P_NET=1

nctl assets-setup nodes=${NODE_COUNT} users=${NODE_COUNT}
nctl start node=all loglevel=info

cd utils/nctl-metrics
exec supervisord
