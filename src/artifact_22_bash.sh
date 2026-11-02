# Install Solana CLI & Anchor 0.30+
sh -c "$(curl -sSfL https://release.solana.com/v1.18.18/install)"
cargo install --git https://github.com/coral-xyz/anchor --tag v0.30.1 anchor-cli --locked

# Build Program Artifacts
anchor build