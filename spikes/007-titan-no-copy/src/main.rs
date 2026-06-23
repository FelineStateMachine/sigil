//! Spike 007: No-copy/paste — derive Iroh identity from Titan key
//!
//! Both host and client derive the SAME Iroh SecretKey from the Titan's
//! hmac-secret extension. Uses resident keys (discoverable credentials)
//! so the credential persists on the Titan — no credential ID needs to
//! be shared between machines.
//!
//! First run on a Titan: creates a resident credential.
//! Subsequent runs: reuses the existing credential via get_assertion
//! without specifying a credential ID.
//!
//! Usage:
//!   cargo run -- host    # Tap Titan, starts hosting
//!   cargo run -- client  # Tap Titan, auto-connects to host

use anyhow::{bail, Context, Result};
use ctap_hid_fido2::fidokey::{
    GetAssertionArgsBuilder, MakeCredentialArgsBuilder,
    get_assertion::Extension as Gext,
    get_assertion::get_assertion_params::Assertion,
    make_credential::Extension as Mext,
};
use ctap_hid_fido2::{verifier, Cfg, FidoKeyHidFactory};
use iroh::{Endpoint, SecretKey, endpoint::presets};
use iroh::endpoint::Connection;
use iroh::protocol::{ProtocolHandler, Router};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Duration;

const ALPN: &[u8] = b"keyhome/titan-probe/0";
const RPID: &str = "keyhome";
const SALT_MESSAGE: &str = "keyhome-iroh-identity-v1";

// ─── Titan HMAC-Secret Derivation ────────────────────────────────────────────

fn read_pin() -> Result<String> {
    if let Ok(p) = std::env::var("TITAN_PIN") {
        if !p.is_empty() {
            return Ok(p);
        }
    }
    eprint!("Enter Titan PIN: ");
    std::io::Write::flush(&mut std::io::stderr())?;
    let mut pin = String::new();
    std::io::stdin().read_line(&mut pin)?;
    let pin = pin.trim().to_string();
    if pin.is_empty() {
        bail!("PIN is required");
    }
    Ok(pin)
}

/// Derive a 32-byte secret from the Titan using hmac-secret extension.
///
/// Strategy: try get_assertion WITHOUT a credential ID first (uses resident key).
/// If that fails (no credential exists yet), create a resident credential,
/// then get_assertion with it.
///
/// Same Titan + same PIN + same salt = same 32 bytes every time.
fn derive_secret_from_titan() -> Result<[u8; 32]> {
    let pin = read_pin()?;

    let cfg = Cfg::init();
    let device = FidoKeyHidFactory::create(&cfg)
        .context("Failed to open FIDO2 device. Is the Titan plugged in?")?;

    eprintln!("[titan] FIDO2 device opened");

    let salt: [u8; 32] = {
        let mut hasher = Sha256::new();
        hasher.update(SALT_MESSAGE.as_bytes());
        let result = hasher.finalize();
        let mut s = [0u8; 32];
        s.copy_from_slice(&result);
        s
    };

    // Step 1: Try get_assertion WITHOUT credential ID (uses resident key)
    eprintln!("[titan] Trying get_assertion with resident key (TAP TITAN when it blinks)");

    let challenge = verifier::create_challenge();
    let get_args = GetAssertionArgsBuilder::new(RPID, &challenge)
        .pin(&pin)
        .extensions(&[Gext::HmacSecret(Some(salt))])
        .build();

    match device.get_assertion_with_args(&get_args) {
        Ok(assertions) => {
            eprintln!("[titan] resident key found, assertion received");
            return extract_hmac_secret(&assertions);
        }
        Err(e) => {
            eprintln!("[titan] no resident key found ({}), creating one...", e);
        }
    }

    // Step 2: Create a resident credential with hmac-secret
    eprintln!("[titan] make_credential (TAP TITAN when it blinks)");

    let challenge = verifier::create_challenge();
    let make_args = MakeCredentialArgsBuilder::new(RPID, &challenge)
        .pin(&pin)
        .extensions(&[Mext::HmacSecret(Some(true))])
        .build();

    let attestation = device
        .make_credential_with_args(&make_args)
        .context("make_credential failed")?;

    let verify_result = verifier::verify_attestation(RPID, &challenge, &attestation);
    if !verify_result.is_success {
        bail!("Attestation verification failed");
    }
    let credential_id = verify_result.credential_id;
    eprintln!("[titan] credential created, id_len={}", credential_id.len());

    // Step 3: Get assertion with the new credential
    eprintln!("[titan] get_assertion (TAP TITAN when it blinks)");

    let challenge2 = verifier::create_challenge();
    let get_args = GetAssertionArgsBuilder::new(RPID, &challenge2)
        .pin(&pin)
        .credential_id(&credential_id)
        .extensions(&[Gext::HmacSecret(Some(salt))])
        .build();

    let assertions = device
        .get_assertion_with_args(&get_args)
        .context("get_assertion failed")?;

    extract_hmac_secret(&assertions)
}

