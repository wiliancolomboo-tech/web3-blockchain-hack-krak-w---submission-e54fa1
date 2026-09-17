# Set Solana CLI to local test validator
solana config set --url localhost

# Generate deployer keypair if not present
solana-keygen new --no-bip39-passphrase --force --outfile ~/.config/solana/id.json

# Check wallet address
solana address