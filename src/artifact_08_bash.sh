# 1. Clone & install dependencies
git clone https://github.com/streamshield/streamshield-krakow.git
cd streamshield-krakow
npm install

# 2. Build on-chain program
anchor build

# 3. Deploy to Solana Devnet
solana config set --url https://api.devnet.solana.com
anchor deploy --provider.cluster devnet

# 4. Run the comprehensive integration test suite
anchor test

# 5. Boot production daemon with Docker
docker-compose up -d --build