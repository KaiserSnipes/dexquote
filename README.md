# dexquote

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Compare swap quotes across 25 DEX backends on 4 chains from your terminal. Read-only, no wallet, no signing, no API keys.

```
$ dexquote WETH USDC 1

 1 WETH -> USDC on Ethereum
 ----------------------------------------------------------------
 UniswapV2    2,355.1257 USDC  2,354.78   gas $0.34
 UniswapV3    2,355.9592 USDC  2,355.62   gas $0.34
 UniswapV4    2,355.5220 USDC  2,355.18   gas $0.34
 SushiV2      2,354.7341 USDC  2,354.39   gas $0.34
 PancakeV3    2,355.9668 USDC  2,355.63   gas $0.34    best net
 Curve        2,355.9518 USDC  2,355.61   gas $0.34
 ODOS         2,356.0175 USDC       --    gas --        best
 Paraswap     2,355.9771 USDC  2,355.63   gas $0.34
 KyberSwap    2,356.0364 USDC  2,355.69   gas $0.34
 OpenOcean    2,355.9709 USDC       --    gas --
 LiFi         2,355.4913 USDC  2,355.15   gas $0.34
 CoWSwap      2,355.6350 USDC       --    gas --
 ----------------------------------------------------------------
 Best rate: 2,356.04 USDC per 1 WETH
 Spread 0.06%  ·  Best KyberSwap  (+0.05 USDC vs median)  ·  4.6s
```

## Why dexquote?

- **25 backends, 4 chains, zero setup.** 14 on-chain DEX quoters + 6 EVM aggregators + 5 Solana aggregators. Works out of the box with public RPCs.
- **See the actual routing.** `dexquote route` shows the multi-hop path each aggregator selected -- information you can't get from any other free tool. OpenOcean might split your trade across 9 venues; CoW Swap might route through a single solver. Now you can see it.
- **Price-impact curves.** `dexquote depth` quotes the same pair at 5 notional levels (0.1x to 1000x) and shows where liquidity breaks down.
- **Per-backend leaderboard.** `dexquote benchmark` sweeps ~30 canonical pairs across all chains and ranks every backend by win count, success rate, latency, and spread.
- **Full-screen interactive TUI.** Run bare `dexquote` for a menu-driven experience with live gas tracking, streaming results, and every subcommand accessible without memorizing CLI syntax.
- **Gas-aware best selection.** The `net` column subtracts gas cost from the quoted amount, so you see which backend actually gives you the most tokens after fees.
- **Scriptable.** `--json` and `--minimal` output modes for pipelines. `--watch` for polling. `--at-block` for historical replay.

## Prerequisites

