#!/bin/sh
# Runs the TPROXY tests of the `it` binary given as $1, as root on Linux.
#
# The transparent tests need an upstream on another host, because Linux does not send a packet
# with a foreign source address to a loopback address. The network namespace `up` plays that
# host: it is reached over a veth pair, and its servers answer with the address they see.
#   root namespace: dummy0 10.66.0.1 (the port) and 10.66.0.5 (the client), veth0 10.77.0.1
#   namespace up:   veth1 10.77.0.2, UDP 5000 and TCP 5001
set -eu
apt-get update -qq >/dev/null
apt-get install -y -qq nftables iproute2 socat >/dev/null 2>&1

ip link add dummy0 type dummy
ip addr add 10.66.0.1/24 dev dummy0
ip addr add 10.66.0.5/24 dev dummy0
ip link set dummy0 up
ip netns add up
ip link add veth0 type veth peer name veth1
ip link set veth1 netns up
ip addr add 10.77.0.1/24 dev veth0
ip link set veth0 up
ip -n up addr add 10.77.0.2/24 dev veth1
ip -n up link set veth1 up
ip -n up link set lo up
ip -n up route add default via 10.77.0.1

ip netns exec up socat UDP4-RECVFROM:5000,bind=10.77.0.2,fork SYSTEM:'printf %s "$SOCAT_PEERADDR"' &
ip netns exec up socat TCP4-LISTEN:5001,bind=10.77.0.2,fork,reuseaddr SYSTEM:'printf %s "$SOCAT_PEERADDR"' &
sleep 1

"$1" --ignored --test-threads 1 tproxy_test::
