extern crate core;

use std::{fs, path::PathBuf, str::FromStr};

use clap::{Parser, Subcommand};
use jsonrpsee::{
    core::{client::Error, ClientError},
    http_client::{HttpClient, HttpClientBuilder},
};
use protocol::{
    bitcoin::{Amount, FeeRate, OutPoint, Txid},
    hasher::KeyHasher,
    slabel::SLabel,
};
use serde::{Deserialize, Serialize};
use spaced::{
    config::{default_spaces_rpc_port, ExtendedNetwork},
    rpc::{
        BidParams, ExecuteParams, OpenParams, RegisterParams, RpcClient, RpcWalletRequest,
        RpcWalletTxBuilder, SendCoinsParams, TransferSpacesParams,
    },
    store::Sha256,
    wallets::AddressKind,
};
use wallet::export::WalletExport;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
    /// Bitcoin network to use
    #[arg(long, env = "SPACED_CHAIN", default_value = "mainnet")]
    chain: ExtendedNetwork,
    /// Spaced RPC URL [default: based on specified chain]
    #[arg(long)]
    spaced_rpc_url: Option<String>,
    /// Specify wallet to use
    #[arg(long, short, global = true, default_value = "default")]
    wallet: String,
    /// Custom dust amount in sat for bid outputs
    #[arg(long, short, global = true)]
    dust: Option<u64>,
    /// Force invalid transaction (for testing only)
    #[arg(long, global = true, default_value = "false")]
    force: bool,
    /// Skip tx checker (not recommended)
    #[arg(long, global = true, default_value = "false")]
    skip_tx_check: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug, Clone)]
