# 1. Install Solana CLI & Anchor 0.30.1
solana config set --url https://api.devnet.solana.com

# 2. Generate Deployer Keypair (if not present)
solana-keygen new --no-passphrase -o ~/.config/solana/id.json

# 3. Request Devnet SOL
solana airdrop 2

# 4. Build program
anchor build

# 5. Extract Program ID and synchronize keys
solana address -k target/deploy/solwawel_protocol-keypair.json
# (Replace declare_id! in programs/solwawel-protocol/src/lib.rs with returned address)

# 6. Rebuild and deploy to Devnet
anchor build
anchor deploy --provider.cluster devnet