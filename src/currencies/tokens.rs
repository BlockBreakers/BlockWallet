use colored::Colorize;
use std::path::PathBuf;
use std::fmt;
use std::fmt::Display;
use std::collections::HashMap;
use serde::Serialize;

#[derive(Clone, Debug)]
pub struct Tokens {
    pub eth_tokens: HashMap<String, Token>,
    pub token_transfers: HashMap<String, TransferMethods>
}

impl Tokens {
    pub fn new() -> Self {
        let token_transfers = HashMap::<String, TransferMethods>::from([
            ("0x5d3a536e4d6dbd6114cc1ead35777bab948e3643".to_string(), TransferMethods::Transfer), // cDAI
            ("0xfbbe9b1142c699512545f47937ee6fae0e4b0aa9".to_string(), TransferMethods::Transfer), // EDDA
            ("0xa3bed4e1c75d00fa6f4e5e6922db7261b5e9acd2".to_string(), TransferMethods::Transfer), // META
            ("0x2b591e99afe9f32eaa6214f7b7629768c40eeb39".to_string(), TransferMethods::Transfer), // HEX
            ("0x70d2b7c19352bb76e4409858ff5746e500f2b67c".to_string(), TransferMethods::Transfer), // UPI
            ("0x123151402076fc819b7564510989e475c9cd93ca".to_string(), TransferMethods::Transfer), // wDGLD
            ("0x36f3fd68e7325a35eb768f1aedaae9ea0689d723".to_string(), TransferMethods::Transfer), // ESD
            ("0x8b39b70e39aa811b69365398e0aace9bee238aeb".to_string(), TransferMethods::Transfer), // PKF
            ("0xd7b7d3c0bda57723fb54ab95fd8f9ea033af37f2".to_string(), TransferMethods::Transfer), // PYLON
            ("0xba3d9687cf50fe253cd2e1cfeede1d6787344ed5".to_string(), TransferMethods::Transfer), // aAAVE
            ("0x328c4c80bc7aca0834db37e6600a6c49e12da4de".to_string(), TransferMethods::Transfer), // aSNX
            ("0x16de59092dae5ccf4a1e6439d611fd0653f0bd01".to_string(), TransferMethods::Transfer), // yDAI
            ("0x7825e833d495f3d1c28872415a4aee339d26ac88".to_string(), TransferMethods::Transfer), // TLOS
            ("0x92b767185fb3b04f881e3ac8e5b0662a027a1d9f".to_string(), TransferMethods::Transfer), // crDAI
            ("0x3e780920601d61cedb860fe9c4a90c9ea6a35e78".to_string(), TransferMethods::Transfer), // BOOST
            ("0x6fb0855c404e09c47c3fbca25f08d4e41f9f062f".to_string(), TransferMethods::Transfer), // aZRX
            ("0xd7efb00d12c2c13131fd319336fdf952525da2af".to_string(), TransferMethods::Transfer), // XPR
            ("0x7d2d3688df45ce7c552e19c27e007673da9204b8".to_string(), TransferMethods::Transfer), // aLEND
            ("0x28cb7e841ee97947a86b06fa4090c8451f64c0be".to_string(), TransferMethods::Transfer), // YFL
            ("0x7420b4b9a0110cdc71fb720908340c03f9bc03ec".to_string(), TransferMethods::Transfer), // JASMY
            ("0x3a3a65aab0dd2a17e3f1947ba16138cd37d08c04".to_string(), TransferMethods::Transfer), // aETH
            ("0x515d7e9d75e2b76db60f8a051cd890eba23286bc".to_string(), TransferMethods::Transfer), // GDAO
            ("0x8b0e42f366ba502d787bb134478adfae966c8798".to_string(), TransferMethods::Transfer), // LABS
            ("0x6fce4a401b6b80ace52baaefe4421bd188e76f6f".to_string(), TransferMethods::Transfer), // aMANA
            ("0x0e29e5abbb5fd88e28b2d355774e73bd47de3bcd".to_string(), TransferMethods::Transfer), // HAKKA
            ("0x26ea744e5b887e5205727f55dfbe8685e3b21951".to_string(), TransferMethods::Transfer), // yUSDC
            ("0x4da9b813057d04baef4e5800e36083717b4a0341".to_string(), TransferMethods::Transfer), // aTUSD
            ("0x488e0369f9bc5c40c002ea7c1fe4fd01a198801c".to_string(), TransferMethods::Transfer), // GOF
            ("0x04fa0d235c4abf4bcf4787af4cf447de572ef828".to_string(), TransferMethods::Transfer), // UMA
            ("0x85eee30c52b0b379b046fb0f85f4f3dc3009afec".to_string(), TransferMethods::Transfer), // KEEP
            ("0x9ba00d6856a4edf4665bca2c2309936572473b7e".to_string(), TransferMethods::Transfer), // aUSDC
            ("0xed30dd7e50edf3581ad970efc5d9379ce2614adb".to_string(), TransferMethods::Transfer), // ARCX
            ("0x712db54daa836b53ef1ecbb9c6ba3b9efb073f40".to_string(), TransferMethods::Transfer), // aENJ
            ("0x71010a9d003445ac60c4e6a7017c1e89a477b438".to_string(), TransferMethods::Transfer), // aREP
            ("0xc2cb1040220768554cf699b0d863a3cd4324ce32".to_string(), TransferMethods::Transfer), // yDAI
            ("0x5caf454ba92e6f2c929df14667ee360ed9fd5b26".to_string(), TransferMethods::Transfer), // DEV
            ("0x66c0dded8433c9ea86c8cf91237b14e10b4d70b7".to_string(), TransferMethods::Transfer), // Mars
            ("0x71fc860f7d3a592a4a98740e39db31d25db65ae8".to_string(), TransferMethods::Transfer), // aUSDT
            ("0xfc1e690f61efd961294b3e1ce3313fbd8aa4f85d".to_string(), TransferMethods::Transfer), // aDAI
            ("0x6ee0f7bb50a54ab5253da0667b0dc2ee526c30a8".to_string(), TransferMethods::Transfer), // aBUSD
            ("0xbe9375c6a420d2eeb258962efb95551a5b722803".to_string(), TransferMethods::Transfer), // STMX
            ("0xc813ea5e3b48bebeedb796ab42a30c5599b01740".to_string(), TransferMethods::Transfer), // NIOX
            ("0xf650c3d88d12db855b8bf7d11be6c55a4e07dcc9".to_string(), TransferMethods::Transfer), // cUSDT
            ("0xe6354ed5bc4b393a5aad09f21c46e101e692d447".to_string(), TransferMethods::Transfer), // yUSDT
            ("0x20c36f062a31865bed8a5b1e512d9a1a20aa333a".to_string(), TransferMethods::Transfer), // DFD
            ("0x84ca8bc7997272c7cfb4d0cd3d55cd942b3c9419".to_string(), TransferMethods::Transfer), // DAI
            ("0x625ae63000f46200499120b906716420bd059240".to_string(), TransferMethods::Transfer), // aSUSD
            ("0xa1d0e215a23d7030842fc67ce582a6afa3ccab83".to_string(), TransferMethods::Transfer), // YFII
            ("0x07bac35846e5ed502aa91adf6a9e7aa210f2dcbe".to_string(), TransferMethods::Transfer), // erowan
            ("0x69948cc03f478b95283f7dbf1ce764d0fc7ec54c".to_string(), TransferMethods::Transfer), // aREN
            ("0xfa5047c9c78b8877af97bdcb85db743fd7313d4a".to_string(), TransferMethods::Transfer), // ROOK
            ("0x7deb5e830be29f91e298ba5ff1356bb7f8146998".to_string(), TransferMethods::Transfer), // aMKR
            ("0x0bc529c00c6401aef6d220be8c6ea1667f6ad93e".to_string(), TransferMethods::Transfer), // YFI
            ("0xbd2f0cd039e0bfcf88901c98c0bfac5ab27566e3".to_string(), TransferMethods::Transfer), // DSD
            ("0x04aa51bbcb46541455ccf1b8bef2ebc5d3787ec9".to_string(), TransferMethods::Transfer), // yWBTC
            ("0xe1ba0fb44ccb0d11b80f92f4f8ed94ca3ff51d00".to_string(), TransferMethods::Transfer), // aBAT
            ("0x0aacfbec6a24756c20d41914f2caba817c0d8521".to_string(), TransferMethods::Transfer), // YAM
            ("0x45f24baeef268bb6d63aee5129015d69702bcdfa".to_string(), TransferMethods::Transfer), // YFV
            ("0x1321f1f1aa541a56c31682c57b80ecfccd9bb288".to_string(), TransferMethods::Transfer), // ARCX
            ("0x9d91be44c06d373a8a226e1f3b146956083803eb".to_string(), TransferMethods::Transfer), // aKNC
            ("0x8888801af4d980682e47f1a9036e589479e835c5".to_string(), TransferMethods::Transfer), // MPH
            ("0x9e32b13ce7f2e80a01932b42553652e053d6ed8e".to_string(), TransferMethods::Transfer), // METIS
            ("0xfeea0bdd3d07eb6fe305938878c0cadbfa169042".to_string(), TransferMethods::Transfer), // 8PAY
            ("0xa64bd6c70cb9051f6a9ba1f163fdc07e0dfb5f84".to_string(), TransferMethods::Transfer), // aLINK
            ("0x06f3c323f0238c72bf35011071f2b5b7f43a054c".to_string(), TransferMethods::Transfer), // MASQ
            ("0x12e51e77daaa58aa0e9247db7510ea4b46f9bead".to_string(), TransferMethods::Transfer), // aYFI
            ("0x87edffde3e14c7a66c9b9724747a1c5696b742e6".to_string(), TransferMethods::Transfer), // SWAG
            ("0x35a18000230da775cac24873d00ff85bccded550".to_string(), TransferMethods::Transfer), // cUNI
            ("0xfc4b8ed459e00e5400be803a9bb3954234fd50e3".to_string(), TransferMethods::Transfer), // aWBTC
            ("0xd6ad7a6750a7593e092a9b218d66c0a814a3436e".to_string(), TransferMethods::Transfer), // yUSDC
            ("0x0d438f3b5175bebc262bf23753c1e53d03432bde".to_string(), TransferMethods::Transfer), // wNXM
            ("0xa8b919680258d369114910511cc87595aec0be6d".to_string(), TransferMethods::Transfer), // LYXe
            ("0xb124541127a0a657f056d9dd06188c4f1b0e5aab".to_string(), TransferMethods::Transfer), // aUNI
            ("0x6c972b70c533e2e045f333ee28b9ffb8d717be69".to_string(), TransferMethods::Transfer), // FRY
            ("0xa0246c9032bc3a600820415ae600c6388619a14d".to_string(), TransferMethods::Transfer), // FARM
            ("0x83f798e925bcd4017eb265844fddabb448f1707d".to_string(), TransferMethods::Transfer), // yUSDT
            ("0x1f8a626883d7724dbd59ef51cbd4bf1cf2016d13".to_string(), TransferMethods::Transfer), // STAK
            ("0x3505f494c3f0fed0b594e01fa41dd3967645ca39".to_string(), TransferMethods::Transfer), // SWM
            ("0x467bccd9d29f223bce8043b84e8c8b282827790f".to_string(), TransferMethods::TransferFrom), // TEL
            ("0xe9a95d175a5f4c9369f3b74222402eb1b837693b".to_string(), TransferMethods::TransferFrom), // NOW
            ("0xa4bdb11dc0a2bec88d24a3aa1e6bb17201112ebe".to_string(), TransferMethods::TransferFrom), // USDS
            ("0xc82e3db60a52cf7529253b4ec688f631aad9e7c2".to_string(), TransferMethods::TransferFrom), // ARC
            ("0xfc05987bd2be489accf0f509e44b0145d68240f7".to_string(), TransferMethods::TransferFrom), // ESS
            ("0x6fb3e0a217407efff7ca062d46c26e5d60a14d69".to_string(), TransferMethods::TransferFrom), // IOTX
            ("0x6c6ee5e31d828de241282b9606c8e98ea48526e2".to_string(), TransferMethods::TransferFrom), // HOT
            ("0x0ae055097c6d159879521c384f1d2123d1f195e6".to_string(), TransferMethods::TransferFrom), // STAKE
            ("0xfc82bb4ba86045af6f327323a46e80412b91b27d".to_string(), TransferMethods::TransferFrom), // PROM
            ("0x05079687d35b93538cbd59fe5596380cae9054a9".to_string(), TransferMethods::TransferFrom), // BTSG
            ("0x89bd2e7e388fab44ae88bef4e1ad12b4f1e0911c".to_string(), TransferMethods::TransferFrom),// NUX
            ("0x814e0908b12a99fecf5bc101bb5d0b8b5cdf7d26".to_string(), TransferMethods::TransferFrom), // MDT
            ("0x4de2573e27e648607b50e1cfff921a33e4a34405".to_string(), TransferMethods::TransferFrom), // LST
            ("0x5adc961d6ac3f7062d2ea45fefb8d8167d44b190".to_string(), TransferMethods::TransferFrom), // DTH
            ("0xcf3c8be2e2c42331da80ef210e9b1b307c03d36a".to_string(), TransferMethods::TransferFrom), // BEPRO
            ("0x340d2bde5eb28c1eed91b2f790723e3b160613b7".to_string(), TransferMethods::TransferFrom), // VEE
            ("0x408e41876cccdc0f92210600ef50372656052a38".to_string(), TransferMethods::TransferFrom), // REN
            ("0xb9ef770b6a5e12e45983c5d80545258aa38f3b78".to_string(), TransferMethods::TransferFrom), // ZCN
            ("0xe48972fcd82a274411c01834e2f031d4377fa2c0".to_string(), TransferMethods::TransferFrom), // 2KEY
            ("0x178c820f862b14f316509ec36b13123da19a6054".to_string(), TransferMethods::TransferFrom), // EWTB
            ("0x6f259637dcd74c767781e37bc6133cd6a68aa161".to_string(), TransferMethods::TransferFrom), // HT
            ("0xc28e931814725bbeb9e670676fabbcb694fe7df2".to_string(), TransferMethods::TransferFrom), // eQUAD
            ("0xebd9d99a3982d547c5bb4db7e3b1f9f14b67eb83".to_string(), TransferMethods::TransferFrom), // ID
            ("0xddb3422497e61e13543bea06989c0789117555c5".to_string(), TransferMethods::TransferFrom), // COTI
            ("0xee573a945b01b788b9287ce062a0cfc15be9fd86".to_string(), TransferMethods::TransferFrom), // XED
            ("0x0e8d6b471e332f140e7d9dbb99e5e3822f728da6".to_string(), TransferMethods::TransferFrom), // ABYSS
            ("0x83e6f1e41cdd28eaceb20cb649155049fac3d5aa".to_string(), TransferMethods::TransferFrom), // POLS
            ("0x10633216e7e8281e33c86f02bf8e565a635d9770".to_string(), TransferMethods::TransferFrom), // DVI
            ("0x08d967bb0134f2d07f7cfb6e246680c53927dd30".to_string(), TransferMethods::TransferFrom), // MATH
            ("0x58b6a8a3302369daec383334672404ee733ab239".to_string(), TransferMethods::TransferFrom), // LPT
            ("0xc4f6e93aeddc11dc22268488465babcaf09399ac".to_string(), TransferMethods::TransferFrom), // HI
            ("0x8207c1ffc5b6804f6024322ccf34f29c3541ae26".to_string(), TransferMethods::TransferFrom), // OGN
            ("0x967da4048cd07ab37855c090aaf366e4ce1b9f48".to_string(), TransferMethods::TransferFrom), // OCEAN
            ("0x4946fcea7c692606e8908002e55a582af44ac121".to_string(), TransferMethods::TransferFrom), // FOAM
            ("0x4730fb1463a6f1f44aeb45f6c5c422427f37f4d0".to_string(), TransferMethods::TransferFrom), // FOUR
            ("0x9992ec3cf6a55b00978cddf2b27bc6882d88d1ec".to_string(), TransferMethods::TransferFrom), // POLY
            ("0x0f71b8de197a1c84d31de0f1fa7926c365f052b3".to_string(), TransferMethods::TransferFrom), // ARCONA
            ("0x80fb784b7ed66730e8b1dbd9820afd29931aab03".to_string(), TransferMethods::TransferFrom), // LEND
            ("0xc719d010b63e5bbf2c0551872cd5316ed26acd83".to_string(), TransferMethods::TransferFrom), // DIP
            ("0x6a7ef4998eb9d0f706238756949f311a59e05745".to_string(), TransferMethods::TransferFrom), // KEN
            ("0xd559f20296ff4895da39b5bd9add54b442596a61".to_string(), TransferMethods::TransferFrom), // FTX
            ("0xbbbbca6a901c926f240b89eacb641d8aec7aeafd".to_string(), TransferMethods::TransferFrom), // LRC
            ("0xcca0c9c383076649604ee31b20248bc04fdf61ca".to_string(), TransferMethods::TransferFrom), // BTMX
            ("0x5cf04716ba20127f1e2297addcf4b5035000c9eb".to_string(), TransferMethods::TransferFrom), // NKN
            ("0x5b09a0371c1da44a8e24d36bf5deb1141a84d875".to_string(), TransferMethods::TransferFrom), // MAD
            ("0x4c11249814f11b9346808179cf06e71ac328c1b5".to_string(), TransferMethods::TransferFrom), // ORAI
            ("0x2260fac5e5542a773aa44fbcfedf7c193bc2c599".to_string(), TransferMethods::TransferFrom), // WBTC
            ("0x8c543aed163909142695f2d2acd0d55791a9edb9".to_string(), TransferMethods::TransferFrom), // VLX
            ("0xff56cc6b1e6ded347aa0b7676c85ab0b3d08b0fa".to_string(), TransferMethods::TransferFrom), // ORBS
            ("0x543ff227f64aa17ea132bf9886cab5db55dcaddf".to_string(), TransferMethods::TransferFrom), // GEN
            ("0xe1c7e30c42c24582888c758984f6e382096786bd".to_string(), TransferMethods::TransferFrom), // XCUR
            ("0x8eb24319393716668d768dcec29356ae9cffe285".to_string(), TransferMethods::TransferFrom), // AGI
            ("0x514910771af9ca656af840dff83e8264ecf986ca".to_string(), TransferMethods::TransferFrom), // LINK
            ("0xd478161c952357f05f0292b56012cd8457f1cfbf".to_string(), TransferMethods::TransferFrom), // POLK
            ("0xbf2179859fc6d5bee9bf9158632dc51678a4100e".to_string(), TransferMethods::TransferFrom), // ELF
            ("0x68d57c9a1c35f63e2c83ee8e49a64e9d70528d25".to_string(), TransferMethods::TransferFrom), // SRN
            ("0xbc86727e770de68b1060c91f6bb6945c73e10388".to_string(), TransferMethods::TransferFrom), // XNK
            ("0xd15ecdcf5ea68e3995b2d0527a0ae0a3258302f8".to_string(), TransferMethods::TransferFrom), // MCX
            ("0xa0cf46eb152656c7090e769916eb44a138aaa406".to_string(), TransferMethods::TransferFrom), // SPH
            ("0xb26631c6dda06ad89b93c71400d25692de89c068".to_string(), TransferMethods::TransferFrom), // MINDS
            ("0x4a220e6096b25eadb88358cb44068a3248254675".to_string(), TransferMethods::TransferFrom), // QNT
            ("0x5c872500c00565505f3624ab435c222e558e9ff8".to_string(), TransferMethods::TransferFrom), // COT
            ("0x3a9fff453d50d4ac52a6890647b823379ba36b9e".to_string(), TransferMethods::TransferFrom), // SHUF
            ("0x8290333cef9e6d528dd5618fb97a76f268f3edd4".to_string(), TransferMethods::TransferFrom)  // ANKR
        ]);

        let btc_path = crate::configuration::paths::token_icon_path("BTC");
        let eth_path = crate::configuration::paths::token_icon_path("ETH");
        let sol_path = crate::configuration::paths::token_icon_path("SOL");
        let ltc_path = crate::configuration::paths::token_icon_path("LTC");
        let xmr_path = crate::configuration::paths::token_icon_path("XMR");
        let usdc_path = crate::configuration::paths::token_icon_path("USDC");

        let mut eth_tokens = HashMap::new();
        eth_tokens.insert(
            String::from("btc:BTC"),
            Token {
                name: String::from("Bitcoin"),
                symbol: String::from("BTC"),
                address: String::from("0x0000000000000000000000000000000000000000"),
                logo: btc_path,
                decimals: 8,
                chain: String::from("btc"),
            },
        );
        eth_tokens.insert(
            String::from("ltc:LTC"),
            Token {
                name: String::from("Litecoin"),
                symbol: String::from("LTC"),
                address: String::from("ltc:native"),
                logo: ltc_path,
                decimals: 8,
                chain: String::from("ltc"),
            },
        );
        eth_tokens.insert(
            String::from("xmr:XMR"),
            Token {
                name: String::from("Monero"),
                symbol: String::from("XMR"),
                address: String::from("xmr:native"),
                logo: xmr_path,
                decimals: 12,
                chain: String::from("xmr"),
            },
        );
        eth_tokens.insert(
            String::from("eth:ETH"),
            Token {
                name: String::from("Ethereum"),
                symbol: String::from("ETH"),
                address: String::from("0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"),
                logo: eth_path,
                decimals: 18,
                chain: String::from("eth"),
            },
        );
        eth_tokens.insert(
            String::from("sol:SOL"),
            Token {
                name: String::from("Solana"),
                symbol: String::from("SOL"),
                address: String::from("11111111111111111111111111111111"),
                logo: sol_path,
                decimals: 9,
                chain: String::from("sol"),
            },
        );
        eth_tokens.insert(
            String::from("eth:USDC"),
            Token {
                name: String::from("USD Coin"),
                symbol: String::from("USDC"),
                address: String::from("0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"),
                logo: usdc_path,
                decimals: 6,
                chain: String::from("eth"),
            },
        );
        eth_tokens.insert(
            String::from("eth:USDT"),
            Token {
                name: String::from("Tether USD"),
                symbol: String::from("USDT"),
                address: String::from("0xdac17f958d2ee523a2206206994597c13d831ec7"),
                logo: crate::configuration::paths::token_icon_path("USDT"),
                decimals: 6,
                chain: String::from("eth"),
            },
        );
        eth_tokens.insert(
            String::from("eth:DAI"),
            Token {
                name: String::from("Dai Stablecoin"),
                symbol: String::from("DAI"),
                address: String::from("0x6b175474e89094c44da98b954eedeac495271d0f"),
                logo: crate::configuration::paths::token_icon_path("DAI"),
                decimals: 18,
                chain: String::from("eth"),
            },
        );
        eth_tokens.insert(
            String::from("eth:WBTC"),
            Token {
                name: String::from("Wrapped BTC"),
                symbol: String::from("WBTC"),
                address: String::from("0x2260fac5e5542a773aa44fbcfedf7c193bc2c599"),
                logo: crate::configuration::paths::token_icon_path("WBTC"),
                decimals: 8,
                chain: String::from("eth"),
            },
        );

        Tokens {
            eth_tokens:      eth_tokens,
            token_transfers: token_transfers
        }
    }