fn extract_hmac_secret(assertions: &[Assertion]) -> Result<[u8; 32]> {
    for ext in &assertions[0].extensions {
        if let Gext::HmacSecret(Some(output)) = ext {
            let mut secret = [0u8; 32];
            secret.copy_from_slice(&output[..]);
            eprintln!("[titan] derived 32-byte secret from hmac-secret");
            return Ok(secret);
        }
    }
    bail!("No hmac-secret in assertion response")
}

/// Derive an Iroh SecretKey from the Titan.
fn derive_iroh_secret_from_titan() -> Result<SecretKey> {
    let secret_bytes = derive_secret_from_titan()?;
    let secret_key = SecretKey::from_bytes(&secret_bytes);
    eprintln!("[titan] derived Iroh node ID: {}", secret_key.public());
    Ok(secret_key)
}

// ─── Host ────────────────────────────────────────────────────────────────────

async fn host() -> Result<()> {
    eprintln!("[host] Deriving identity from Titan...");
    let secret = derive_iroh_secret_from_titan()?;

    let endpoint = Endpoint::builder(presets::N0)
        .secret_key(secret)
        .bind()
        .await
        .context("Failed to bind endpoint")?;

    let _ = tokio::time::timeout(Duration::from_secs(5), endpoint.online()).await;

    let node_id = endpoint.id();
    println!("[host] node_id={}", node_id);
    eprintln!("[host] waiting for connections on ALPN {:?}...", ALPN);

    let handler = Arc::new(TitanProbeHandler);
    let _router = Router::builder(endpoint.clone())
        .accept(ALPN, handler)
        .spawn();

    tokio::signal::ctrl_c().await?;
    endpoint.close().await;
    Ok(())
}

#[derive(Debug)]
struct TitanProbeHandler;

impl ProtocolHandler for TitanProbeHandler {
    async fn accept(&self, conn: Connection) -> Result<(), iroh::protocol::AcceptError> {
        eprintln!("[host] client connected: {}", conn.remote_id());

        let (mut send, mut recv) = conn.accept_bi().await?;

        let mut buf = [0u8; 256];
        match recv.read(&mut buf).await {
            Ok(Some(n)) => {
                eprintln!("[host] received: {}", String::from_utf8_lossy(&buf[..n]));
            }
            _ => {}
        }

        let response = "Hello from titan-derived host!";
        send.write_all(response.as_bytes()).await.ok();

        Ok(())
    }
}

// ─── Client ──────────────────────────────────────────────────────────────────

async fn client() -> Result<()> {
    eprintln!("[client] Deriving host identity from Titan...");
    let host_secret = derive_iroh_secret_from_titan()?;

    let host_node_id = host_secret.public();
    println!("[client] derived host node ID: {}", host_node_id);

    // Client uses a RANDOM identity (can't connect to yourself)
    let client_secret = SecretKey::generate();
    eprintln!("[client] client node ID: {}", client_secret.public());

    let endpoint = Endpoint::builder(presets::N0)
        .secret_key(client_secret)
        .bind()
        .await
        .context("Failed to bind endpoint")?;

    let _ = tokio::time::timeout(Duration::from_secs(10), endpoint.online()).await;

    // Connect using just the node ID + N0 relay — no address JSON
    let addr = iroh::EndpointAddr::new(host_node_id)
        .with_relay_url("https://usw1-1.relay.n0.iroh.link./".parse()
            .context("invalid relay URL")?);

    eprintln!("[client] connecting to host via relay (no address JSON)...");
    let conn = endpoint.connect(addr, ALPN).await
        .context("Failed to connect to host")?;

    println!("[client] connected! host node: {}", conn.remote_id());

    let (mut send, mut recv) = conn.open_bi().await?;

    send.write_all(b"Hello from titan-derived client!").await?;

    let mut buf = [0u8; 256];
    match recv.read(&mut buf).await {
        Ok(Some(n)) => {
            println!("[client] host response: {}", String::from_utf8_lossy(&buf[..n]));
        }
        _ => {
            println!("[client] no response received");
        }
    }

    println!("\n=== Spike 007: PASS ===");
    println!("Connected to host using only Titan-derived node ID — no address copy/paste");

    let _ = endpoint.close().await;
    Ok(())
}

// ─── Main ────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(|s| s.as_str()) {
        Some("host") => host().await,
        Some("client") => client().await,
        _ => {
            bail!("usage: cargo run -- host | client\n\nSet TITAN_PIN env var or enter PIN when prompted.")
        }
    }
}
