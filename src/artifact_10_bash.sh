# 1. Clone repository and install dependencies
git clone https://github.com/aegis-settlement/aegis-solana.git
cd aegis-solana
yarn install

# 2. Build Anchor Smart Contracts
anchor build

# 3. Synchronize program keys
solana address -k target/deploy/aegis_settlement-keypair.json
# (Ensure declare_id matches in programs/aegis-settlement/src/lib.rs)

# 4. Run automated test suite against local validator
anchor test

# 5. Deploy to Solana Devnet
solana config set --url devnet
anchor deploy --provider.cluster devnet

# 6. Initialize Enterprise Environment & Run Docker Container
cp .env.example .env
# Fill in RESEND_API_KEY and SLACK_WEBHOOK_URL
docker-compose up -d --build

# 7. Verify Health & Logs
docker logs -f aegis-settlement-worker