enum Commands {
    /// Generate a new wallet
    #[command(name = "createwallet")]
    CreateWallet,
    /// Load a wallet
    #[command(name = "loadwallet")]
    LoadWallet,
    /// Export a wallet
    #[command(name = "exportwallet")]
    ExportWallet {
        // Destination path to export json file
        path: PathBuf,
    },
    /// Import a wallet
    #[command(name = "importwallet")]
    ImportWallet {
        // Wallet json file to import
        path: PathBuf,
    },
    /// Export a wallet
    #[command(name = "getwalletinfo")]
    GetWalletInfo,
    /// Export a wallet
    #[command(name = "getserverinfo")]
    GetServerInfo,
    /// Open an auction
    Open {
        /// Space name
        space: String,
        /// Amount in sats
        #[arg(default_value = "1000")]
        initial_bid: u64,
        /// Fee rate to use in sat/vB
        #[arg(long, short)]
        fee_rate: Option<u64>,
    },
    /// Place a bid
    Bid {
        /// Space name
        space: String,
        /// Amount in satoshi
        amount: u64,
        /// Fee rate to use in sat/vB
        #[arg(long, short)]
        fee_rate: Option<u64>,
        #[arg(long, short, default_value = "false")]
        confirmed_only: bool,
    },
    /// Register a won auction
    Register {
        /// Space name
        space: String,
        /// Recipient address
        address: Option<String>,
        /// Fee rate to use in sat/vB
        #[arg(long, short)]
        fee_rate: Option<u64>,
    },
    /// Get space info
    #[command(name = "getspace")]
    GetSpace {
        /// The space name
        space: String,
    },
    /// Transfer ownership of a set of spaces to the given name or address
    #[command(
        name = "transfer",
        override_usage = "space-cli transfer [SPACES]... --to <SPACE-OR-ADDRESS>"
    )]
    Transfer {
        /// Spaces to send
        #[arg(display_order = 0)]
        spaces: Vec<String>,
        /// Recipient space name or address (must be a space address)
        #[arg(long, display_order = 1)]
        to: String,
        /// Fee rate to use in sat/vB
        #[arg(long, short)]
        fee_rate: Option<u64>,
    },
    /// Estimates the minimum bid needed for a rollout within the given target blocks
    #[command(name = "estimatebid")]
    EstimateBid {
        /// Rollout within target blocks
        #[arg(default_value = "0")]
        target: usize,
    },
    /// Send the specified amount of BTC to the given name or address
    #[command(
        name = "send",
        override_usage = "space-cli send <AMOUNT> --to <SPACE-OR-ADDRESS>"
    )]
    SendCoins {
        /// Amount to send in satoshi
        #[arg(display_order = 0)]
        amount: u64,
        /// Recipient space name or address
        #[arg(long, display_order = 1)]
        to: String,
        /// Fee rate to use in sat/vB
        #[arg(long, short)]
        fee_rate: Option<u64>,
    },
    /// Get wallet balance
    #[command(name = "balance")]
    Balance,
    /// Pre-create outputs that can be auctioned off during the bidding process
    #[command(name = "createbidouts")]
    CreateBidOuts {
        /// Number of output pairs to create
        /// Each pair can be used to make a bid
        pairs: u8,
        /// Fee rate to use in sat/vB
        #[arg(long, short)]
        fee_rate: Option<u64>,
    },
    /// Bump the fee for a transaction created by this wallet
    #[command(name = "bumpfee")]
    BumpFee {
        txid: Txid,
        /// Fee rate to use in sat/vB
        #[arg(long, short)]
        fee_rate: u64,
    },
    /// Get a spaceout - a Bitcoin output relevant to the Spaces protocol.
    #[command(name = "getspaceout")]
    GetSpaceOut {
        /// The OutPoint
        outpoint: OutPoint,
    },
    /// Get the estimated rollout batch for the specified interval
    #[command(name = "getrollout")]
    GetRollout {
        // Get the estimated rollout for the target interval. Every ~144 blocks (a rollout interval),
        // 10 spaces are released for auction. Specify 0 [default] for the coming interval, 1
        // for the interval after and so on.
        #[arg(default_value = "0")]
        target_interval: usize,
    },
    /// Associate the specified data with a given space (not recommended use Fabric instead)
    /// If for whatever reason it's not possible to use other protocols, then you may use this.
    #[command(name = "setrawfallback")]
    SetRawFallback {
        /// Space name
        space: String,
        /// Hex encoded data
        data: String,
        /// Fee rate to use in sat/vB
        #[arg(long, short)]
        fee_rate: Option<u64>,
    },
    /// List last transactions
    #[command(name = "listtransactions")]
    ListTransactions {
        #[arg(default_value = "10")]
        count: usize,
        #[arg(default_value = "0")]
        skip: usize,
    },
    /// List won spaces including ones
    /// still in auction with a winning bid
    #[command(name = "listspaces")]
    ListSpaces,
    /// List unspent auction outputs i.e. outputs that can be
    /// auctioned off in the bidding process
    #[command(name = "listbidouts")]
    ListBidOuts,
    /// List unspent coins owned by wallet
    #[command(name = "listunspent")]
    ListUnspent,
    /// Get a new Bitcoin address suitable for receiving spaces and coins
    /// (Spaces compatible bitcoin wallets only)
    #[command(name = "getnewspaceaddress")]
    GetSpaceAddress,
    /// Get a new Bitcoin address suitable for receiving coins
    /// compatible with most bitcoin wallets
    #[command(name = "getnewaddress")]
    GetCoinAddress,
    /// Force spend an output owned by wallet (for testing only)
    #[command(name = "forcespend")]
    ForceSpend {
        outpoint: OutPoint,
        #[arg(long, short)]
        fee_rate: u64,
    },
    /// DNS encodes the space and calculates the SHA-256 hash
    #[command(name = "hashspace")]
    HashSpace { space: String },
}

struct SpaceCli {
    wallet: String,
    dust: Option<Amount>,
    force: bool,
    skip_tx_check: bool,
    network: ExtendedNetwork,
    rpc_url: String,
    client: HttpClient,
}

impl SpaceCli {
    async fn configure() -> anyhow::Result<(Self, Args)> {
        let mut args = Args::parse();
        if args.spaced_rpc_url.is_none() {
            args.spaced_rpc_url = Some(default_spaced_rpc_url(&args.chain));
        }

        let client = HttpClientBuilder::default().build(args.spaced_rpc_url.clone().unwrap())?;
        Ok((
            Self {
                wallet: args.wallet.clone(),
                dust: args.dust.map(|d| Amount::from_sat(d)),
                force: args.force,
                skip_tx_check: args.skip_tx_check,
                network: args.chain,
                rpc_url: args.spaced_rpc_url.clone().unwrap(),
                client,
            },
            args,
        ))
    }

    async fn send_request(
        &self,
        req: Option<RpcWalletRequest>,
        bidouts: Option<u8>,
        fee_rate: Option<u64>,
        confirmed_only: bool,
    ) -> Result<(), ClientError> {
        let fee_rate = fee_rate.map(|fee| FeeRate::from_sat_per_vb(fee).unwrap());
        let result = self
            .client
            .wallet_send_request(
                &self.wallet,
                RpcWalletTxBuilder {
                    bidouts,
                    requests: match req {
                        None => vec![],
                        Some(req) => vec![req],
                    },
                    fee_rate,
                    dust: self.dust,
                    force: self.force,
                    confirmed_only,
                    skip_tx_check: self.skip_tx_check,
                },
            )
            .await?;

        println!(
            "{}",
            serde_json::to_string_pretty(&result).expect("serialize")
        );
        Ok(())
    }
}

