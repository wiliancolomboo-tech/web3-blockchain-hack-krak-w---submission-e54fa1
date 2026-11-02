#!/usr/bin/env bash
set -euo pipefail

echo "==> [1/4] Building Solana Anchor Engine..."
anchor build

echo "==> [2/4] Deploying program to Solana Devnet..."
solana program deploy \
  --program-id target/deploy/krak_stream-keypair.json \
  target/deploy/krak_stream.so \
  --url devnet

echo "==> [3/4] Exporting IDL to Client..."
cp target/idl/krak_stream.json packages/daemon/src/idl.json

echo "==> [4/4] Starting Docker Telemetry Container..."
docker compose up -d --build

echo "✅ KrakStream Protocol successfully deployed and online!"