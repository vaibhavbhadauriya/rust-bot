import asyncio
import json
import csv
import os
from datetime import datetime, timezone
import websockets
from eth_utils import to_checksum_address

# ====================== CONFIG ======================

ALCHEMY_KEY = os.environ["ALCHEMY_KEY"]
RPC_URL = f"wss://eth-mainnet.g.alchemy.com/v2/{ALCHEMY_KEY}"

LOG_FILE = "spread_log.csv"

GET_RESERVES_SELECTOR = "0x0902f1ac"
GET_PAIR_SELECTOR = "e6a43905"  # getPair(address,address)

EXCHANGES = {
    "Uniswap_V2": {
        "factory": "0x5C69bEe701ef814a2B6a3EDD4B1652CB9cc5aA6f",
    },
    "SushiSwap": {
        "factory": "0xC0AEe478e3658e2610c5F7A4A2E1777cE9e4f2Ac",
    }
}

TOKENS = {
    "WETH": {"address": "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2", "decimals": 18},
    "USDT": {"address": "0xdAC17F958D2ee523a2206206994597C13D831ec7", "decimals": 6},
    "USDC": {"address": "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48", "decimals": 6},
    "WBTC": {"address": "0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599", "decimals": 8},
}

PAIRS_TO_SCAN = [
    ("WETH", "USDT"),
    ("WETH", "USDC"),
    ("WBTC", "USDT"),
    ("WBTC", "USDC"),
]

DECIMALS_MAP = {v["address"].lower(): v["decimals"] for v in TOKENS.values()}
ZERO_ADDRESS = "0x0000000000000000000000000000000000000000"

# ====================== CSV LOGGING ======================

def init_log_file():
    file_exists = os.path.isfile(LOG_FILE)
    if not file_exists:
        with open(LOG_FILE, "w", newline="") as f:
            writer = csv.writer(f)
            writer.writerow([
                "timestamp_utc",
                "base",
                "quote",
                "exchange_1",
                "price_1",
                "exchange_2",
                "price_2",
                "spread_pct"
            ])
        print(f"Created new log file: {LOG_FILE}")
    else:
        print(f"Appending to existing log file: {LOG_FILE}")

def log_spread(base_symbol, quote_symbol, name1, p1, name2, p2, spread):
    timestamp = datetime.now(timezone.utc).isoformat()
    with open(LOG_FILE, "a", newline="") as f:
        writer = csv.writer(f)
        writer.writerow([
            timestamp, base_symbol, quote_symbol,
            name1, f"{p1:.8f}",
            name2, f"{p2:.8f}",
            f"{spread:.6f}"
        ])

# ====================== RPC CALL ======================

request_id_counter = 0

async def eth_call(ws, to_address, data):
    global request_id_counter
    request_id_counter += 1
    req_id = request_id_counter

    payload = {
        "jsonrpc": "2.0",
        "method": "eth_call",
        "params": [{"to": to_address, "data": data}, "latest"],
        "id": req_id
    }

    await ws.send(json.dumps(payload))

    while True:
        raw = await ws.recv()
        response = json.loads(raw)
        if response.get("id") == req_id:
            if "error" in response:
                raise RuntimeError(f"RPC error: {response['error']}")
            return response["result"]

# ====================== GET REAL PAIR ADDRESS FROM FACTORY ======================

def encode_get_pair(token_a, token_b):
    addr_a = token_a[2:].lower().zfill(64)
    addr_b = token_b[2:].lower().zfill(64)
    return "0x" + GET_PAIR_SELECTOR + addr_a + addr_b

def decode_address(result_hex):
    data = result_hex[2:]
    addr_hex = data[-40:]
    return to_checksum_address("0x" + addr_hex)

async def resolve_pair_address(ws, factory_address, token_a, token_b):
    call_data = encode_get_pair(token_a, token_b)
    result = await eth_call(ws, factory_address, call_data)
    pair_address = decode_address(result)
    if pair_address.lower() == ZERO_ADDRESS.lower():
        raise ValueError("Pair does not exist on this exchange")
    return pair_address

# ====================== DECODE / PRICE ======================

def decode_reserves(result_hex):
    data = result_hex[2:]
    if len(data) < 128:
        raise ValueError(f"Unexpected short response, length={len(data)}")
    reserve0 = int(data[0:64], 16)
    reserve1 = int(data[64:128], 16)
    return reserve0, reserve1

