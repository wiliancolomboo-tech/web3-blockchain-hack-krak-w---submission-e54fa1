# Step 1: Build the Anchor program binary
anchor build

# Step 2: Deploy to Solana Devnet
solana program deploy \
  --program-id target/deploy/vistula_escrow-keypair.json \
  target/deploy/vistula_escrow.so \
  --url devnet

# Step 3: Run comprehensive verification test suite
anchor test --skip-deploy

# Step 4: Launch containerized enterprise listener daemon
docker compose up -d --build

# Step 5: Verify real-time logs and healthcheck
docker compose logs -f vistula-worker