- **No prerequisites for pre-built binaries.** Download and run.
- **Building from source** requires a [Rust toolchain](https://rustup.rs/) (stable, 1.80+).
- **Optional:** An RPC endpoint for your chain. Public endpoints are baked in and work for casual use, but rate-limit under heavy load. For production use, grab a free tier from [Alchemy](https://www.alchemy.com/), [Infura](https://www.infura.io/), or [QuickNode](https://www.quicknode.com/).

## Install

### Pre-built binary

Grab a release from [GitHub Releases](https://github.com/KaiserSnipes/dexquote/releases/latest):

```sh
# Linux / macOS
curl -L https://github.com/KaiserSnipes/dexquote/releases/latest/download/dexquote-x86_64-linux.tar.gz | tar xz
sudo mv dexquote /usr/local/bin/

# Windows (PowerShell)
Invoke-WebRequest -Uri https://github.com/KaiserSnipes/dexquote/releases/latest/download/dexquote-x86_64-windows.zip -OutFile dexquote.zip
Expand-Archive dexquote.zip
# Move dexquote.exe to a folder on your PATH
```

### From source

```sh
git clone https://github.com/KaiserSnipes/dexquote.git
cd dexquote
cargo install --path crates/dexquote
```

First run creates a config file at your OS-standard path (`~/.config/dexquote/config.json` on Linux/macOS, `%APPDATA%\dexquote\config.json` on Windows) with a public RPC baked in, so `dexquote WETH USDC 1` works immediately.

## Quick start

```sh
# Compare quotes for 1 WETH -> USDC on Ethereum (default chain)
dexquote WETH USDC 1

# Switch to Arbitrum
dexquote --chain arbitrum WETH USDC 1

# Launch the interactive TUI (main menu -> chain picker -> action)
dexquote

# See which DEXs each aggregator routes through
dexquote route WETH USDC 1

# Price-impact curve at 5 notional levels
dexquote depth WETH USDC 1

# Benchmark every backend across ~30 canonical pairs
dexquote benchmark

# Self-test your RPC + every backend
dexquote doctor
```

## Usage

### Interactive mode

```sh
dexquote
```

Launches a full-screen TUI with a main menu. Pick an action (Quote, Depth, Route, Benchmark, Doctor, Tokens, History), pick a chain, and everything runs inside the same window. Esc backs out to the menu; Esc from the menu quits.

| Key | Action |
|-----|--------|
| **Arrow keys / j k** | navigate lists |
| **Tab / Shift-Tab** | cycle form fields |
| **Enter** | select / fire quote |
| **1-8** | jump to menu item |
| **R** | re-run last quote |
| **S** | swap sell / buy |
| **Y** | copy best quote to clipboard |
| **?** | help overlay |
| **Esc** | back (quits from main menu) |

A live gas tracker runs along the top: gas price (gwei), ETH/USD from Chainlink, estimated swap cost, block number, and freshness. It respawns automatically when you switch chains.

### Direct mode

```sh
dexquote <SELL> <BUY> <AMOUNT> [OPTIONS]
```

Returns a quote table and exits. Supports `--json` and `--minimal` for scripting:

```sh
dexquote WETH USDC 1 --json | jq '.[] | select(.best) | .amount_out'
dexquote WETH USDC 1 --minimal | cut -f2
```

### Subcommands

```sh
dexquote route WETH USDC 1            # multi-hop path each backend used
dexquote depth WETH USDC 1            # price-impact curve (0.1x - 1000x)
dexquote benchmark                    # per-backend leaderboard (~30 pairs)
dexquote doctor                       # self-test RPC + every backend
dexquote tokens                       # browse bundled token registry
dexquote history                      # list recent quotes
dexquote last                         # re-run the most recent quote
dexquote config show                  # view config
dexquote config set defaults.chain base   # change default chain
dexquote completions bash             # shell completions
```

All subcommands are also reachable from the interactive TUI menu.

### Options

| Flag | Description |
|------|-------------|
| `--chain <CHAIN>` | `arbitrum`, `base`, `ethereum`, or `solana` |
| `--rpc <URL>` | RPC endpoint override (env: `DEXQUOTE_RPC`) |
| `--backends <LIST>` | Comma-separated backend filter |
| `--timeout <MS>` | Per-backend timeout in milliseconds |
| `--watch <DURATION>` | Re-run on interval (e.g. `30s`, `1m`) |
| `--at-block <HEIGHT>` | Historical replay (on-chain backends only) |
| `--json` / `--minimal` | Machine-readable output |
| `--color <MODE>` | `auto` / `always` / `never` |
| `-i, --interactive` | Force TUI even with positional args |

Setting precedence: **CLI flag > env var > config file > built-in default**.

### Amount shortcuts

```sh
dexquote USDT WETH 100k    # 100,000
dexquote WETH USDC 1.5m    # 1,500,000
dexquote WETH USDC 1e3     # 1,000
```

### Token resolution

Symbols are case-insensitive. Common aliases work (`eth` -> `WETH`, `btc` -> `WBTC`). Typos fuzzy-match:

```
$ dexquote WTH USDC 1
  sell token: unknown token symbol `WTH` on Ethereum
  -> did you mean `WETH`, `wstETH`, `weETH`?
```

Any ERC-20 works via raw `0x...` address -- dexquote fetches `decimals()` and `symbol()` on-chain automatically.

## Supported chains and backends

**4 chains, 25 backends, zero API keys.**

### EVM on-chain (14 backends)

These call smart contracts directly via `eth_call`. Require an RPC endpoint (public defaults baked in).

| Backend | Arbitrum | Base | Ethereum |
|---------|:--------:|:----:|:--------:|
| UniswapV2 | | | x |
| UniswapV3 | x | x | x |
| UniswapV4 | | | x |
| SushiV2 | x | x | x |
| FraxSwap | | | x |
| TraderJoe | x | | |
| PancakeV3 | x | x | x |
| CamelotV3 | x | | |
| Curve | x | x | x |
| Aerodrome | | x | |
| Slipstream | | x | |
| BalancerV2 | x | x | x |
| MaverickV2 | x | x | x |
| DODO | | | x |

### EVM HTTP aggregators (6 backends)

No RPC needed. Each routes across dozens of underlying DEXs.

| Backend | Endpoint |
|---------|----------|
| ODOS | `api.odos.xyz` |
| Paraswap | `apiv5.paraswap.io` |
| KyberSwap | `aggregator-api.kyberswap.com` |
| OpenOcean | `open-api.openocean.finance` |
| LiFi | `li.quest` |
| CoWSwap | `api.cow.fi` |

### Solana HTTP aggregators (5 backends)

| Backend | What it routes through |
|---------|----------------------|
| Jupiter Swap | Every major Solana DEX (Raydium, Orca, Meteora, Phoenix, ...) |
| Jupiter Ultra | Jupiter's newer "iris" router -- different path from Swap |
| Raydium | Raydium's own pools (CPMM, CLMM, stable) |
| OpenOcean | Bundles Jupiter + Titan internally |
| LiFi | Routes through OKX DEX |

Backend availability is chain-aware -- unsupported backends are silently filtered. You never see "no route" for a backend that doesn't exist on that chain.

## Configuration

```sh
dexquote config show                          # view all settings
dexquote config set defaults.chain solana     # switch default chain
dexquote config set defaults.rpc https://...  # set custom RPC
dexquote config set defaults.timeout_ms 5000  # adjust timeout
dexquote config edit                          # open in $EDITOR
dexquote config reset                         # restore defaults
```

Config lives at `~/.config/dexquote/config.json` (Linux/macOS) or `%APPDATA%\dexquote\config.json` (Windows).

## Architecture

```
dexquote (binary crate)
  clap CLI + ratatui TUI + config + streaming renderer + error handling

dexquote-core (library crate)
  DexBackend trait + 25 backend implementations + token registry
  + quote_all fan-out + GasPricer (Chainlink ETH/USD)
```

Built with [alloy](https://github.com/alloy-rs/alloy) for EVM contract calls, [ratatui](https://github.com/ratatui/ratatui) for the TUI, [tokio](https://tokio.rs/) for async, and [reqwest](https://github.com/seanmonstar/reqwest) for HTTP aggregator APIs.

## Development

```sh
cargo test --workspace                    # unit tests (25 tests)
cargo clippy --workspace -- -D warnings   # lint
cargo build --release -p dexquote         # release build

# Live integration tests (hit real endpoints)
DEXQUOTE_TEST_RPC=https://arb1.arbitrum.io/rpc \
  cargo test --workspace -- --ignored
```

## Contributing

Contributions welcome. Some ideas:

- **New backends** -- any DEX with a public quoter contract or API
- **New chains** -- Polygon, Avalanche, BSC, etc.
- **Direct Solana program integration** -- Raydium/Orca/Meteora on-chain quoters
- **Bug reports** -- especially "backend X returns wrong/stale quotes on chain Y"

Please open an issue before starting large PRs so we can align on approach.

## License

MIT -- see [LICENSE](LICENSE).
