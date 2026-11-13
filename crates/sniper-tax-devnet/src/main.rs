//! Devnet helper for the sniper-tax program. It uses the same instruction builders as the
//! integration tests, and talks to the cluster through `curl`.
//!
//! The program must already be deployed (`solana program deploy`) with `KEYPAIR` as its
//! upgrade authority.
//!
//! ```sh
//! # Register with the Pipeline router and create the protocol config (once).
//! KEYPAIR=deployer.json TREASURY=<address> cargo run -p sniper-tax-devnet -- setup
//! # Launch a demo token, snipe it at launch, then claim and sweep the tax.
//! KEYPAIR=deployer.json cargo run -p sniper-tax-devnet -- demo [--wait]
//! # Move the treasury, and propose a new admin (signed by the current admin). ADMIN is
//! # optional; the proposed admin takes over with `accept-admin`.
//! KEYPAIR=admin.json TREASURY=<address> [ADMIN=<address>] cargo run -p sniper-tax-devnet -- config
//! # Take over as the proposed admin (signed by that admin).
//! KEYPAIR=new-admin.json cargo run -p sniper-tax-devnet -- accept-admin
//! # Point the router registration at METADATA_URI (signed by the upgrade authority).
//! KEYPAIR=deployer.json cargo run -p sniper-tax-devnet -- metadata
//! ```
//!
//! `RPC_URL` defaults to `https://api.devnet.solana.com`. `--wait` also buys again after
//! the 90-second tax window, to show the tax gone.

use std::process::Command;
use std::str::FromStr;
use std::thread::sleep;
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{json, Value};
use sniper_tax_client::*;
use solana_hash::Hash;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction::Transaction;
use spl_token_2022_interface::instruction::AuthorityType;

const DEFAULT_RPC: &str = "https://api.devnet.solana.com";
const METADATA_URI: &str =
    "https://raw.githubusercontent.com/MajocoReborn/pipeline-sniper-tax-strate/main/metadata/sniper-tax.devnet.json";
/// Refuse to register if the router's listing fee is above this.
const MAX_LISTING_FEE: u64 = 20_000_000;

const SOL: u64 = 1_000_000_000;
/// Protocol settings `setup` creates the config with.
const PROTOCOL_SHARE_BPS: u16 = 1_000;
const LAUNCH_FEE: u64 = 10_000_000;
/// Solana's packet limit for a serialised transaction.
const MAX_TRANSACTION_SIZE: usize = 1_232;

const TOKEN_DECIMALS: u8 = 6;
const TOKEN_SUPPLY: u64 = 1_000_000_000 * 1_000_000;
const POOL_TOKENS: u64 = 800_000_000 * 1_000_000;
const POOL_QUOTE: u64 = SOL / 5;
const SNIPE: u64 = SOL / 20;

type Result<T> = std::result::Result<T, String>;

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let rpc = Rpc {
        url: std::env::var("RPC_URL").unwrap_or_else(|_| DEFAULT_RPC.to_string()),
    };
    let keypair_path = std::env::var("KEYPAIR").map_err(|_| "set KEYPAIR".to_string())?;
    let payer = read_keypair(&keypair_path)?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("setup") => {
            let treasury = std::env::var("TREASURY").map_err(|_| "set TREASURY".to_string())?;
            let treasury = Pubkey::from_str(&treasury).map_err(|e| e.to_string())?;
            setup(&rpc, &payer, treasury)
        }
        Some("demo") => demo(&rpc, &payer, args.iter().any(|a| a == "--wait")),
        Some("config") => {
            let treasury = wallet_from_env("TREASURY")?;
            let admin = match std::env::var("ADMIN") {
                Ok(value) => Some(Pubkey::from_str(&value).map_err(|e| format!("ADMIN: {e}"))?),
                Err(_) => None,
            };
            update_config(&rpc, &payer, treasury, admin)
        }
        Some("accept-admin") => accept_admin(&rpc, &payer),
        Some("metadata") => update_metadata(&rpc, &payer),
        _ => Err(
            "usage: sniper-tax-devnet setup | demo [--wait] | config | accept-admin | metadata"
                .into(),
        ),
    }
}

fn read_keypair(path: &str) -> Result<Keypair> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let bytes: Vec<u8> = serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))?;
    Keypair::try_from(bytes.as_slice()).map_err(|e| format!("{path}: {e}"))
}

fn explorer(signature: &str) -> String {
    format!("https://explorer.solana.com/tx/{signature}?cluster=devnet")
}

