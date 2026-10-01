#!/bin/sh
# Run after the .deb or .rpm installs or upgrades the meter (see
# src-tauri/tauri.conf.json, bundle.linux). Packet capture needs CAP_NET_RAW;
# grant it to the installed binary so the meter never has to run as root. A
# new binary (every upgrade) needs it again. The Arch package does the same in
# packaging/arch/a2tools-dps-meter.install.

BIN=/usr/bin/a2tools-dps-meter

if setcap cap_net_raw,cap_net_admin=eip "$BIN" 2>/dev/null; then
  echo "A2Tools DPS Meter may now capture packets."
else
  echo "Could not grant packet-capture permission. Run:"
  echo "    sudo setcap cap_net_raw,cap_net_admin=eip $BIN"
fi

# Never fail the install over it: the meter runs, and says it cannot capture.
exit 0
