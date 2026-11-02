# 1. Clone repository & install dependencies
git clone https://github.com/sponsor-org/blockchain-hack-krakow-sentry.git
cd blockchain-hack-krakow-sentry
yarn install

# 2. Build on-chain program
anchor build

# 3. Synchronize Program ID
solana address -k target/deploy/krakow_sentry-keypair.json
# (Replace ID in declare_id! and Anchor.toml if deploying new keypair)

# 4. Deploy to Solana Devnet
solana config set --url https://api.devnet.solana.com
anchor deploy --provider.cluster Devnet

# 5. Run full end-to-end integration tests
anchor test