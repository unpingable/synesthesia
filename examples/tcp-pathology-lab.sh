#!/usr/bin/env bash
#
# A contained, bounded TCP-pathology qualification lab for Synesthesia.
# Run in a second terminal while `synesthesia ebpf tcp` is visible:
#
#   sudo ./examples/tcp-pathology-lab.sh
#
# The script never touches a host interface or route. It creates two temporary
# network namespaces joined by one veth pair and removes both on every exit.

set -euo pipefail

if [[ ${EUID} -ne 0 ]]; then
  echo "run this lab explicitly as root: sudo ./examples/tcp-pathology-lab.sh" >&2
  exit 2
fi

for tool in ip tc iperf3 nc; do
  if ! command -v "${tool}" >/dev/null 2>&1; then
    echo "required local tool is unavailable: ${tool}" >&2
    exit 2
  fi
done

suffix=$$
client_ns="syn-tcp-c-${suffix}"
server_ns="syn-tcp-s-${suffix}"
client_if="syntc${suffix}"
server_if="synts${suffix}"
server_pid=

cleanup() {
  trap - EXIT INT TERM
  if [[ -n ${server_pid} ]]; then
    kill "${server_pid}" 2>/dev/null || true
    wait "${server_pid}" 2>/dev/null || true
  fi
  ip netns delete "${client_ns}" 2>/dev/null || true
  ip netns delete "${server_ns}" 2>/dev/null || true
  echo "cleaned namespaces ${client_ns} and ${server_ns}"
}
trap cleanup EXIT INT TERM

ip netns add "${client_ns}"
ip netns add "${server_ns}"
ip link add "${client_if}" type veth peer name "${server_if}"
ip link set "${client_if}" netns "${client_ns}"
ip link set "${server_if}" netns "${server_ns}"
ip -n "${client_ns}" address add 192.0.2.1/30 dev "${client_if}"
ip -n "${server_ns}" address add 192.0.2.2/30 dev "${server_if}"
ip -n "${client_ns}" link set lo up
ip -n "${server_ns}" link set lo up
ip -n "${client_ns}" link set "${client_if}" up
ip -n "${server_ns}" link set "${server_if}" up

run_transfer() {
  local seconds=$1
  ip netns exec "${server_ns}" iperf3 -s -1 >/dev/null 2>&1 &
  server_pid=$!
  sleep 0.25
  ip netns exec "${client_ns}" iperf3 \
    --client 192.0.2.2 --time "${seconds}" --parallel 1 >/dev/null
  wait "${server_pid}"
  server_pid=
}

echo "quiet: 10 seconds"
sleep 10

echo "healthy transfer: 4 seconds, no impairment"
run_transfer 4
sleep 3

echo "isolated loss: 1% on the private client veth"
ip netns exec "${client_ns}" tc qdisc replace dev "${client_if}" root netem loss 1%
run_transfer 4
sleep 3

echo "sustained impairment: 8% loss, bounded 6-second transfer"
ip netns exec "${client_ns}" tc qdisc replace dev "${client_if}" root netem loss 8%
run_transfer 6
sleep 3

echo "reset: connect to a deliberately closed port"
ip netns exec "${client_ns}" nc -z -w 1 192.0.2.2 65000 >/dev/null 2>&1 || true
sleep 3

echo "settle: remove impairment and observe for 10 seconds"
ip netns exec "${client_ns}" tc qdisc delete dev "${client_if}" root
sleep 10
