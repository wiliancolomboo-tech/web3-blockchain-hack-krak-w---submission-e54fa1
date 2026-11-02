# Set Solana CLI to Kraków Hackathon Devnet
solana config set --url https://api.devnet.solana.com

# Build optimized BPF bytecode
anchor build

# Sync Program ID into lib.rs and Anchor.toml
anchor keys sync

# Deploy to Devnet
anchor deploy --program-name kraksettle --provider.cluster devnet