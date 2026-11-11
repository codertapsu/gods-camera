//! The network side of the tool: running a hub that devices can pair with,
//! pairing as a device, and using a hub as a paired device.

use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use gc_link::gc_pair::{HubIdentity, Initiator, PairingCode};
use gc_link::{Hello, Hub, PairFinishRequest, PairFinishResponse, PairStartRequest, PairStartResponse, PairingEnd};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// How to reach a hub: where it is, what to present, and whom to trust.
pub(crate) struct HubAccess {
    /// The start of every address, such as `http://127.0.0.1:47800`.
    pub base: String,
    pub token: String,
    pub http: reqwest::Client,
}

impl HubAccess {
    /// A hub on loopback with the token from the environment, or, with the
    /// file written by `gc pair`, a hub on the network as a paired device.
    pub(crate) fn open(hub: String, paired: Option<&Path>) -> Result<Self> {
        let Some(file) = paired else {
            return Ok(Self { base: hub, token: crate::token()?, http: reqwest::Client::new() });
        };
        let text = std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
        let pairing: Pairing = serde_json::from_str(&text).with_context(|| format!("{} is not a pairing", file.display()))?;
        // Only the hub whose certificate was seen while pairing is accepted.
        let tls = gc_link::tls::pinned_client_config(&pairing.fingerprint)?;
        let http = reqwest::Client::builder().tls_backend_preconfigured(tls).build()?;
        Ok(Self { base: format!("https://{}", pairing.address), token: pairing.token, http })
    }
}

/// What a device keeps after pairing.
#[derive(Serialize, Deserialize)]
struct Pairing {
    /// The hub's network listener, as host:port.
    address: String,
    /// The SHA-256 of the hub's certificate. No other certificate is accepted.
    fingerprint: String,
    device_id: String,
    /// What this device presents with every request.
    token: String,
}

/// Write a file that only its owner can read.
fn write_private(path: &Path, text: &str) -> Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).with_context(|| format!("cannot write {}", path.display()))?;
    #[cfg(unix)]
    {
        // The mode above only applies to a new file.
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(text.as_bytes())?;
    Ok(())
}

/// The hub's certificate and key, from the data folder. They are made on the
/// first run and kept, because paired devices trust that certificate only.
pub(crate) fn hub_identity(data_dir: &Path, name: &str) -> Result<String> {
    let file = data_dir.join("hub-identity.pem");
    if file.exists() {
        let pem = std::fs::read_to_string(&file).with_context(|| format!("cannot read {}", file.display()))?;
        HubIdentity::from_pem(&pem).with_context(|| format!("{} is damaged", file.display()))?;
        return Ok(pem);
    }
    std::fs::create_dir_all(data_dir)?;
    let identity = HubIdentity::generate(name)?;
    write_private(&file, identity.to_pem())?;
    Ok(identity.to_pem().to_string())
}

/// Show a pairing code, and say what became of it.
pub(crate) async fn show_code(hub: Hub) {
    let code = match hub.open_pairing(gc_link::DEFAULT_PAIRING_TTL) {
        Ok(code) => code,
        Err(error) => return eprintln!("cannot pair: {error}"),
    };
    println!("pairing code {code} (good for {} minutes, for one device)", gc_link::DEFAULT_PAIRING_TTL.as_secs() / 60);
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        let status = hub.pairing_status();
        match status.ended {
            None => continue,
            Some(PairingEnd::Paired) => {
                let name = status.paired.map(|device| device.name).unwrap_or_default();
                println!("paired with {name}");
            }
            Some(PairingEnd::TooManyAttempts) => println!("pairing closed: too many wrong codes"),
            Some(PairingEnd::Expired) => println!("pairing closed: nobody paired in time"),
            Some(PairingEnd::Closed) => println!("pairing closed"),
        }
        return;
    }
}

/// What the hub said when it turned a request down.
async fn refused(response: reqwest::Response) -> String {
    let status = response.status();
    let reason = response.json::<gc_link::ErrorBody>().await.map(|body| body.error).unwrap_or_default();
    format!("the hub answered {status}: {reason}")
}

/// Pair with the hub at `address` as a device named `name`, and save the result in `out`.
pub(crate) async fn pair(address: &str, code: &str, name: &str, out: &Path) -> Result<()> {
    let code = PairingCode::parse(code)?;
    let device = Initiator::new(&code, name)?;
    let base = format!("https://{address}");
    // The hub's certificate is taken as it comes; the exchange below decides
    // whether it is the hub's.
    let (tls, seen) = gc_link::tls::pairing_client_config();
    let http = reqwest::Client::builder().tls_backend_preconfigured(tls).build()?;

    let hello = http.get(format!("{base}/v1/hello")).send().await.with_context(|| format!("no hub answers at {address}"))?;
    if !hello.status().is_success() {
        bail!("{}", refused(hello).await);
    }
    let hello: Hello = hello.json().await.context("that is not a hub")?;
    if !hello.pairing_open {
        bail!("{} is not showing a code at the moment", hello.name);
    }

    let begun = http
        .post(format!("{base}/v1/pair/start"))
        .json(&PairStartRequest { device_name: device.device_name().to_string(), message: BASE64.encode(device.hello()) })
        .send()
        .await?;
    if !begun.status().is_success() {
        bail!("{}", refused(begun).await);
    }
    let begun: PairStartResponse = begun.json().await?;
    let fingerprint = seen.fingerprint().context("no connection was made")?;
    let (device, confirmation) = device.finish(&BASE64.decode(&begun.message)?, &fingerprint)?;

    let ended = http
        .post(format!("{base}/v1/pair/finish"))
        .json(&PairFinishRequest { session: begun.session, confirmation: BASE64.encode(confirmation) })
        .send()
        .await?;
    if !ended.status().is_success() {
        // Wrong code, most likely. The hub does not say, and neither can this tool.
        bail!("{}", refused(ended).await);
    }
    let ended: PairFinishResponse = ended.json().await?;
    let grant = device
        .accept(&BASE64.decode(&ended.confirmation)?, &BASE64.decode(&ended.grant)?)
        .context("what answered is not the hub that shows the code; nothing was saved")?;

    let pairing = Pairing { address: address.to_string(), fingerprint, device_id: grant.device_id, token: grant.token };
    write_private(out, &serde_json::to_string_pretty(&pairing)?)?;
    println!("paired with {} as {}; saved in {}", hello.name, pairing.device_id, out.display());
    Ok(())
}
