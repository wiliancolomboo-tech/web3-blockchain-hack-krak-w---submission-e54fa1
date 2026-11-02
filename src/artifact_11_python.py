import asyncio
from solana.rpc.async_api import AsyncClient
from solders.pubkey import Pubkey
from anchorpy import Provider, Wallet, Idl, Program

RPC_URL = "https://api.devnet.solana.com"
PROGRAM_ID = Pubkey.from_string("Krak7YxTq8B7WbFwQ3v8kZgD9sY6J8P2h4uA5vN3mD1")

async def run_krak_keeper():
    client = AsyncClient(RPC_URL)
    print(f"[*] KrakEngine Autonomous Keeper Loop listening on {RPC_URL}...")
    
    while True:
        try:
            # Query all Bounty accounts in Active state needing settlement
            resp = await client.get_program_accounts(PROGRAM_ID)
            print(f"[+] Heartbeat: Synced {len(resp.value)} protocol state accounts.")
            await asyncio.sleep(10)
        except Exception as e:
            print(f"[!] Error in Keeper loop: {str(e)}")
            await asyncio.sleep(5)

if __name__ == "__main__":
    asyncio.run(run_krak_keeper())