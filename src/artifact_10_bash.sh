# Switch to Solana Devnet
solana config set --url devnet

# Airdrop testnet SOL
solana airdrop 2

# Deploy program
anchor deploy --provider.cluster devnet