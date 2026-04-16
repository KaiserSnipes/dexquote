// Arbitrum One token registry.
//
// Verified against two authoritative sources:
//   - https://tokens.uniswap.org           (Uniswap default list)
//   - https://tokens.coingecko.com/arbitrum-one/all.json
//
// Every address has also been cross-checked on Arbiscan or live-quoted via
// the ODOS SOR endpoint, and this file is the source of truth — NEVER let a
// model generate addresses for it. Regenerate with `scripts/build-tokens.py`
// (see README) if you need to refresh.
//
// Symbols are stored in their display casing (e.g. `wstETH`, `USDC.e`); the
// `Token::lookup_symbol` function compares case-insensitively, so
// `dexquote usdc.e ...` and `dexquote WSTETH ...` both work.
const ARBITRUM_TOKENS: &[Entry] = &[
    // Stablecoins
    Entry { symbol: "USDC",    name: "USD Coin",                        address: address!("af88d065e77c8cC2239327C5EDb3A432268e5831"), decimals: 6 },
    Entry { symbol: "USDT",    name: "Tether USD",                      address: address!("Fd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9"), decimals: 6 },
    Entry { symbol: "DAI",     name: "Dai Stablecoin",                  address: address!("DA10009cBd5D07dd0CeCc66161FC93D7c9000da1"), decimals: 18 },
    Entry { symbol: "USDC.e",  name: "Bridged USDC",                    address: address!("FF970A61A04b1cA14834A43f5dE4533eBDDB5CC8"), decimals: 6 },
    Entry { symbol: "FRAX",    name: "Frax",                            address: address!("17FC002b466eec40DAe837Fc4bE5c67993ddBd6F"), decimals: 18 },
    Entry { symbol: "LUSD",    name: "Liquity USD",                     address: address!("93b346b6BC2548dA6A1E7d98E9a421B42541425b"), decimals: 18 },
    Entry { symbol: "crvUSD",  name: "Curve.Fi USD Stablecoin",         address: address!("498Bf2B1e120FeD3ad3D42EA2165E9b73f99C1e5"), decimals: 18 },

    // Majors
    Entry { symbol: "WETH",    name: "Wrapped Ether",                   address: address!("82aF49447D8a07e3bd95BD0d56f35241523fBab1"), decimals: 18 },
    Entry { symbol: "WBTC",    name: "Wrapped BTC",                     address: address!("2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f"), decimals: 8 },
    Entry { symbol: "tBTC",    name: "Threshold BTC",                   address: address!("6c84a8f1C29108F47a79964b5Fe888D4f4D0dE40"), decimals: 18 },
    Entry { symbol: "ARB",     name: "Arbitrum",                        address: address!("912CE59144191C1204E64559FE8253a0e49E6548"), decimals: 18 },

    // Liquid staking derivatives
    Entry { symbol: "wstETH",  name: "Wrapped stETH",                   address: address!("5979D7b546E38E414F7E9822514be443A4800529"), decimals: 18 },
    Entry { symbol: "rETH",    name: "Rocket Pool ETH",                 address: address!("EC70Dcb4A1EFa46b8F2D97C310C9c4790ba5ffA8"), decimals: 18 },
    Entry { symbol: "cbETH",   name: "Coinbase Wrapped Staked ETH",     address: address!("1DEBd73E752bEaF79865Fd6446b0c970EaE7732f"), decimals: 18 },
    Entry { symbol: "weETH",   name: "Wrapped eETH",                    address: address!("35751007a407ca6FEFfE80b3cB397736D2cf4dbe"), decimals: 18 },

    // Arbitrum-native majors
    Entry { symbol: "GMX",     name: "GMX",                             address: address!("fc5A1A6EB076a2C7aD06eD22C90d7E710E35ad0a"), decimals: 18 },
    Entry { symbol: "MAGIC",   name: "MAGIC",                           address: address!("539bdE0d7Dbd336b79148AA742883198BBF60342"), decimals: 18 },
    Entry { symbol: "GNS",     name: "Gains Network",                   address: address!("18c11FD286C5EC11c3b683Caa813B77f5163A122"), decimals: 18 },
    Entry { symbol: "RDNT",    name: "Radiant Capital",                 address: address!("3082CC23568eA640225c2467653dB90e9250AaA0"), decimals: 18 },
    Entry { symbol: "PENDLE",  name: "Pendle",                          address: address!("0c880f6761F1af8d9Aa9C466984b80DAb9a8c9e8"), decimals: 18 },
    Entry { symbol: "JOE",     name: "JOE",                             address: address!("371c7EC6D8039ff7933a2AA28EB827Ffe1F52f07"), decimals: 18 },

    // Blue-chip ERC20s bridged from Ethereum
    Entry { symbol: "LINK",    name: "ChainLink Token",                 address: address!("f97f4df75117a78c1A5a0DBb814Af92458539FB4"), decimals: 18 },
    Entry { symbol: "UNI",     name: "Uniswap",                         address: address!("Fa7F8980b0f1E64A2062791cc3b0871572f1F7f0"), decimals: 18 },
    Entry { symbol: "AAVE",    name: "Aave",                            address: address!("ba5DdD1f9d7F570dc94a51479a000E3BCE967196"), decimals: 18 },
    Entry { symbol: "CRV",     name: "Curve DAO Token",                 address: address!("11cDb42B0EB46D95f990BEdD4695a6e3fA034978"), decimals: 18 },
    Entry { symbol: "BAL",     name: "Balancer",                        address: address!("040d1EdC9569d4Bab2D15287Dc5A4F10F56a56B8"), decimals: 18 },
    Entry { symbol: "LDO",     name: "Lido DAO",                        address: address!("13Ad51ed4F1B7e9Dc168d8a00cB3f4dDD85EfA60"), decimals: 18 },
    Entry { symbol: "MKR",     name: "Maker",                           address: address!("2e9a6Df78E42a30712c10a9Dc4b1C8656f8F2879"), decimals: 18 },
    Entry { symbol: "SNX",     name: "Synthetix Network Token",         address: address!("cBA56Cd8216FCBBF3fA6DF6137F3147cBcA37D60"), decimals: 18 },
    Entry { symbol: "COMP",    name: "Compound",                        address: address!("354A6dA3fcde098F8389cad84b0182725c6C91dE"), decimals: 18 },
    Entry { symbol: "1INCH",   name: "1inch",                           address: address!("6314C31A7a1652cE482cffe247E9CB7c3f4BB9aF"), decimals: 18 },
    Entry { symbol: "GRT",     name: "The Graph",                       address: address!("9623063377AD1B27544C965cCd7342f7EA7e88C7"), decimals: 18 },
    Entry { symbol: "ENS",     name: "Ethereum Name Service",           address: address!("feA31d704DEb0975dA8e77Bf13E04239e70d7c28"), decimals: 18 },
    Entry { symbol: "APE",     name: "ApeCoin",                         address: address!("74885b4D524d497261259B38900f54e6dbAd2210"), decimals: 18 },
    Entry { symbol: "BLUR",    name: "Blur",                            address: address!("Ef171a5BA71348eff16616fd692855c2Fe606EB2"), decimals: 18 },
    Entry { symbol: "ENA",     name: "Ethena",                          address: address!("df8F0c63D9335A0AbD89F9F752d293A98EA977d8"), decimals: 18 },
    Entry { symbol: "ETHFI",   name: "Ether.fi",                        address: address!("07D65C18CECbA423298c0aEB5d2BeDED4DFd5736"), decimals: 18 },
    Entry { symbol: "IMX",     name: "Immutable X",                     address: address!("3cFD99593a7F035F717142095a3898e3Fca7783e"), decimals: 18 },
    Entry { symbol: "GNO",     name: "Gnosis Token",                    address: address!("a0b862F60edEf4452F25B4160F177db44DeB6Cf1"), decimals: 18 },
    Entry { symbol: "CVX",     name: "Convex Finance",                  address: address!("aAFcFD42c9954C6689ef1901e03db742520829c5"), decimals: 18 },
    Entry { symbol: "FXS",     name: "Frax Share",                      address: address!("d9f9d2Ee2d3EFE420699079f16D9e924affFdEA4"), decimals: 18 },
    Entry { symbol: "AXS",     name: "Axie Infinity",                   address: address!("e88998Fb579266628aF6a03e3821d5983e5D0089"), decimals: 18 },
    Entry { symbol: "BAT",     name: "Basic Attention Token",           address: address!("3450687EF141dCd6110b77c2DC44B008616AeE75"), decimals: 18 },
    Entry { symbol: "DYDX",    name: "dYdX",                            address: address!("51863cB90Ce5d6dA9663106F292fA27c8CC90c5a"), decimals: 18 },
    Entry { symbol: "BNT",     name: "Bancor Network Token",            address: address!("7A24159672b83ED1b89467c9d6A99556bA06D073"), decimals: 18 },
];
