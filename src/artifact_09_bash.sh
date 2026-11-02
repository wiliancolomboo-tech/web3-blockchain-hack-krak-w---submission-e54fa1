# 1. Clone repository
git clone https://github.com/aegis-krakow/protocol.git
cd protocol

# 2. Build Anchor Smart Contract
anchor build

# 3. Synchronize Program Keys
solana-keygen new --no-bip39-passphrase -o target/deploy/aegis_escrow-keypair.json --force
anchor keys sync

# 4. Deploy to Devnet / Mainnet
solana config set --url devnet
anchor deploy --provider.cluster devnet

# 5. Launch Enterprise Notification Daemon via Docker
cp .env.example .env
# Edit .env with your SLACK_WEBHOOK_URL and RESEND_API_KEY
docker-compose up -d --build

# 6. Verify Container Status
docker-compose logs -f aegis-daemon