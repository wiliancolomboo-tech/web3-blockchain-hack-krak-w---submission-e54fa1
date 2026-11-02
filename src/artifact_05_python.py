#!/usr/bin/env python3
"""
KrakStream Autonomous Keeper Crank Bot
Scans for expired streams and issues crank settlement transactions to harvest keeper bounties.
"""

import asyncio
import struct
import time
import logging
from typing import List
from solana.rpc.async_api import AsyncClient
from solders.pubkey import Pubkey
from solders.keypair import Keypair
from solders.instruction import Instruction, AccountMeta
from solders.transaction import VersionedTransaction
from solders.message import MessageV0
from spl.token.instructions import get_associated_token_address

logging.basicConfig(level=logging.INFO, format="%(asctime)s [%(levelname)s] %(message)s")

PROGRAM_ID = Pubkey.from_string("Krak111111111111111111111111111111111111111")
TOKEN_PROGRAM_ID = Pubkey.from_string("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA")
STREAM_SEED = b"stream"
ESCROW_SEED = b"escrow"

# Sha256("global:keeper_crank_settle")[0:8] Anchor discriminator
CRANK_DISCRIMINATOR = bytes([163, 114, 185, 222, 102, 15, 33, 91])

async def fetch_expired_streams(client: AsyncClient, current_timestamp: int):
    """
    Fetches all Program Accounts matching StreamState size, filtering on-chain for end_time <= current_time
    """
    # Offset 152 corresponds to end_time i64 in StreamState struct layout
    filters = [{"dataSize": 8 + 194}]
    resp = await client.get_program_accounts(PROGRAM_ID, encoding="base64", filters=filters)
    expired_streams = []

    for item in resp.value:
        pubkey = item.pubkey
        raw_data = item.account.data
        # Parse fields from raw data:
        # 0..8: Discriminator
        # 8..40: sender (32 bytes)
        # 40..72: recipient (32 bytes)
        # 72..104: mint (32 bytes)
        # 104..136: escrow_vault (32 bytes)
        # 136..144: stream_id (u64)
        # 144..152: deposit_amount (u64)
        # 152..160: withdrawn_amount (u64)
        # 160..168: start_time (i64)
        # 168..176: end_time (i64)
        
        sender = Pubkey.from_bytes(raw_data[8:40])
        recipient = Pubkey.from_bytes(raw_data[40:72])
        mint = Pubkey.from_bytes(raw_data[72:104])
        escrow_vault = Pubkey.from_bytes(raw_data[104:136])
        deposit_amount = struct.unpack("<Q", raw_data[144:152])[0]
        withdrawn_amount = struct.unpack("<Q", raw_data[152:160])[0]
        end_time = struct.unpack("<q", raw_data[168:176])[0]

        if current_timestamp >= end_time and withdrawn_amount < deposit_amount:
            expired_streams.append({
                "stream_pda": pubkey,
                "sender": sender,
                "recipient": recipient,
                "mint": mint,
                "escrow_vault": escrow_vault,
            })
            
    return expired_streams

async def main():
    rpc_url = "https://api.devnet.solana.com"
    client = AsyncClient(rpc_url)
    keeper_keypair = Keypair() # In production: Keypair.from_bytes(secrets)
    logging.info(f"Starting KrakStream Keeper Bot. Pubkey: {keeper_keypair.pubkey()}")

    while True:
        try:
            current_time = int(time.time())
            expired = await fetch_expired_streams(client, current_time)
            logging.info(f"Scan complete: {len(expired)} executable streams identified.")

            for stream in expired:
                logging.info(f"Executing settlement crank for stream PDA: {stream['stream_pda']}")
                keeper_ata = get_associated_token_address(keeper_keypair.pubkey(), stream["mint"])
                recipient_ata = get_associated_token_address(stream["recipient"], stream["mint"])

                accounts = [
                    AccountMeta(pubkey=keeper_keypair.pubkey(), is_signer=True, is_writable=True),
                    AccountMeta(pubkey=stream["sender"], is_signer=False, is_writable=True),
                    AccountMeta(pubkey=stream["recipient"], is_signer=False, is_writable=True),
                    AccountMeta(pubkey=stream["stream_pda"], is_signer=False, is_writable=True),
                    AccountMeta(pubkey=stream["escrow_vault"], is_signer=False, is_writable=True),
                    AccountMeta(pubkey=keeper_ata, is_signer=False, is_writable=True),
                    AccountMeta(pubkey=recipient_ata, is_signer=False, is_writable=True),
                    AccountMeta(pubkey=stream["mint"], is_signer=False, is_writable=False),
                    AccountMeta(pubkey=TOKEN_PROGRAM_ID, is_signer=False, is_writable=False),
                ]

                ix = Instruction(PROGRAM_ID, CRANK_DISCRIMINATOR, accounts)
                latest_blockhash = (await client.get_latest_blockhash()).value.blockhash
                msg = MessageV0.try_compile(
                    payer=keeper_keypair.pubkey(),
                    instructions=[ix],
                    address_lookup_table_accounts=[],
                    recent_blockhash=latest_blockhash
                )
                tx = VersionedTransaction(msg, [keeper_keypair])
                tx_sig = await client.send_transaction(tx)
                logging.info(f"Crank executed successfully! Tx Signature: {tx_sig.value}")

        except Exception as e:
            logging.error(f"Keeper loop encountered exception: {e}")

        await asyncio.sleep(5)

if __name__ == "__main__":
    asyncio.run(main())