// ---------------------------------------------------------------------------------------
// setup
// ---------------------------------------------------------------------------------------

fn setup(rpc: &Rpc, payer: &Keypair, treasury: Pubkey) -> Result<()> {
    let admin = payer.pubkey();
    println!("program   {PROGRAM_ID}");
    println!("signer    {admin}");
    println!("treasury  {treasury}");
    rpc.account(&PROGRAM_ID)?
        .ok_or("the program is not deployed yet: run `solana program deploy` first")?;

    if rpc.account(&router_pda::strategy(&PROGRAM_ID))?.is_some() {
        println!("already registered with the Pipeline router");
    } else {
        let config = rpc
            .account(&router_pda::config())?
            .ok_or("the Pipeline router has no config on this cluster")?;
        let router = router::decode_config(&config.1).ok_or("unreadable router config")?;
        println!(
            "registering with the router (listing fee {} lamports)",
            router.listing_fee_lamports
        );
        let ix = router::register_strategy(
            &admin,
            &admin,
            &PROGRAM_ID,
            &router.listing_fee_recipient,
            METADATA_URI,
            MAX_LISTING_FEE,
        );
        let signature = rpc.send(&[ix], payer, &[])?;
        println!("registered: {}", explorer(&signature));
    }

    if rpc.account(&config_address())?.is_some() {
        println!("protocol config already exists");
    } else {
        let args = ConfigArgs {
            admin,
            treasury,
            protocol_tax_share_bps: PROTOCOL_SHARE_BPS,
            launch_fee_lamports: LAUNCH_FEE,
        };
        let signature = rpc.send(&[initialize_config_ix(&admin, &args)], payer, &[])?;
        println!("config created: {}", explorer(&signature));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// metadata
// ---------------------------------------------------------------------------------------

/// Points the router's registration of this program at `METADATA_URI`. Only the program's
/// upgrade authority can sign this.
fn update_metadata(rpc: &Rpc, authority: &Keypair) -> Result<()> {
    let strategy = rpc
        .account(&router_pda::strategy(&PROGRAM_ID))?
        .ok_or("the program is not registered with the Pipeline router: run `setup`")?;
    let strategy = router::decode_strategy(&strategy.1).ok_or("unreadable strategy account")?;
    println!("current   {}", strategy.metadata_uri);
    if strategy.metadata_uri == METADATA_URI {
        println!("already up to date");
        return Ok(());
    }
    println!("new       {METADATA_URI}");
    let ix = router::update_metadata(&authority.pubkey(), &PROGRAM_ID, METADATA_URI);
    let signature = rpc.send(&[ix], authority, &[])?;
    println!("updated: {}", explorer(&signature));
    Ok(())
}

// ---------------------------------------------------------------------------------------
// config
// ---------------------------------------------------------------------------------------

/// An address from the environment that a wallet can sign for. A program address (off the
/// ed25519 curve) can't sign, so the treasury's tokens could only be moved by that program.
fn wallet_from_env(name: &str) -> Result<Pubkey> {
    let value = std::env::var(name).map_err(|_| format!("set {name}"))?;
    let key = Pubkey::from_str(&value).map_err(|e| format!("{name}: {e}"))?;
    if !key.is_on_curve() {
        return Err(format!("{name} {key} is not a wallet address"));
    }
    Ok(key)
}

fn protocol_config(rpc: &Rpc) -> Result<ProtocolConfig> {
    let account = rpc
        .account(&config_address())?
        .ok_or("no protocol config")?;
    ProtocolConfig::decode(&account.1).ok_or_else(|| "unreadable config".into())
}

/// Moves the config's treasury, keeping its fees, and proposes `new_admin` if given. The
/// admin changes only when the proposed admin accepts (`accept-admin`), so a proposal for
/// an address nobody controls loses nothing: propose another.
fn update_config(
    rpc: &Rpc,
    admin: &Keypair,
    treasury: Pubkey,
    new_admin: Option<Pubkey>,
) -> Result<()> {
    let config = protocol_config(rpc)?;
    let mut ixs = vec![update_config_ix(
        &admin.pubkey(),
        &ConfigUpdate {
            treasury,
            protocol_tax_share_bps: config.protocol_tax_share_bps,
            launch_fee_lamports: config.launch_fee_lamports,
        },
    )];
    if let Some(new_admin) = new_admin {
        ixs.push(propose_admin_ix(&admin.pubkey(), &new_admin));
    }
    let signature = rpc.send(&ixs, admin, &[])?;
    let config = protocol_config(rpc)?;
    if config.treasury != treasury || new_admin.is_some_and(|a| config.pending_admin != a) {
        return Err("the config did not change".into());
    }
    println!("config treasury {treasury}: {}", explorer(&signature));
    if let Some(new_admin) = new_admin {
        println!("proposed admin {new_admin}; it takes over by running `accept-admin`");
    }
    Ok(())
}

/// The proposed admin (`KEYPAIR`) takes over the config.
fn accept_admin(rpc: &Rpc, new_admin: &Keypair) -> Result<()> {
    let me = new_admin.pubkey();
    if protocol_config(rpc)?.pending_admin != me {
        return Err(format!("{me} is not the proposed admin"));
    }
    let signature = rpc.send(&[accept_admin_ix(&me)], new_admin, &[])?;
    if protocol_config(rpc)?.admin != me {
        return Err("the admin did not change".into());
    }
    println!("config admin {me}: {}", explorer(&signature));
    Ok(())
}

// ---------------------------------------------------------------------------------------
// demo
// ---------------------------------------------------------------------------------------

fn demo(rpc: &Rpc, creator: &Keypair, wait: bool) -> Result<()> {
    let config = rpc
        .account(&config_address())?
        .ok_or("no protocol config: run `setup` first")?;
    let config: ProtocolConfig = ProtocolConfig::decode(&config.1).ok_or("unreadable config")?;
    let me = creator.pubkey();
    let token_program = router::TOKEN_PROGRAM_ID;
    let quote_mint = router::wallet::NATIVE_MINT;

    // A new launch token: full supply to the creator, then no more minting.
    let mint = Keypair::new();
    let creator_token = router_pda::associated_token(&me, &mint.pubkey(), &token_program);
    let creator_quote = router_pda::associated_token(&me, &quote_mint, &token_program);
    let mint_rent = rpc.rent(82)?;
    let ixs = vec![
        solana_system_interface::instruction::create_account(
            &me,
            &mint.pubkey(),
            mint_rent,
            82,
            &token_program,
        ),
        spl_token_2022_interface::instruction::initialize_mint2(
            &token_program,
            &mint.pubkey(),
            &me,
            None,
            TOKEN_DECIMALS,
        )
        .map_err(|e| e.to_string())?,
        ata_ix(&me, &me, &mint.pubkey(), &token_program),
        spl_token_2022_interface::instruction::mint_to(
            &token_program,
            &mint.pubkey(),
            &creator_token,
            &me,
            &[],
            TOKEN_SUPPLY,
        )
        .map_err(|e| e.to_string())?,
        spl_token_2022_interface::instruction::set_authority(
            &token_program,
            &mint.pubkey(),
            None,
            AuthorityType::MintTokens,
            &me,
            &[],
        )
        .map_err(|e| e.to_string())?,
        ata_ix(&me, &me, &quote_mint, &token_program),
        router::wallet::system_transfer(&me, &creator_quote, POOL_QUOTE),
        router::wallet::sync_native(&creator_quote),
    ];
    let signature = rpc.send(&ixs, creator, &[&mint])?;
    println!("token {} created: {}", mint.pubkey(), explorer(&signature));

    // Create the pool through Pipeline and launch it, in one transaction.
    let params = InitParams {
        creator: me,
        tax_start_bps: 9_900,
        tax_end_bps: 0,
        tax_duration_secs: 90,
    };
    let pool = PoolKeys::new(
        params,
        mint.pubkey(),
        token_program,
        quote_mint,
        token_program,
    );
    let address = pool.address;
    let mut ixs = vec![pool.create_market_ix(&me)];
    ixs.extend(pool.fee_account_ixs(&me));
    ixs.push(pool.launch_ix(&config.treasury, POOL_TOKENS, POOL_QUOTE));
    let signature = rpc.send(&ixs, creator, &[])?;
    println!(
        "pool {address} created and launched: {}",
        explorer(&signature)
    );

    // A sniper buys right after the launch.
    let sniper = Keypair::new();
    let sniper_quote = router_pda::associated_token(&sniper.pubkey(), &quote_mint, &token_program);
    let ixs = vec![
        router::wallet::system_transfer(&me, &sniper.pubkey(), SOL / 100),
        ata_ix(&me, &sniper.pubkey(), &quote_mint, &token_program),
        router::wallet::system_transfer(&me, &sniper_quote, SNIPE * 2),
        router::wallet::sync_native(&sniper_quote),
        ata_ix(&me, &sniper.pubkey(), &mint.pubkey(), &token_program),
    ];
    rpc.send(&ixs, creator, &[])?;
    let quoted = rpc.quote(&pool, creator, SNIPE, Some(sniper.pubkey()))?;
    let as_creator = rpc.quote(&pool, creator, SNIPE, Some(me))?;
    println!(
        "quote for {} lamports: sniper gets {} tokens, the creator would get {}",
        SNIPE, quoted.amount_out, as_creator.amount_out
    );
    let before = pool_state(rpc, &pool)?;
    let ix = pool.swap_ix(
        &sniper.pubkey(),
        Side::Buy,
        &exact_in(SNIPE, quoted.amount_out),
    );
    let signature = rpc.send(&[ix], creator, &[&sniper])?;
    let after = pool_state(rpc, &pool)?;
    let tax = (after.accrued_tax_quote - before.accrued_tax_quote)
        + (after.accrued_protocol_quote - before.accrued_protocol_quote);
    println!(
        "sniper bought: {} of {} lamports was tax ({:.1}%): {}",
        tax,
        SNIPE,
        tax as f64 * 100.0 / SNIPE as f64,
        explorer(&signature)
    );

    if wait {
        let launched = after.launch_ts;
        println!("waiting for the 90-second window to pass...");
        let deadline = Instant::now() + Duration::from_secs(150);
        while rpc.cluster_time()? < launched + 91 {
            if Instant::now() > deadline {
                return Err("the cluster clock did not pass the window".into());
            }
            sleep(Duration::from_secs(5));
        }
        let before = pool_state(rpc, &pool)?;
        let ix = pool.swap_ix(&sniper.pubkey(), Side::Buy, &exact_in(SNIPE / 2, 1));
        let signature = rpc.send(&[ix], creator, &[&sniper])?;
        let after = pool_state(rpc, &pool)?;
        let tax = (after.accrued_tax_quote - before.accrued_tax_quote)
            + (after.accrued_protocol_quote - before.accrued_protocol_quote);
        println!(
            "bought again after the window: tax {tax}: {}",
            explorer(&signature)
        );
    }

    // The creator claims their part; anyone sweeps the protocol's part to the treasury.
    let state = pool_state(rpc, &pool)?;
    let signature = rpc.send(&[pool.claim_tax_ix(&creator_quote)], creator, &[])?;
    println!(
        "creator claimed {} lamports of tax: {}",
        state.accrued_tax_quote,
        explorer(&signature)
    );
    let treasury_quote =
        router_pda::associated_token(&config.treasury, &quote_mint, &token_program);
    let ixs = [
        ata_ix(&me, &config.treasury, &quote_mint, &token_program),
        pool.sweep_ix(&config.treasury, &treasury_quote),
    ];
    let signature = rpc.send(&ixs, creator, &[])?;
    println!(
        "swept {} lamports to the treasury: {}",
        state.accrued_protocol_quote,
        explorer(&signature)
    );
    Ok(())
}

fn ata_ix(payer: &Pubkey, owner: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> Instruction {
    spl_associated_token_account_interface::instruction::create_associated_token_account_idempotent(
        payer,
        owner,
        mint,
        token_program,
    )
}

fn pool_state(rpc: &Rpc, pool: &PoolKeys) -> Result<LaunchPool> {
    let account = rpc.account(&pool.address)?.ok_or("pool not found")?;
    LaunchPool::decode(&account.1).ok_or_else(|| "unreadable pool".into())
}

// ---------------------------------------------------------------------------------------
// JSON-RPC over curl
// ---------------------------------------------------------------------------------------

struct Rpc {
    url: String,
}

impl Rpc {
    fn call(&self, method: &str, params: Value) -> Result<Value> {
        let body =
            json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
        for attempt in 0..8 {
            // The HTTP status goes on its own last line, so only a real rate limit or
            // server error is retried.
            let output = Command::new("curl")
                .args(["-sS", "-m", "60", "-X", "POST", &self.url])
                .args([
                    "-H",
                    "Content-Type: application/json",
                    "--data-binary",
                    &body,
                    "-w",
                    "\n%{http_code}",
                ])
                .output()
                .map_err(|e| format!("curl: {e}"))?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            let (text, status) = stdout.rsplit_once('\n').unwrap_or(("", &stdout));
            if status == "429" || status.starts_with('5') || text.trim().is_empty() {
                sleep(Duration::from_millis(500 << attempt));
                continue;
            }
            let reply: Value =
                serde_json::from_str(text).map_err(|e| format!("{method}: {e}: {text}"))?;
            if let Some(error) = reply.get("error") {
                return Err(format!("{method}: {error}"));
            }
            return Ok(reply["result"].clone());
        }
        Err(format!("{method}: no answer from {}", self.url))
    }

    fn blockhash(&self) -> Result<Hash> {
        let result = self.call("getLatestBlockhash", json!([{"commitment": "confirmed"}]))?;
        let hash = result["value"]["blockhash"]
            .as_str()
            .ok_or("no blockhash")?;
        Hash::from_str(hash).map_err(|e| e.to_string())
    }

    fn rent(&self, len: usize) -> Result<u64> {
        self.call("getMinimumBalanceForRentExemption", json!([len]))?
            .as_u64()
            .ok_or_else(|| "bad rent".into())
    }

    /// The cluster's clock, as programs see it: the Clock sysvar's Unix timestamp.
    fn cluster_time(&self) -> Result<i64> {
        let clock = Pubkey::from_str("SysvarC1ock11111111111111111111111111111111")
            .map_err(|e| e.to_string())?;
        let (_, data) = self.account(&clock)?.ok_or("no clock sysvar")?;
        let timestamp = data.get(32..40).ok_or("short clock sysvar")?;
        Ok(i64::from_le_bytes(timestamp.try_into().unwrap()))
    }

    /// `(lamports, data)` of an account, if it exists.
    fn account(&self, address: &Pubkey) -> Result<Option<(u64, Vec<u8>)>> {
        let result = self.call(
            "getAccountInfo",
            json!([address.to_string(), {"encoding": "base64", "commitment": "confirmed"}]),
        )?;
        let value = &result["value"];
        if value.is_null() {
            return Ok(None);
        }
        let data = value["data"][0].as_str().ok_or("no account data")?;
        let data = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| e.to_string())?;
        Ok(Some((value["lamports"].as_u64().unwrap_or(0), data)))
    }

    fn transaction(
        &self,
        ixs: &[Instruction],
        payer: &Keypair,
        signers: &[&Keypair],
    ) -> Result<String> {
        let mut all = vec![payer];
        all.extend_from_slice(signers);
        let tx =
            Transaction::new_signed_with_payer(ixs, Some(&payer.pubkey()), &all, self.blockhash()?);
        let bytes = bincode::serialize(&tx).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_TRANSACTION_SIZE {
            return Err(format!("transaction is {} bytes", bytes.len()));
        }
        Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    /// Sends a transaction and waits until it is confirmed.
    fn send(&self, ixs: &[Instruction], payer: &Keypair, signers: &[&Keypair]) -> Result<String> {
        let tx = self.transaction(ixs, payer, signers)?;
        let signature = self.call(
            "sendTransaction",
            json!([tx, {"encoding": "base64", "preflightCommitment": "confirmed"}]),
        )?;
        let signature = signature.as_str().ok_or("no signature")?.to_string();
        let deadline = Instant::now() + Duration::from_secs(90);
        while Instant::now() < deadline {
            let statuses = self.call("getSignatureStatuses", json!([[signature]]))?;
            let status = &statuses["value"][0];
            if !status.is_null() {
                if !status["err"].is_null() {
                    return Err(format!("{signature} failed: {}", status["err"]));
                }
                if matches!(
                    status["confirmationStatus"].as_str(),
                    Some("confirmed" | "finalized")
                ) {
                    return Ok(signature);
                }
            }
            sleep(Duration::from_secs(1));
        }
        Err(format!("{signature} was not confirmed in time"))
    }

    /// Simulates `pipeline_quote` for a buy of `amount`.
    fn quote(
        &self,
        pool: &PoolKeys,
        payer: &Keypair,
        amount: u64,
        trader: Option<Pubkey>,
    ) -> Result<psi::QuoteResult> {
        let ix = pool.quote_ix(Side::Buy, TradeMode::ExactIn, amount, trader);
        let tx = self.transaction(&[ix], payer, &[])?;
        let result = self.call(
            "simulateTransaction",
            json!([tx, {"encoding": "base64", "commitment": "confirmed"}]),
        )?;
        let value = &result["value"];
        if !value["err"].is_null() {
            return Err(format!("quote failed: {} {}", value["err"], value["logs"]));
        }
        let data = value["returnData"]["data"][0]
            .as_str()
            .ok_or("no return data")?;
        let data = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| e.to_string())?;
        borsh::from_slice(&data).map_err(|e| e.to_string())
    }
}