    pub fn len(&self) -> usize {
        self.eth_tokens.len()
    }
}

impl Display for Tokens {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut output = String::new();
        for (_key, value) in &self.eth_tokens {
            output.push_str(&format!("{}\n", value));
        }
        write!(f, "{}", output)
    }
}

#[derive(Serialize, Debug, Clone)]
pub struct Token {
    pub name    : String,
    pub symbol  : String,
    pub address : String,
    pub logo    : PathBuf,
    pub decimals: i32,
    /// Which wallet family this token belongs to: "btc", "eth", or "sol". This is the real
    /// dispatch key for chain-specific UI/send logic — `symbol` alone is not unique across
    /// chains (e.g. USDC exists as both an ERC-20 and an SPL token).
    pub chain   : String
}

impl Token {
    pub fn new(name: String, ticker: String, address: String, logo: PathBuf, digits: i32, chain: String) -> Self {
        Token {
            name    : name,
            symbol  : ticker,
            address : address,
            logo    : logo,
            decimals: digits,
            chain   : chain
        }
    }

    pub fn empty() -> Self {
        Token {
            name    : String::new(),
            symbol  : String::new(),
            address : String::new(),
            logo    : PathBuf::new(),
            decimals: 0,
            chain   : String::from("eth")
        }
    }
}

impl Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let output = [
            format!("      {}              {}\n", "Name".cyan().bold(), self.name),
            format!("      {}            {}\n", "Ticker".cyan().bold(), self.symbol),
            format!("      {}           {}\n", "Address".cyan().bold(), self.address),
            format!("      {}              {}\n", "logo".cyan().bold(), self.logo.display()),
            format!("      {}            {}\n", "Digits".cyan().bold(), self.decimals),
        ]
        .concat();
        
        write!(f, "{}", output)
    }
}

#[derive(Clone, Debug)]
pub enum TransferMethods {
    Transfer,
    TransferFrom
}