def compute_price(reserve0, reserve1, token0, token1, base_addr, quote_addr):
    base_addr = base_addr.lower()
    quote_addr = quote_addr.lower()
    token0 = token0.lower()
    token1 = token1.lower()

    if token0 == base_addr:
        reserve_base, reserve_quote = reserve0, reserve1
    else:
        reserve_base, reserve_quote = reserve1, reserve0

    if reserve_base == 0:
        raise ValueError("Zero base reserve — empty or invalid pool")

    base_dec = DECIMALS_MAP[base_addr]
    quote_dec = DECIMALS_MAP[quote_addr]

    real_base = reserve_base / (10 ** base_dec)
    real_quote = reserve_quote / (10 ** quote_dec)

    return real_quote / real_base

# ====================== STARTUP: RESOLVE ALL PAIR ADDRESSES ONCE ======================

async def build_pair_map(ws):
    pair_map = {}

    for base_symbol, quote_symbol in PAIRS_TO_SCAN:
        base = TOKENS[base_symbol]
        quote = TOKENS[quote_symbol]

        for ex_name, ex_data in EXCHANGES.items():
            try:
                pool_address = await resolve_pair_address(
                    ws, ex_data["factory"], base["address"], quote["address"]
                )

                t0, t1 = (base["address"], quote["address"]) if int(base["address"], 16) < int(quote["address"], 16) else (quote["address"], base["address"])

                pair_map[(base_symbol, quote_symbol, ex_name)] = (pool_address, t0, t1)
                print(f"[RESOLVED] {base_symbol}/{quote_symbol} on {ex_name}: {pool_address}")

            except Exception as e:
                print(f"[RESOLVE ERROR] {base_symbol}/{quote_symbol} on {ex_name}: {e}")

    return pair_map

# ====================== CORE SCAN LOGIC (runs inside one live connection) ======================

async def run_scanner(ws):
    print("Resolving real pair addresses from factories...")
    pair_map = await build_pair_map(ws)
    print("Scanning all vectors...")
    print("-" * 60)

    while True:
        for base_symbol, quote_symbol in PAIRS_TO_SCAN:
            prices = {}

            for ex_name in EXCHANGES:
                key = (base_symbol, quote_symbol, ex_name)
                if key not in pair_map:
                    continue

                pool_address, t0, t1 = pair_map[key]

                try:
                    result = await eth_call(ws, pool_address, GET_RESERVES_SELECTOR)
                    reserve0, reserve1 = decode_reserves(result)
                    base_addr = TOKENS[base_symbol]["address"]
                    quote_addr = TOKENS[quote_symbol]["address"]
                    price = compute_price(reserve0, reserve1, t0, t1, base_addr, quote_addr)
                    prices[ex_name] = price

                except (websockets.exceptions.ConnectionClosed, ConnectionError, OSError):
                    raise

                except Exception as e:
                    print(f"[ERROR] {base_symbol}/{quote_symbol} on {ex_name}: {e}")

            if len(prices) == 2:
                names = list(prices.keys())
                p1, p2 = prices[names[0]], prices[names[1]]
                spread = abs(p1 - p2) / ((p1 + p2) / 2) * 100

                print(
                    f"[LIVE] {base_symbol}/{quote_symbol} "
                    f"{names[0]}: ${p1:,.2f} | {names[1]}: ${p2:,.2f} | "
                    f"Spread: {spread:.4f}%"
                )

                log_spread(base_symbol, quote_symbol, names[0], p1, names[1], p2, spread)

        await asyncio.sleep(1.0)

# ====================== OUTER LOOP: HANDLES CONNECTION + AUTO-RECONNECT ======================

async def scan_loop():
    print("Initializing the Panopticon... calculating pool coordinates. 6767.")
    init_log_file()

    while True:
        try:
            async with websockets.connect(RPC_URL, ping_interval=20, ping_timeout=20) as ws:
                print("Connected to mainnet.")
                await run_scanner(ws)

        except (websockets.exceptions.ConnectionClosed, ConnectionError, OSError) as e:
            print(f"[CONNECTION LOST] {e}")
            print("Reconnecting in 3 seconds...")
            await asyncio.sleep(3)

        except Exception as e:
            print(f"[UNEXPECTED ERROR] {e}")
            print("Reconnecting in 3 seconds...")
            await asyncio.sleep(3)

if __name__ == "__main__":
    asyncio.run(scan_loop())