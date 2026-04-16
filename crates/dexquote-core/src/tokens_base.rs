// Base (chain 8453) token registry.
//
// Verified against the Uniswap default tokenlist
// (https://tokens.uniswap.org) and the CoinGecko Base list
// (https://tokens.coingecko.com/base/all.json). Core addresses — USDC,
// WETH, Chainlink feeds, Aerodrome — were cross-checked against BaseScan
// and on-chain via eth_getCode / latestAnswer before landing here.
//
// Symbols stored in display casing; lookup is case-insensitive via
// `eq_ignore_ascii_case`.
const BASE_TOKENS: &[Entry] = &[
    // Stablecoins
    Entry { symbol: "USDC",    name: "USD Coin",                        address: address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"), decimals: 6 },
    Entry { symbol: "USDbC",   name: "USD Base Coin (bridged)",         address: address!("d9aAEc86B65D86f6A7B5B1b0c42FFA531710b6CA"), decimals: 6 },
    Entry { symbol: "DAI",     name: "Dai Stablecoin",                  address: address!("50c5725949A6F0c72E6C4a641F24049A917DB0Cb"), decimals: 18 },
    Entry { symbol: "scrvUSD", name: "Savings crvUSD",                  address: address!("646A737B9B6024e49f5908762B3fF73e65B5160c"), decimals: 18 },
    Entry { symbol: "USDSM",   name: "USDS (Maker)",                    address: address!("26c358F7c5FEdb20A6DDEF108CD91eFb6B8dA0Cb"), decimals: 18 },
    Entry { symbol: "tBTC",    name: "Threshold BTC",                   address: address!("236aa50979D5f3De3Bd1Eeb40E81137F22ab794b"), decimals: 18 },
    Entry { symbol: "crvUSD",  name: "Curve.Fi USD",                    address: address!("417Ac0e078398C154EdFadD9Ef675d30Be60Af93"), decimals: 18 },
    Entry { symbol: "superOETHb", name: "Super OETH on Base",           address: address!("DBFeFD2e8460a6Ee4955A68582F85708BAEA60A3"), decimals: 18 },

    // Majors
    Entry { symbol: "WETH",    name: "Wrapped Ether",                   address: address!("4200000000000000000000000000000000000006"), decimals: 18 },
    Entry { symbol: "cbBTC",   name: "Coinbase Wrapped BTC",            address: address!("cbB7C0000aB88B473b1f5aFd9ef808440eed33Bf"), decimals: 8 },

    // Liquid staking derivatives
    Entry { symbol: "cbETH",   name: "Coinbase Wrapped Staked ETH",     address: address!("2Ae3F1Ec7F1F5012CFEab0185bfc7aa3cf0DEc22"), decimals: 18 },
    Entry { symbol: "wstETH",  name: "Wrapped staked ETH",              address: address!("c1CBa3fCea344f92D9239c08C0568f6F2F0ee452"), decimals: 18 },
    Entry { symbol: "weETH",   name: "Wrapped eETH",                    address: address!("04C0599Ae5A44757c0af6F9eC3b93da8976c150A"), decimals: 18 },
    Entry { symbol: "rETH",    name: "Rocket Pool ETH",                 address: address!("B6fe221Fe9EeF5aBa221c348bA20A1Bf5e73624c"), decimals: 18 },

    // Base-native DEX + governance
    Entry { symbol: "AERO",    name: "Aerodrome",                       address: address!("940181a94A35A4569E4529A3CDfB74e38FD98631"), decimals: 18 },

    // Base blue chips (bridged)
    Entry { symbol: "UNI",     name: "Uniswap",                         address: address!("c3De830EA07524a0761646a6a4e4be0e114a3C83"), decimals: 18 },
    Entry { symbol: "LINK",    name: "ChainLink Token",                 address: address!("88Fb150BDc53A65fe94Dea0c9BA0a6dAf8C6e196"), decimals: 18 },
    Entry { symbol: "AAVE",    name: "Aave",                            address: address!("63706e401c06ac8513145b7687A14804d17f814b"), decimals: 18 },
    Entry { symbol: "COMP",    name: "Compound",                        address: address!("9e1028F5F1D5eDe59748FFceE5532509976840E0"), decimals: 18 },
    Entry { symbol: "CRV",     name: "Curve DAO Token",                 address: address!("8Ee73c484A26e0A5df2Ee2a4960B789967dd0415"), decimals: 18 },

    // Base memecoins (high volume)
    Entry { symbol: "DEGEN",   name: "Degen",                           address: address!("4ed4E862860beD51a9570b96d89aF5E1B0Efefed"), decimals: 18 },
    Entry { symbol: "BRETT",   name: "Brett",                           address: address!("532f27101965dd16442E59d40670FaF5eBB142E4"), decimals: 18 },
    Entry { symbol: "TOSHI",   name: "Toshi",                           address: address!("Ac1Bd2486aAf3B5C0fc3Fd868558b082a531B2B4"), decimals: 18 },
    Entry { symbol: "MOXIE",   name: "Moxie Protocol",                  address: address!("8C9037D1Ef5c6D1f6816278C7AAF5491d24CD527"), decimals: 18 },
    Entry { symbol: "VIRTUAL", name: "Virtual Protocol",                address: address!("0b3e328455c4059EEb9e3f84b5543F74E24e7E1b"), decimals: 18 },
];
