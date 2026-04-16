// Ethereum mainnet (chain 1) token registry.
//
// Verified against the canonical Uniswap default tokenlist
// (https://tokens.uniswap.org) and the CoinGecko Ethereum list
// (https://tokens.coingecko.com/ethereum/all.json). Every address is a
// well-known canonical deployment (WETH is the Etherscan-labeled Wrapped
// Ether at 0xC02a…, not some wrapped-of-wrapped variant). NEVER let a
// model generate addresses for this file.
//
// Symbols stored in display casing; `Token::lookup_symbol` compares
// case-insensitively so `dexquote usdc …` and `dexquote USDC …` both
// work.
const ETHEREUM_TOKENS: &[Entry] = &[
    // Stablecoins
    Entry { symbol: "USDC",    name: "USD Coin",                        address: address!("A0b86991c6218b36c1D19D4a2e9Eb0cE3606eB48"), decimals: 6 },
    Entry { symbol: "USDT",    name: "Tether USD",                      address: address!("dAC17F958D2ee523a2206206994597C13D831ec7"), decimals: 6 },
    Entry { symbol: "DAI",     name: "Dai Stablecoin",                  address: address!("6B175474E89094C44Da98b954EedeAC495271d0F"), decimals: 18 },
    Entry { symbol: "FRAX",    name: "Frax",                            address: address!("853d955aCEf822Db058eb8505911ED77F175b99e"), decimals: 18 },
    Entry { symbol: "LUSD",    name: "Liquity USD",                     address: address!("5f98805A4E8be255a32880FDeC7F6728C6568bA0"), decimals: 18 },
    Entry { symbol: "crvUSD",  name: "Curve.Fi USD",                    address: address!("f939E0A03FB07F59A73314E73794Be0E57ac1b4E"), decimals: 18 },
    Entry { symbol: "USDe",    name: "Ethena USDe",                     address: address!("4c9EDD5852cd905f086C759E8383e09bff1E68B3"), decimals: 18 },
    Entry { symbol: "sUSDe",   name: "Staked USDe",                     address: address!("9D39A5DE30e57443BfF2A8307A4256c8797A3497"), decimals: 18 },
    Entry { symbol: "PYUSD",   name: "PayPal USD",                      address: address!("6c3ea9036406852006290770BEdFcAbA0e23A0e8"), decimals: 6 },
    Entry { symbol: "GHO",     name: "Aave GHO",                        address: address!("40D16FC0246aD3160Ccc09B8D0D3A2cd28aE6C2f"), decimals: 18 },

    // Majors
    Entry { symbol: "WETH",    name: "Wrapped Ether",                   address: address!("C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"), decimals: 18 },
    Entry { symbol: "WBTC",    name: "Wrapped BTC",                     address: address!("2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599"), decimals: 8 },
    Entry { symbol: "cbBTC",   name: "Coinbase Wrapped BTC",            address: address!("cbB7C0000aB88B473b1f5aFd9ef808440eed33Bf"), decimals: 8 },
    Entry { symbol: "tBTC",    name: "Threshold BTC",                   address: address!("18084fbA666a33d37592fA2633fD49a74DD93a88"), decimals: 18 },

    // Liquid staking derivatives
    Entry { symbol: "stETH",   name: "Lido Staked ETH",                 address: address!("ae7ab96520DE3A18E5e111B5EaAb095312D7fE84"), decimals: 18 },
    Entry { symbol: "wstETH",  name: "Wrapped staked ETH",              address: address!("7f39C581F595B53c5cb19bD0b3f8dA6c935E2Ca0"), decimals: 18 },
    Entry { symbol: "rETH",    name: "Rocket Pool ETH",                 address: address!("ae78736Cd615f374D3085123A210448E74Fc6393"), decimals: 18 },
    Entry { symbol: "cbETH",   name: "Coinbase Wrapped Staked ETH",     address: address!("Be9895146f7AF43049ca1c1AE358B0541Ea49704"), decimals: 18 },
    Entry { symbol: "weETH",   name: "Wrapped eETH",                    address: address!("Cd5fE23C85820F7B72D0926FC9b05b43E359b7ee"), decimals: 18 },
    Entry { symbol: "sfrxETH", name: "Staked Frax Ether",               address: address!("ac3E018457B222d93114458476f3E3416Abbe38F"), decimals: 18 },

    // Blue-chip ERC20s
    Entry { symbol: "LINK",    name: "ChainLink Token",                 address: address!("514910771AF9Ca656af840dff83E8264EcF986CA"), decimals: 18 },
    Entry { symbol: "UNI",     name: "Uniswap",                         address: address!("1f9840a85d5aF5bf1D1762F925BDADdC4201F984"), decimals: 18 },
    Entry { symbol: "AAVE",    name: "Aave",                            address: address!("7Fc66500c84A76Ad7e9c93437bFc5Ac33E2DDaE9"), decimals: 18 },
    Entry { symbol: "CRV",     name: "Curve DAO Token",                 address: address!("D533a949740bb3306d119CC777fa900bA034cd52"), decimals: 18 },
    Entry { symbol: "BAL",     name: "Balancer",                        address: address!("ba100000625a3754423978a60c9317c58a424e3D"), decimals: 18 },
    Entry { symbol: "LDO",     name: "Lido DAO",                        address: address!("5A98FcBEA516Cf06857215779Fd812CA3beF1B32"), decimals: 18 },
    Entry { symbol: "MKR",     name: "Maker",                           address: address!("9f8F72aA9304c8B593d555F12eF6589cC3A579A2"), decimals: 18 },
    Entry { symbol: "SNX",     name: "Synthetix Network Token",         address: address!("C011a73ee8576Fb46F5E1c5751cA3B9Fe0af2a6F"), decimals: 18 },
    Entry { symbol: "COMP",    name: "Compound",                        address: address!("c00e94Cb662C3520282E6f5717214004A7f26888"), decimals: 18 },
    Entry { symbol: "1INCH",   name: "1inch",                           address: address!("111111111117dC0aa78b770fA6A738034120C302"), decimals: 18 },
    Entry { symbol: "CVX",     name: "Convex Finance",                  address: address!("4e3FBD56CD56c3e72c1403e103b45Db9da5B9D2B"), decimals: 18 },
    Entry { symbol: "FXS",     name: "Frax Share",                      address: address!("3432B6A60D23Ca0dFCa7761B7ab56459D9C964D0"), decimals: 18 },
    Entry { symbol: "PENDLE",  name: "Pendle",                          address: address!("808507121B80c02388fAd14726482e061B8da827"), decimals: 18 },
    Entry { symbol: "GRT",     name: "The Graph",                       address: address!("c944E90C64B2c07662A292be6244BDf05Cda44a7"), decimals: 18 },
    Entry { symbol: "ENS",     name: "Ethereum Name Service",           address: address!("C18360217D8F7Ab5e7c516566761Ea12Ce7F9D72"), decimals: 18 },
    Entry { symbol: "ENA",     name: "Ethena",                          address: address!("57e114B691Db790C35207b2e685D4A43181e6061"), decimals: 18 },
    Entry { symbol: "ETHFI",   name: "Ether.fi",                        address: address!("Fe0c30065B384F05761f15d0CC899D4F9F9Cc0eB"), decimals: 18 },

    // Memecoins / high-volume retail
    Entry { symbol: "SHIB",    name: "Shiba Inu",                       address: address!("95aD61b0a150d79219dCF64E1E6Cc01f0B64C4cE"), decimals: 18 },
    Entry { symbol: "PEPE",    name: "Pepe",                            address: address!("6982508145454Ce325dDbE47a25d4ec3d2311933"), decimals: 18 },
    Entry { symbol: "MOG",     name: "Mog Coin",                        address: address!("aaeE1A9723aaDB7afA2810263653A34bA2C21C7a"), decimals: 18 },

    // Gaming / metaverse
    Entry { symbol: "APE",     name: "ApeCoin",                         address: address!("4d224452801ACEd8B2F0aebE155379bb5D594381"), decimals: 18 },
    Entry { symbol: "MANA",    name: "Decentraland",                    address: address!("0F5D2fB29fb7d3CFeE444a200298f468908cC942"), decimals: 18 },
    Entry { symbol: "SAND",    name: "The Sandbox",                     address: address!("3845badAde8e6dFF049820680d1F14bD3903a5d0"), decimals: 18 },
    Entry { symbol: "IMX",     name: "Immutable X",                     address: address!("F57e7e7C23978C3cAEC3C3548E3D615c346e79fF"), decimals: 18 },

    // DeFi / infra tokens
    Entry { symbol: "RPL",     name: "Rocket Pool",                     address: address!("D33526068D116cE69F19A9ee46F0bd304F21A51f"), decimals: 18 },
    Entry { symbol: "LQTY",    name: "Liquity",                         address: address!("6DEA81C8171D0bA574754EF6F8b412F2Ed88c54D"), decimals: 18 },
    Entry { symbol: "STG",     name: "Stargate Finance",                address: address!("AF5191B0De278C7286d6C7CC6ab6BB8A73bA2Cd6"), decimals: 18 },
    Entry { symbol: "DYDX",    name: "dYdX",                            address: address!("92D6C1e31e14520e676a687F0a93788B716BEff5"), decimals: 18 },
    Entry { symbol: "GNO",     name: "Gnosis",                          address: address!("6810e776880C02933D47DB1b9fc05908e5386b96"), decimals: 18 },
    Entry { symbol: "BNT",     name: "Bancor Network Token",            address: address!("1F573D6Fb3F13d689FF844B4cE37794d79a7FF1C"), decimals: 18 },
];
