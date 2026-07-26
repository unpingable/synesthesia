# Synesthesia

**Data in. Terminal weather out.**

Synesthesia turns live machine activity into a terminal-native visual instrument.

```sh
cargo run --release -- demo
```

Any line can become weather:

```sh
nc -lk 9000 | cargo run --release -- stdin --format lines
```

The network hook uses one exact field order:

```sh
sudo tshark -l -n -T fields \
  -e frame.time_epoch \
  -e ip.src -e ipv6.src -e ip.dst -e ipv6.dst \
  -e _ws.col.Protocol -e frame.len \
  -e tcp.srcport -e udp.srcport -e tcp.dstport -e udp.dstport \
  -E header=n -E separator=/t -E occurrence=f -E quote=n \
  | cargo run --release -- stdin --format tshark-tsv
```

That command emits exactly 11 tab-separated columns. The checked-in fixture is
parser-tested. This machine did not have `tshark` installed, so the invocation
has not yet been live-capture verified.

Wireshark is analysis. This is the hallucination layer.