fn normalize_space(space: &str) -> String {
    let lowercase = space.to_ascii_lowercase();
    if lowercase.starts_with('@') {
        lowercase
    } else {
        format!("@{}", lowercase)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RpcError {
    code: i32,
    message: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (cli, args) = SpaceCli::configure().await?;
    let result = handle_commands(&cli, args.command).await;

    match result {
        Ok(_) => {}
        Err(error) => match ClientError::from(error) {
            Error::Call(rpc) => {
                let error = RpcError {
                    code: rpc.code(),
                    message: rpc.message().to_string(),
                };
                println!(
                    "{}",
                    serde_json::to_string_pretty(&error).expect("serialize")
                );
            }
            Error::Transport(err) => {
                println!(
                    "Transport error: {}: Rpc url: {} (network: {})",
                    err, cli.rpc_url, cli.network
                );
            }
            Error::RestartNeeded(err) => {
                println!("Restart needed: {}", err);
            }
            Error::ParseError(err) => {
                println!("Parse error: {}", err);
            }
            Error::InvalidSubscriptionId => {
                println!("Invalid subscription ID");
            }
            Error::InvalidRequestId(err) => {
                println!("Invalid request ID: {}", err);
            }
            Error::RequestTimeout => {
                println!("Request timeout");
            }
            Error::MaxSlotsExceeded => {
                println!("Max concurrent requests exceeded");
            }
            Error::Custom(msg) => {
                println!("Custom error: {}", msg);
            }
            Error::HttpNotImplemented => {
                println!("HTTP not implemented");
            }
            Error::EmptyBatchRequest(err) => {
                println!("Empty batch request: {}", err);
            }
            Error::RegisterMethod(err) => {
                println!("Register method error: {}", err);
            }
        },
    }
    Ok(())
}

fn hash_space(spaceish: &str) -> anyhow::Result<String> {
    let space = normalize_space(&spaceish);
    let sname = SLabel::from_str(&space)?;
    Ok(hex::encode(Sha256::hash(sname.as_ref())))
}

async fn handle_commands(
    cli: &SpaceCli,
    command: Commands,
) -> std::result::Result<(), ClientError> {
    match command {
        Commands::GetRollout {
            target_interval: target,
        } => {
            let data = cli.client.get_rollout(target).await?;
            println!("{}", serde_json::to_string_pretty(&data)?);
        }
        Commands::EstimateBid { target } => {
            let response = cli.client.estimate_bid(target).await?;
            println!("{} sat", Amount::from_sat(response).to_string());
        }
        Commands::GetSpace { space } => {
            let space_hash = hash_space(&space).map_err(|e| ClientError::Custom(e.to_string()))?;
            let response = cli.client.get_space(&space_hash).await?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        Commands::GetSpaceOut { outpoint } => {
            let response = cli.client.get_spaceout(outpoint).await?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        Commands::CreateWallet => {
            cli.client.wallet_create(&cli.wallet).await?;
        }
        Commands::LoadWallet => {
            cli.client.wallet_load(&cli.wallet).await?;
        }
        Commands::ImportWallet { path } => {
            let content =
                fs::read_to_string(path).map_err(|e| ClientError::Custom(e.to_string()))?;
            let wallet: WalletExport = serde_json::from_str(&content)?;
            cli.client.wallet_import(wallet).await?;
        }
        Commands::ExportWallet { path } => {
            let result = cli.client.wallet_export(&cli.wallet).await?;
            let content = serde_json::to_string_pretty(&result).expect("result");
            fs::write(path, content).map_err(|e| {
                ClientError::Custom(format!("Could not save to path: {}", e.to_string()))
            })?;
        }
        Commands::GetWalletInfo => {
            let result = cli.client.wallet_get_info(&cli.wallet).await?;
            println!("{}", serde_json::to_string_pretty(&result).expect("result"));
        }
        Commands::GetServerInfo => {
            let result = cli.client.get_server_info().await?;
            println!("{}", serde_json::to_string_pretty(&result).expect("result"));
        }
        Commands::Open {
            ref space,
            initial_bid,
            fee_rate,
        } => {
            cli.send_request(
                Some(RpcWalletRequest::Open(OpenParams {
                    name: normalize_space(space),
                    amount: initial_bid,
                })),
                None,
                fee_rate,
                false,
            )
            .await?
        }
        Commands::Bid {
            space,
            amount,
            fee_rate,
            confirmed_only,
        } => {
            cli.send_request(
                Some(RpcWalletRequest::Bid(BidParams {
                    name: normalize_space(&space),
                    amount,
                })),
                None,
                fee_rate,
                confirmed_only,
            )
            .await?
        }
        Commands::CreateBidOuts { pairs, fee_rate } => {
            cli.send_request(None, Some(pairs), fee_rate, false).await?
        }
        Commands::Register {
            space,
            address,
            fee_rate,
        } => {
            cli.send_request(
                Some(RpcWalletRequest::Register(RegisterParams {
                    name: normalize_space(&space),
                    to: address,
                })),
                None,
                fee_rate,
                false,
            )
            .await?
        }
        Commands::Transfer {
            spaces,
            to,
            fee_rate,
        } => {
            let spaces: Vec<_> = spaces.into_iter().map(|s| normalize_space(&s)).collect();
            cli.send_request(
                Some(RpcWalletRequest::Transfer(TransferSpacesParams {
                    spaces,
                    to,
                })),
                None,
                fee_rate,
                false,
            )
            .await?
        }
        Commands::SendCoins {
            amount,
            to,
            fee_rate,
        } => {
            cli.send_request(
                Some(RpcWalletRequest::SendCoins(SendCoinsParams {
                    amount: Amount::from_sat(amount),
                    to,
                })),
                None,
                fee_rate,
                false,
            )
            .await?
        }
        Commands::SetRawFallback {
            mut space,
            data,
            fee_rate,
        } => {
            space = normalize_space(&space);
            let data = match hex::decode(data) {
                Ok(data) => data,
                Err(e) => {
                    return Err(ClientError::Custom(format!(
                        "Could not hex decode data: {}",
                        e
                    )))
                }
            };

            let space_script = protocol::script::SpaceScript::create_set_fallback(data.as_slice());

            cli.send_request(
                Some(RpcWalletRequest::Execute(ExecuteParams {
                    context: vec![space],
                    space_script,
                })),
                None,
                fee_rate,
                false,
            )
            .await?;
        }
        Commands::ListUnspent => {
            let spaces = cli.client.wallet_list_unspent(&cli.wallet).await?;
            println!("{}", serde_json::to_string_pretty(&spaces)?);
        }
        Commands::ListBidOuts => {
            let spaces = cli.client.wallet_list_bidouts(&cli.wallet).await?;
            println!("{}", serde_json::to_string_pretty(&spaces)?);
        }
        Commands::ListTransactions { count, skip } => {
            let txs = cli
                .client
                .wallet_list_transactions(&cli.wallet, count, skip)
                .await?;
            println!("{}", serde_json::to_string_pretty(&txs)?);
        }
        Commands::ListSpaces => {
            let spaces = cli.client.wallet_list_spaces(&cli.wallet).await?;
            println!("{}", serde_json::to_string_pretty(&spaces)?);
        }
        Commands::Balance => {
            let balance = cli.client.wallet_get_balance(&cli.wallet).await?;
            println!("{}", serde_json::to_string_pretty(&balance)?);
        }
        Commands::GetCoinAddress => {
            let response = cli
                .client
                .wallet_get_new_address(&cli.wallet, AddressKind::Coin)
                .await?;
            println!("{}", response);
        }
        Commands::GetSpaceAddress => {
            let response = cli
                .client
                .wallet_get_new_address(&cli.wallet, AddressKind::Space)
                .await?;
            println!("{}", response);
        }
        Commands::BumpFee { txid, fee_rate } => {
            let fee_rate = FeeRate::from_sat_per_vb(fee_rate).expect("valid fee rate");
            let response = cli
                .client
                .wallet_bump_fee(&cli.wallet, txid, fee_rate, cli.skip_tx_check)
                .await?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        Commands::ForceSpend { outpoint, fee_rate } => {
            let result = cli
                .client
                .wallet_force_spend(
                    &cli.wallet,
                    outpoint,
                    FeeRate::from_sat_per_vb(fee_rate).unwrap(),
                )
                .await?;
            println!("{}", serde_json::to_string_pretty(&result).expect("result"));
        }
        Commands::HashSpace { space } => {
            println!(
                "{}",
                hash_space(&space).map_err(|e| ClientError::Custom(e.to_string()))?
            );
        }
    }

    Ok(())
}

fn default_spaced_rpc_url(chain: &ExtendedNetwork) -> String {
    format!("http://127.0.0.1:{}", default_spaces_rpc_port(chain))
}
